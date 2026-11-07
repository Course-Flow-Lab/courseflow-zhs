//! 智慧树 platform implementation behind the `PlatformApi` trait.
//!
//! Two sources are supported, selected by `CF_MODULE_SETTINGS.source`:
//!   * `fixture` (default) — no network at all, deterministic demo data;
//!   * `live` — real calls through `cw.net.fetch`, as far as this draft has ported.
//!
//! The module never sees cookies, tokens or credentials: challenge and session
//! handles are opaque strings passed back to the framework.

pub mod catalog;
pub mod endpoints;
pub mod live;
pub mod session;
pub mod crypto;
pub mod study;

pub use catalog::{Course, Unit, UnitState};

use serde_json::{json, Value};

use crate::error::ModuleError;
use crate::host::{FetchRequest, FetchResponse, Host};
use crate::platform::{self, PlatformApi, PlatformDescriptor};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Fixture,
    Live,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fixture => "fixture",
            Self::Live => "live",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Settings {
    pub source: Source,
}

impl Default for Settings {
    fn default() -> Self {
        // A platform module's default is the real platform. `fixture` is an explicit opt-in for
        // tests, conformance runs and demos without an account.
        Self {
            source: Source::Live,
        }
    }
}

impl Settings {
    /// Parse `CF_MODULE_SETTINGS` (a JSON object, `{}` when unset). Malformed input or
    /// an unknown `source` falls back to fixture and returns human-readable warnings
    /// for stderr; it never takes the module down.
    pub fn from_env(raw: Option<&str>) -> (Self, Vec<String>) {
        let mut warnings = Vec::new();
        let Some(raw) = raw else {
            return (Self::default(), warnings);
        };
        let parsed: Value = match serde_json::from_str(raw) {
            Ok(parsed) => parsed,
            Err(error) => {
                warnings.push(format!(
                    "CF_MODULE_SETTINGS 不是合法 JSON（{error}），已回退 fixture"
                ));
                return (Self::default(), warnings);
            }
        };
        let Some(settings) = parsed.as_object() else {
            warnings.push("CF_MODULE_SETTINGS 不是 JSON 对象，已回退 fixture".to_owned());
            return (Self::default(), warnings);
        };
        let source = match settings.get("source") {
            None | Some(Value::Null) => "live".to_owned(),
            Some(Value::String(source)) => source.clone(),
            Some(other) => {
                warnings.push(format!(
                    "CF_MODULE_SETTINGS.source 必须是字符串（收到 {other}），已回退 fixture"
                ));
                "fixture".to_owned()
            }
        };
        let source = match source.as_str() {
            "fixture" => Source::Fixture,
            "live" => Source::Live,
            other => {
                warnings.push(format!(
                    "未知的 CF_MODULE_SETTINGS.source = {other}，已回退 live"
                ));
                Source::Live
            }
        };
        (Self { source }, warnings)
    }
}

/// Module storage namespace and keys for the catalogue snapshot (module API §4.2).
const CATALOG_NAMESPACE: &str = "catalog";
const ENTRIES_KEY: &str = "entries";

/// How much watched time one `study.step` writes. The reference client flushes every 30 s.
const STUDY_CHUNK_SECONDS: i64 = 30;

fn units_key(course: &Course) -> String {
    format!("units/{}/{}", course.id, course.recruit_id)
}

pub struct Zhs {
    settings: Settings,
    challenges: session::ChallengeStore,
    keys: live::Keys,
}

impl Zhs {
    pub fn new(settings: Settings) -> Self {
        Self {
            settings,
            challenges: session::ChallengeStore::default(),
            keys: live::Keys::default(),
        }
    }

    pub fn source(&self) -> Source {
        self.settings.source
    }

    fn entries(&self, host: &dyn Host, session: &str) -> Result<Value, ModuleError> {
        let value = host.state_get(CATALOG_NAMESPACE, ENTRIES_KEY, session)?;
        Ok(if value.is_object() {
            value
        } else {
            serde_json::json!({})
        })
    }

    /// The platform's per-course `secret` entry, refreshed from the course list when the cached
    /// snapshot does not have it (first run after an upgrade, or a course added on the platform).
    fn entry_for(
        &self,
        host: &dyn Host,
        session: &str,
        course: &Course,
    ) -> Result<String, ModuleError> {
        let key = format!("{}/{}", course.id, course.recruit_id);
        if let Some(entry) = self
            .entries(host, session)?
            .get(key.as_str())
            .and_then(Value::as_str)
            .map(str::to_owned)
        {
            return Ok(entry);
        }
        let (_, entries) = live::courses(host, &self.keys, session)?;
        host.state_set(CATALOG_NAMESPACE, ENTRIES_KEY, &entries, session)?;
        entries
            .get(key.as_str())
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| {
                ModuleError::new(
                    "platform_rejected",
                    "平台没有提供这门课程的目录入口，暂时只能查看课程摘要",
                )
            })
    }

    /// The reference refuses to open a chapter while an earlier one is unfinished, unless the
    /// course allows jumping. Uses the outline snapshot written by `catalog.units`.
    fn earlier_incomplete(
        &self,
        host: &dyn Host,
        session: &str,
        course: &Course,
        unit: &Unit,
    ) -> Result<bool, ModuleError> {
        let snapshot = host.state_get(CATALOG_NAMESPACE, &units_key(course), session)?;
        let units: Vec<Unit> = serde_json::from_value(snapshot).unwrap_or_default();
        if units.is_empty() {
            return Ok(false);
        }
        Ok(units
            .iter()
            .take_while(|candidate| candidate.chapter_id != unit.chapter_id)
            .any(|candidate| {
                let progress = match (candidate.watched_seconds, candidate.duration_seconds) {
                    (Some(watched), Some(duration)) if duration > 0 => {
                        watched as f64 / duration as f64 * 100.0
                    }
                    _ => 0.0,
                };
                candidate.state != catalog::UnitState::Completed && progress <= 81.0
            }))
    }

    /// One durable study step: prepare once, then write `STUDY_CHUNK_SECONDS` at a time and pace
    /// the next step by the chunk (`wait_ms`), so the framework can pause between writes.
    fn live_step(
        &self,
        host: &dyn Host,
        session: &str,
        course: &Course,
        unit: &Unit,
        checkpoint: Option<&Value>,
    ) -> Result<Value, ModuleError> {
        // A completed unit has nothing to open. The reference learning driver returns early for
        // `target.completed`; asking the gateway to open it answers `421` (course finished).
        if unit.state == catalog::UnitState::Completed {
            return Ok(json!({
                "status": "completed",
                "evidence": {
                    "already_completed": true,
                    "watched_seconds": unit.watched_seconds,
                },
            }));
        }

        let resumed = checkpoint.filter(|value| {
            value.get("phase").and_then(Value::as_str) == Some("running")
        });
        let identity = host.identity(session)?;
        let account_raw = identity
            .get("account_raw")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();

        let mut prepared = match resumed {
            Some(value) => live::Prepared::from_checkpoint(value)?,
            None => {
                let entry = self.entry_for(host, session, course)?;
                let earlier = self.earlier_incomplete(host, session, course, unit)?;
                live::prepare(
                    host,
                    &self.keys,
                    session,
                    &account_raw,
                    course,
                    &entry,
                    unit,
                    earlier,
                )?
            }
        };

        if prepared.position >= prepared.duration {
            return Ok(json!({
                "status": "completed",
                "evidence": { "watched_seconds": prepared.position },
                "checkpoint": prepared.to_checkpoint(),
            }));
        }

        // Popup quizzes that are due at the current position are answered from the platform's own
        // option marks, exactly like the reference learning driver does before a save.
        let due: Vec<Value> = prepared
            .due_questions()
            .into_iter()
            .map(|(_, ids)| ids.clone())
            .collect();
        for ids in due {
            if !live::answer_popup(host, &self.keys, session, course, unit, &ids)? {
                return Ok(json!({
                    "status": "needs_human",
                    "reason": "interaction_popup",
                    "checkpoint": prepared.to_checkpoint(),
                }));
            }
            prepared.questions.retain(|(_, keep)| keep != &ids);
        }

        let boundary = prepared.boundary();
        let chunk = (boundary - prepared.position).min(STUDY_CHUNK_SECONDS);
        if chunk <= 0 {
            // A theme/picture/knowledge-card overlay sits at this position; the reference stops.
            return Ok(json!({
                "status": "needs_human",
                "reason": "interaction_overlay",
                "checkpoint": prepared.to_checkpoint(),
            }));
        }

        live::save(host, &self.keys, session, &account_raw, course, &prepared, chunk)?;
        prepared.advance(chunk);

        if prepared.position >= prepared.duration {
            return Ok(json!({
                "status": "completed",
                "evidence": { "watched_seconds": prepared.position },
                "checkpoint": prepared.to_checkpoint(),
            }));
        }
        Ok(json!({
            "status": "in_progress",
            "progress": { "watched_seconds": prepared.position },
            "wait_ms": chunk * 1000,
            "checkpoint": prepared.to_checkpoint(),
        }))
    }
}

impl PlatformApi for Zhs {
    fn describe(&self) -> PlatformDescriptor {
        platform::manifest_platform()
    }

    fn auth_begin(
        &self,
        host: &dyn Host,
        session: &str,
        method: &str,
    ) -> Result<Value, ModuleError> {
        if method != "qr" {
            return Err(ModuleError::invalid_params(format!(
                "不支持的登录方式 {method}；本模块只声明了 qr"
            )));
        }
        match self.settings.source {
            Source::Fixture => {
                let now = host.now()?;
                Ok(session::begin_fixture(&self.challenges, session, now))
            }
            Source::Live => {
                self.keys.forget(session);
                session::begin_live(host, &self.challenges, session)
            }
        }
    }

    fn auth_poll(
        &self,
        host: &dyn Host,
        challenge: &str,
        session: &str,
    ) -> Result<Value, ModuleError> {
        match self.settings.source {
            Source::Fixture => {
                let now = host.now()?;
                Ok(session::poll_fixture(&self.challenges, challenge, now))
            }
            Source::Live => session::poll_live(host, &self.challenges, challenge, session),
        }
    }

    fn auth_complete(
        &self,
        host: &dyn Host,
        challenge: &str,
        session: &str,
    ) -> Result<Value, ModuleError> {
        match self.settings.source {
            Source::Fixture => Ok(session::complete_fixture()),
            Source::Live => session::complete_live(host, &self.challenges, challenge, session),
        }
    }

    fn auth_verify(&self, host: &dyn Host, session: &str) -> Result<Value, ModuleError> {
        match self.settings.source {
            Source::Fixture => Ok(session::verify_fixture()),
            Source::Live => session::verify_live(host, session),
        }
    }

    fn catalog_courses(&self, host: &dyn Host, session: &str) -> Result<Value, ModuleError> {
        match self.settings.source {
            Source::Fixture => catalog::fixture_courses(),
            Source::Live => {
                let (courses, entries) = live::courses(host, &self.keys, session)?;
                // The outline and study calls need the platform's opaque per-course `secret`;
                // `Course` has no native field by contract, so it is kept in module storage.
                host.state_set(CATALOG_NAMESPACE, ENTRIES_KEY, &entries, session)?;
                Ok(json!({ "courses": courses }))
            }
        }
    }

    fn catalog_units(
        &self,
        host: &dyn Host,
        session: &str,
        course: &Course,
    ) -> Result<Value, ModuleError> {
        match self.settings.source {
            Source::Fixture => catalog::fixture_units(course),
            Source::Live => {
                let entry = self.entry_for(host, session, course)?;
                let units = live::outline(host, &self.keys, session, course, &entry)?;
                let snapshot = serde_json::to_value(&units)
                    .map_err(|_| ModuleError::protocol_changed("目录无法序列化"))?;
                host.state_set(CATALOG_NAMESPACE, &units_key(course), &snapshot, session)?;
                Ok(json!({ "units": units }))
            }
        }
    }

    fn study_step(
        &self,
        host: &dyn Host,
        session: &str,
        course: &Course,
        unit: &Unit,
        checkpoint: Option<&Value>,
        _options: &Value,
    ) -> Result<Value, ModuleError> {
        match self.settings.source {
            Source::Fixture => study::fixture_step(unit, checkpoint),
            Source::Live => self.live_step(host, session, course, unit, checkpoint),
        }
    }
}

// --- shared HTTP/code helpers used by the live branches ---------------------------

pub(crate) fn fetch(host: &dyn Host, request: FetchRequest) -> Result<FetchResponse, ModuleError> {
    let url = request.url.clone();
    let response = host.fetch(&request)?;
    match response.status {
        401 => {
            return Err(ModuleError::login_expired(
                "智慧树登录已失效，请重新扫码；框架账号仍保持登录",
            ));
        }
        403 | 418 | 429 => {
            return Err(ModuleError::verification_required(
                "智慧树要求额外验证，请先到官方页面处理后重试",
            ));
        }
        200..=299 => {}
        status => {
            return Err(ModuleError::unavailable(format!(
                "智慧树返回 HTTP {status}：{url}"
            )));
        }
    }
    Ok(response)
}

pub(crate) fn fetch_json(host: &dyn Host, request: FetchRequest) -> Result<Value, ModuleError> {
    let url = request.url.clone();
    let response = fetch(host, request)?;
    serde_json::from_str(&response.body).map_err(|error| {
        ModuleError::protocol_changed(format!("智慧树响应不是合法 JSON（{url}）：{error}"))
    })
}

/// Check the platform `code` field. 401/403/5008 mean the framework session expired.
pub(crate) fn check_code(reply: &Value, accepted: &[i64]) -> Result<(), ModuleError> {
    match reply.get("code").and_then(number) {
        Some(code) if accepted.contains(&code) => Ok(()),
        Some(401 | 403 | 5008) => Err(ModuleError::login_expired(
            "智慧树登录已失效，请重新扫码；框架账号仍保持登录",
        )),
        Some(code) => Err(ModuleError::protocol_changed(format!(
            "智慧树返回了未预期的 code {code}"
        ))),
        None => Err(ModuleError::protocol_changed("智慧树响应缺少 code 字段")),
    }
}

/// The platform returns ids as numbers in some endpoints and strings in others.
pub(crate) fn id_string(value: &Value) -> Option<String> {
    value
        .as_str()
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
        .or_else(|| value.as_u64().map(|number| number.to_string()))
}

/// Numeric fields are sometimes numbers, sometimes numeric strings.
pub(crate) fn number(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_str().and_then(|text| text.parse::<i64>().ok()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn settings_default_to_live() {
        let (settings, warnings) = Settings::from_env(None);
        assert_eq!(settings.source, Source::Live);
        assert!(warnings.is_empty());
    }

    #[test]
    fn settings_parse_fixture_and_reject_unknown() {
        let (settings, warnings) = Settings::from_env(Some(r#"{"source":"fixture"}"#));
        assert_eq!(settings.source, Source::Fixture);
        assert!(warnings.is_empty());

        let (settings, warnings) = Settings::from_env(Some(r#"{"source":"weird"}"#));
        assert_eq!(settings.source, Source::Live);
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn ids_accept_numbers_and_strings() {
        assert_eq!(id_string(&json!(42)).as_deref(), Some("42"));
        assert_eq!(id_string(&json!("42")).as_deref(), Some("42"));
        assert_eq!(number(&json!("-1")), Some(-1));
    }
}
