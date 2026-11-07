//! Opaque login-session handling.
//!
//! The module never sees a cookie, a token or a password. A *challenge* handle
//! (`ch_…`) and a *session* handle (`s_…`) are treated as opaque strings: we store
//! small per-challenge state in this process and pass the handle back to the
//! framework untouched. If the process restarts, in-memory challenges are gone and
//! the framework must call `auth.begin` again.
//!
//! Fixture mode fabricates a self-contained QR challenge. Live mode performs the
//! real Passport QR handshake through `cw.net.fetch`.

use std::{collections::HashMap, sync::Mutex};

use serde_json::{Value, json};

use crate::error::ModuleError;
use crate::host::{FetchRequest, Host};

use super::{endpoints, number};

/// How long a fixture / live QR challenge stays valid.
pub const QR_TTL_SECONDS: i64 = 120;

pub const FIXTURE_DISPLAY_NAME: &str = "演示学生";
pub const FIXTURE_ACCOUNT_RAW: &str = "cf-demo-10001";

/// A 116x116 QR-looking PNG, recorded for the fixture. It is a demo image, not a
/// scannable code: fixture mode never talks to 智慧树.
pub const FIXTURE_QR_DATA_URL: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAHQAAAB0CAIAAADb+IFwAAABlUlEQVR42u3aUQ6CQAwFQO/h/c+JJzABtq92cfpJENnBpO4rr0PF6oUALlwFFy5cBXcc7jtQ6e86s+DOdcGFCxduJe63888cv/rZletcPefqQ4ILFy7cStyqhpBYzEpDS/yY4MKFC7cbN7H4lXuACxcu3J1wVxrXClbijz1cuHDh7oSbaDJVwcr2qRhcuHDhhgZ5E44/dvoLFy7c7XCnVVUw1Flw4cKF242bCG4mg8KFCxfuHdx0yD1tozFi+gsXLly4hUFM5wsciRf0Ig0NLly4cI/eF+7SjaVqE9Q6iYALF+7f4qaDm6qG1tkY4cKFC7cSdyVkqQp90teBCxcu3OkNLT1wrHqQ6XuIDCjhwoULt3DB0zYsWw4o4cKF+0jcRNicboDHsIILFy7cbtyVh1EVisOFCxfudNx3oKpCoslNDC5cuHArw/JfbSJ2hIYLFy7cVdxdXk6GCxcuXLj1wY2GBhcuXLj3NxFV4Q5cuHDhCm7q4R61iYALFy7chgHlU5sbXLhw4Z7FVXDhwlVw4Sq4cDeuD84hAvDLmDHtAAAAAElFTkSuQmCC";

/// Base64 of the PNG magic bytes; any real 智慧树 QR image starts with this.
const PNG_BASE64_PREFIX: &str = "iVBORw0KGgo";

#[derive(Clone, Debug)]
pub struct Challenge {
    pub kind: String,
    pub expires_at: i64,
    pub polls: u32,
    pub qr_token: Option<String>,
    /// Last QR poll reply. `auth.complete` needs `oncePassword` and `uuid` from it; a live
    /// challenge therefore cannot survive a module restart (the framework re-runs auth.begin).
    pub reply: Option<Value>,
}

impl Challenge {
    fn fixture(expires_at: i64) -> Self {
        Self {
            kind: "qr".to_owned(),
            expires_at,
            polls: 0,
            qr_token: None,
            reply: None,
        }
    }
}

#[derive(Default)]
pub struct ChallengeStore {
    inner: Mutex<HashMap<String, Challenge>>,
}

impl ChallengeStore {
    pub fn remember(&self, id: String, challenge: Challenge) {
        self.lock().insert(id, challenge);
    }

    pub fn snapshot(&self, id: &str) -> Option<Challenge> {
        self.lock().get(id).cloned()
    }

    pub fn set_reply(&self, id: &str, reply: Value) {
        if let Some(entry) = self.lock().get_mut(id) {
            entry.reply = Some(reply);
        }
    }

    pub fn take(&self, id: &str) -> Option<Challenge> {
        self.lock().remove(id)
    }

    /// Fixture poll transition: the first poll reports `waiting`, every poll after
    /// that reports `connected`, and a challenge past `expires_at` reports `expired`.
    /// Unknown challenges are re-created lazily so a restarted module does not wedge
    /// the demo login.
    pub fn fixture_poll(&self, challenge: &str, now: i64) -> &'static str {
        let mut store = self.lock();
        let entry = store
            .entry(challenge.to_owned())
            .or_insert_with(|| Challenge::fixture(now.saturating_add(QR_TTL_SECONDS)));
        if now >= entry.expires_at {
            return "expired";
        }
        entry.polls = entry.polls.saturating_add(1);
        if entry.polls >= 2 {
            "connected"
        } else {
            "waiting"
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Challenge>> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

// --- fixture ----------------------------------------------------------------------

pub fn begin_fixture(store: &ChallengeStore, session: &str, now: i64) -> Value {
    let expires_at = now.saturating_add(QR_TTL_SECONDS);
    store.remember(session.to_owned(), Challenge::fixture(expires_at));
    json!({
        "challenge": {
            "id": session,
            "kind": "qr",
            "image": FIXTURE_QR_DATA_URL,
            "expires_at": expires_at,
        }
    })
}

pub fn poll_fixture(store: &ChallengeStore, challenge: &str, now: i64) -> Value {
    json!({ "state": store.fixture_poll(challenge, now) })
}

/// Passport SSO exchange for a confirmed QR, then a profile read. Mirrors
/// `ZhsClient::finish_login` / `verify` in `courseweave/src/zhs.rs`.
pub fn complete_live(
    host: &dyn Host,
    store: &ChallengeStore,
    challenge: &str,
    session: &str,
) -> Result<Value, ModuleError> {
    let entry = store.take(challenge).ok_or_else(|| {
        ModuleError::login_expired("扫码挑战已不在本模块内存中，请重新调用 auth.begin")
    })?;
    let reply = entry.reply.ok_or_else(|| {
        ModuleError::invalid_params("扫码尚未确认，请先轮询到 connected 再完成登录")
    })?;
    let account_raw = super::id_string(&reply["uuid"])
        .ok_or_else(|| ModuleError::protocol_changed("扫码结果缺少 uuid"))?;
    let profile = super::live::finish_login(host, session, &reply)?;
    let display_name = profile
        .get("display_name")
        .and_then(Value::as_str)
        .unwrap_or("智慧树账号")
        .to_owned();
    // The account keeps the platform identity, not the one-time password.
    Ok(json!({ "display_name": display_name, "account_raw": account_raw }))
}

pub fn complete_fixture() -> Value {
    json!({
        "display_name": FIXTURE_DISPLAY_NAME,
        "account_raw": FIXTURE_ACCOUNT_RAW,
    })
}

pub fn verify_fixture() -> Value {
    json!({ "display_name": FIXTURE_DISPLAY_NAME })
}

// --- live -------------------------------------------------------------------------

/// GET the Passport login page (so its cookie jar is initialised), then GET the QR
/// image. Mirrors `ZhsClient::qr_code` in `courseweave/src/zhs.rs`.
pub fn begin_live(
    host: &dyn Host,
    store: &ChallengeStore,
    session: &str,
) -> Result<Value, ModuleError> {
    let bootstrap = FetchRequest::get(
        session,
        format!(
            "{}{}",
            endpoints::PASSPORT_BASE,
            endpoints::PASSPORT_LOGIN_PATH
        ),
    );
    // The body is a landing page; only the cookies matter to the framework.
    let _ = super::fetch(host, bootstrap)?;

    let reply = super::fetch_json(
        host,
        FetchRequest::get(
            session,
            format!("{}{}", endpoints::PASSPORT_BASE, endpoints::QR_IMAGE_PATH),
        ),
    )?;

    let image = reply
        .get("img")
        .and_then(Value::as_str)
        .filter(|image| !image.is_empty() && image.len() <= 1024 * 1024)
        .ok_or_else(|| ModuleError::protocol_changed("智慧树二维码响应缺少 img"))?;
    if !image.starts_with(PNG_BASE64_PREFIX) {
        return Err(ModuleError::protocol_changed(
            "智慧树二维码不是 PNG base64（协议可能已变化）",
        ));
    }
    let token = reply
        .get("qrToken")
        .and_then(Value::as_str)
        .filter(|token| !token.is_empty() && token.len() <= 512)
        .ok_or_else(|| ModuleError::protocol_changed("智慧树二维码响应缺少 qrToken"))?;

    let now = host.now()?;
    let expires_at = now.saturating_add(QR_TTL_SECONDS);
    store.remember(
        session.to_owned(),
        Challenge {
            kind: "qr".to_owned(),
            expires_at,
            polls: 0,
            qr_token: Some(token.to_owned()),
            reply: None,
        },
    );

    Ok(json!({
        "challenge": {
            "id": session,
            "kind": "qr",
            "image": format!("data:image/png;base64,{image}"),
            "expires_at": expires_at,
        }
    }))
}

/// GET `qrCodeLogin/getLoginQrInfo?qrToken=…` and map the platform status:
/// `-1` waiting, `0` scanned, `1` connected, `2` expired, `3` failed.
pub fn poll_live(
    host: &dyn Host,
    store: &ChallengeStore,
    challenge: &str,
    session: &str,
) -> Result<Value, ModuleError> {
    let challenge_id = challenge.to_owned();
    let challenge = store.snapshot(&challenge_id).ok_or_else(|| {
        ModuleError::login_expired(
            "扫码挑战已不在本模块内存中（模块可能已重启），请重新调用 auth.begin",
        )
    })?;
    let now = host.now()?;
    if now >= challenge.expires_at {
        return Ok(json!({ "state": "expired" }));
    }
    let token = challenge.qr_token.as_deref().ok_or_else(|| {
        ModuleError::protocol_changed("扫码挑战缺少 qrToken，请重新调用 auth.begin")
    })?;

    let url = format!(
        "{}{}?qrToken={}",
        endpoints::PASSPORT_BASE,
        endpoints::QR_INFO_PATH,
        encode_query_value(token)
    );
    let reply = super::fetch_json(host, FetchRequest::get(session, url))?;
    if number(&reply["status"]) == Some(1) {
        store.set_reply(&challenge_id, reply.clone());
    }
    let state = match number(&reply["status"]) {
        Some(-1) => "waiting",
        Some(0) => "scanned",
        Some(1) => "connected",
        Some(2) => "expired",
        Some(3) => "failed",
        Some(other) => {
            return Err(ModuleError::protocol_changed(format!(
                "智慧树扫码状态 {other} 不在已知范围"
            )));
        }
        None => {
            return Err(ModuleError::protocol_changed(
                "智慧树扫码响应缺少 status 字段",
            ));
        }
    };
    Ok(json!({ "state": state }))
}

/// GET `gateway/f/v1/login/getLoginUserInfo?time=…`; mirrors `verify()`.
pub fn verify_live(host: &dyn Host, session: &str) -> Result<Value, ModuleError> {
    let time = host.now()?.saturating_mul(1000);
    let url = format!(
        "{}{}?time={time}",
        endpoints::SERVICE_BASE,
        endpoints::LOGIN_USER_INFO_PATH
    );
    let reply = super::fetch_json(host, FetchRequest::get(session, url))?;
    super::check_code(&reply, &[200])?;
    let profile = reply
        .get("result")
        .filter(|result| result.is_object())
        .ok_or_else(|| ModuleError::login_expired("智慧树会话已失效，请重新扫码"))?;
    let display_name = profile
        .get("realName")
        .or_else(|| profile.get("name"))
        .and_then(Value::as_str)
        .filter(|name| !name.trim().is_empty())
        .unwrap_or("智慧树账号");
    Ok(json!({ "display_name": display_name }))
}

/// Percent-encode a single query value. The module cannot pull in a URL crate, and
/// `qrToken` is the only value we ever place in a URL.
fn encode_query_value(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(byte as char);
        } else {
            use std::fmt::Write as _;
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_poll_transitions_and_expires() {
        let store = ChallengeStore::default();
        begin_fixture(&store, "ch_1", 1_000);
        assert_eq!(store.fixture_poll("ch_1", 1_001), "waiting");
        assert_eq!(store.fixture_poll("ch_1", 1_002), "connected");
        assert_eq!(store.fixture_poll("ch_1", 1_120), "expired");
    }

    #[test]
    fn query_values_are_percent_encoded() {
        assert_eq!(encode_query_value("a-b_c.d~9"), "a-b_c.d~9");
        assert_eq!(encode_query_value("a b&c"), "a%20b%26c");
    }
}
