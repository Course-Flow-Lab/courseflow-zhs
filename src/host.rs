//! Typed wrapper over the `cw.*` framework services (module API v1 §4), plus the
//! `Host` trait used by tests to substitute an in-memory host.
//!
//! A module never opens a socket, never reads a cookie and never keeps a credential.
//! Everything that touches the outside world goes through these calls.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::error::ModuleError;
use crate::rpc::RpcClient;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Encoding {
    #[default]
    Raw,
    Json,
    Form,
    Query,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    #[default]
    Read,
    OpenStudy,
    SaveStudy,
    Submit,
}

/// One `cw.net.fetch` request. Optional fields are omitted from the wire so the
/// framework's defaults (`raw`, `read`, `commit: false`) apply.
#[derive(Clone, Debug, Serialize)]
pub struct FetchRequest {
    pub session: String,
    pub method: String,
    pub url: String,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encoding: Option<Encoding>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stage: Option<Stage>,
    #[serde(skip_serializing_if = "is_false")]
    pub commit: bool,
}

impl FetchRequest {
    pub fn get(session: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            session: session.into(),
            method: "GET".to_owned(),
            url: url.into(),
            headers: BTreeMap::new(),
            body: None,
            encoding: None,
            stage: None,
            commit: false,
        }
    }

    pub fn post_json(session: impl Into<String>, url: impl Into<String>, body: Value) -> Self {
        Self {
            session: session.into(),
            method: "POST".to_owned(),
            url: url.into(),
            headers: BTreeMap::new(),
            body: Some(body),
            encoding: Some(Encoding::Json),
            stage: Some(Stage::Read),
            commit: false,
        }
    }

    pub fn with_headers(mut self, headers: impl IntoIterator<Item = (String, String)>) -> Self {
        self.headers.extend(headers);
        self
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// A `cw.net.fetch` result. `body` is always a UTF-8 string; JSON is parsed by us.
#[derive(Clone, Debug, Deserialize)]
pub struct FetchResponse {
    pub status: u16,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub body: String,
}

/// The only outside world a platform implementation is allowed to touch.
///
/// Tests implement this trait with an in-memory mock (no network, no account).
pub trait Host: Send + Sync {
    /// 扩展宿主服务；模拟宿主可以只实现本次测试使用的方法。
    fn service(&self, method: &str, _params: Value) -> Result<Value, ModuleError> {
        Err(ModuleError::unsupported(format!("宿主尚未提供 {method}")))
    }
    fn fetch(&self, request: &FetchRequest) -> Result<FetchResponse, ModuleError>;

    fn state_get(&self, namespace: &str, key: &str, session: &str) -> Result<Value, ModuleError>;

    fn state_set(
        &self,
        namespace: &str,
        key: &str,
        value: &Value,
        session: &str,
    ) -> Result<(), ModuleError>;

    fn now(&self) -> Result<i64, ModuleError>;

    /// `cw.session.identity`: the platform identity the framework stored at auth.complete.
    /// Default is empty so a mock host needs no change.
    fn identity(&self, _session: &str) -> Result<Value, ModuleError> {
        Ok(Value::Null)
    }

    /// `cw.log` is a notification: it never receives a response. Default is a no-op
    /// so audio-less test hosts do not have to implement it.
    fn log(&self, level: &str, message: &str, fields: Value) {
        let _ = (level, message, fields);
    }
}

/// Real host: turns every `Host` method into a multiplexed `cw.*` RPC call.
pub struct CwHost {
    client: Arc<RpcClient>,
}

impl CwHost {
    pub fn new(client: Arc<RpcClient>) -> Self {
        Self { client }
    }
}

impl Host for CwHost {
    fn service(&self, method: &str, params: Value) -> Result<Value, ModuleError> {
        self.client.call(method, params)
    }
    fn fetch(&self, request: &FetchRequest) -> Result<FetchResponse, ModuleError> {
        let params = serde_json::to_value(request).map_err(|error| {
            ModuleError::protocol_changed(format!("无法序列化 cw.net.fetch 参数：{error}"))
        })?;
        let value = self.client.call("cw.net.fetch", params)?;
        serde_json::from_value(value).map_err(|error| {
            ModuleError::protocol_changed(format!("cw.net.fetch 响应格式不正确：{error}"))
        })
    }

    fn state_get(&self, namespace: &str, key: &str, session: &str) -> Result<Value, ModuleError> {
        let value = self.client.call(
            "cw.state",
            json!({ "namespace": namespace, "key": key, "session": session }),
        )?;
        Ok(value.get("value").cloned().unwrap_or(Value::Null))
    }

    fn state_set(
        &self,
        namespace: &str,
        key: &str,
        value: &Value,
        session: &str,
    ) -> Result<(), ModuleError> {
        self.client.call(
            "cw.state",
            json!({
                "namespace": namespace,
                "key": key,
                "value": value,
                "session": session,
            }),
        )?;
        Ok(())
    }

    fn now(&self) -> Result<i64, ModuleError> {
        let value = self.client.call("cw.clock.now", json!({}))?;
        value
            .get("now")
            .and_then(Value::as_i64)
            .ok_or_else(|| ModuleError::protocol_changed("cw.clock.now 未返回整数 now"))
    }

    fn identity(&self, session: &str) -> Result<Value, ModuleError> {
        self.client
            .call("cw.session.identity", json!({ "session": session }))
    }

    fn log(&self, level: &str, message: &str, fields: Value) {
        let mut params = Map::new();
        params.insert("level".to_owned(), Value::from(level));
        params.insert("message".to_owned(), Value::from(message));
        if !fields.is_null() {
            params.insert("fields".to_owned(), fields);
        }
        self.client.notify("cw.log", Value::Object(params));
    }
}
