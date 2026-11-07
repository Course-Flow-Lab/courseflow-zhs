//! `platform.describe`, the method dispatch table, and the `PlatformApi` boundary.
//!
//! The descriptor is authoritative at runtime (module API §3.1) and must match
//! `courseflow.module.json`; the contract test parses the manifest and compares it.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::error::ModuleError;
use crate::host::Host;
use crate::zhs::{Course, Unit};

pub const MODULE_NAME: &str = env!("CARGO_PKG_NAME");
pub const MODULE_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const MODULE_API: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlatformDescriptor {
    pub id: String,
    pub name: String,
    pub short: String,
    pub login: Vec<String>,
    pub operations: Vec<String>,
    pub unit_kinds: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_speed: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keepalive_seconds: Option<u64>,
    pub presentation: Presentation,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Presentation {
    pub tabs: Vec<PresentationItem>,
    pub actions: Vec<PresentationItem>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PresentationItem {
    pub id: String,
    pub label: String,
    /// Evaluated by the framework, never by the module (module API §1).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub when: BTreeMap<String, String>,
}

/// The single source of truth for the runtime descriptor. Must equal the `platform`
/// object in `courseflow.module.json`; `tests/contract.rs` enforces that.
pub fn manifest_platform() -> PlatformDescriptor {
    let manifest: Value = serde_json::from_str(include_str!("../courseflow.module.json"))
        .expect("构建时模块清单应合法");
    serde_json::from_value(manifest["platform"].clone()).expect("模块平台描述应合法")
}

pub trait PlatformApi: Send + Sync {
    fn extension(
        &self,
        _host: &dyn Host,
        method: &str,
        _params: &Value,
    ) -> Result<Value, ModuleError> {
        Err(ModuleError::unsupported(format!("模块未提供 {method}")))
    }
    fn describe(&self) -> PlatformDescriptor;

    fn auth_begin(
        &self,
        host: &dyn Host,
        session: &str,
        method: &str,
    ) -> Result<Value, ModuleError>;

    fn auth_poll(
        &self,
        host: &dyn Host,
        challenge: &str,
        session: &str,
    ) -> Result<Value, ModuleError>;

    fn auth_complete(
        &self,
        host: &dyn Host,
        challenge: &str,
        session: &str,
    ) -> Result<Value, ModuleError>;

    fn auth_verify(&self, host: &dyn Host, session: &str) -> Result<Value, ModuleError>;

    fn catalog_courses(&self, host: &dyn Host, session: &str) -> Result<Value, ModuleError>;

    fn catalog_units(
        &self,
        host: &dyn Host,
        session: &str,
        course: &Course,
    ) -> Result<Value, ModuleError>;

    fn study_step(
        &self,
        host: &dyn Host,
        session: &str,
        course: &Course,
        unit: &Unit,
        checkpoint: Option<&Value>,
        options: &Value,
    ) -> Result<Value, ModuleError>;
}

pub fn describe(api: &dyn PlatformApi) -> Value {
    json!({
        "module": {
            "name": MODULE_NAME,
            "version": MODULE_VERSION,
            "module_api": MODULE_API,
        },
        "platform": api.describe(),
    })
}

pub fn dispatch(
    api: &dyn PlatformApi,
    host: &dyn Host,
    method: &str,
    params: &Value,
) -> Result<Value, ModuleError> {
    let empty = Value::Object(Map::new());
    let params = match params {
        Value::Null => &empty,
        Value::Object(_) => params,
        _ => return Err(ModuleError::invalid_params("params 必须是 JSON 对象")),
    };

    match method {
        "platform.describe" => {
            check_module_api(params)?;
            Ok(describe(api))
        }
        "auth.begin" => {
            let session = required_str(params, "session")?;
            let login = match params.get("method") {
                None | Some(Value::Null) => "qr",
                Some(Value::String(login)) => login.as_str(),
                Some(_) => {
                    return Err(ModuleError::invalid_params("参数 method 必须是字符串"));
                }
            };
            api.auth_begin(host, session, login)
        }
        "auth.poll" => {
            let challenge = required_any_str(params, &["challenge", "session"])?;
            let session = optional_session(params).unwrap_or(challenge);
            api.auth_poll(host, challenge, session)
        }
        "auth.complete" => {
            let challenge = required_any_str(params, &["challenge", "session"])?;
            let session = optional_session(params).unwrap_or(challenge);
            api.auth_complete(host, challenge, session)
        }
        "auth.verify" => {
            let session = required_str(params, "session")?;
            api.auth_verify(host, session)
        }
        "catalog.courses" => {
            let session = required_str(params, "session")?;
            api.catalog_courses(host, session)
        }
        "catalog.units" => {
            let session = required_str(params, "session")?;
            let course = parse_course(params)?;
            api.catalog_units(host, session, &course)
        }
        "study.step" => {
            let session = required_str(params, "session")?;
            let course = parse_course(params)?;
            let unit = parse_unit(params)?;
            let checkpoint = params.get("checkpoint").filter(|value| !value.is_null());
            let options = params.get("options").cloned().unwrap_or_else(|| json!({}));
            if !options.is_object() {
                return Err(ModuleError::invalid_params("参数 options 必须是 JSON 对象"));
            }
            api.study_step(host, session, &course, &unit, checkpoint, &options)
        }
        other => api.extension(host, other, params),
    }
}

fn check_module_api(params: &Value) -> Result<(), ModuleError> {
    if let Some(version) = params.get("module_api") {
        if version.as_u64() != Some(MODULE_API as u64) {
            return Err(ModuleError::invalid_params(format!(
                "不支持的 module_api {version}；本模块实现 module_api {MODULE_API}"
            )));
        }
    }
    Ok(())
}

fn parse_course(params: &Value) -> Result<Course, ModuleError> {
    let value = params
        .get("course")
        .ok_or_else(|| ModuleError::invalid_params("缺少 course 参数"))?;
    if !value.is_object() {
        return Err(ModuleError::invalid_params("参数 course 必须是对象"));
    }
    serde_json::from_value(value.clone())
        .map_err(|error| ModuleError::invalid_params(format!("course 参数不合法：{error}")))
}

fn parse_unit(params: &Value) -> Result<Unit, ModuleError> {
    let value = params
        .get("unit")
        .ok_or_else(|| ModuleError::invalid_params("缺少 unit 参数"))?;
    if !value.is_object() {
        return Err(ModuleError::invalid_params("参数 unit 必须是对象"));
    }
    serde_json::from_value(value.clone())
        .map_err(|error| ModuleError::invalid_params(format!("unit 参数不合法：{error}")))
}

fn required_str<'a>(params: &'a Value, key: &str) -> Result<&'a str, ModuleError> {
    match params.get(key).and_then(Value::as_str) {
        Some(value) if !value.trim().is_empty() => Ok(value),
        Some(_) => Err(ModuleError::invalid_params(format!("参数 {key} 不能为空"))),
        None => Err(ModuleError::invalid_params(format!("缺少字符串参数 {key}"))),
    }
}

fn required_any_str<'a>(params: &'a Value, keys: &[&str]) -> Result<&'a str, ModuleError> {
    for key in keys {
        if params.get(*key).is_some() {
            return required_str(params, key);
        }
    }
    Err(ModuleError::invalid_params(format!(
        "缺少参数 {}（任选其一）",
        keys.join(" 或 ")
    )))
}

fn optional_session<'a>(params: &'a Value) -> Option<&'a str> {
    params
        .get("session")
        .and_then(Value::as_str)
        .filter(|session| !session.trim().is_empty())
}
