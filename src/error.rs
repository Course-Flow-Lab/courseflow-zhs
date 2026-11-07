//! Typed module errors: the wire shape is `{"code": "<stable string>", "message": "..."}`
//! (module API v1 §6). Codes are strings, never integers.

use serde::Serialize;
use serde_json::{json, Value};

pub const UNSUPPORTED_OPERATION: &str = "unsupported_operation";
pub const INVALID_PARAMS: &str = "invalid_params";
pub const PLATFORM_LOGIN_EXPIRED: &str = "platform_login_expired";
pub const PLATFORM_VERIFICATION_REQUIRED: &str = "platform_verification_required";
pub const PLATFORM_PROTOCOL_CHANGED: &str = "platform_protocol_changed";
pub const PLATFORM_REJECTED: &str = "platform_rejected";
pub const PLATFORM_UNAVAILABLE: &str = "platform_unavailable";
pub const NOT_FOUND: &str = "not_found";

/// Framework-generated codes (never produced by this module, except when they are
/// passed straight through from a host call, which is what the caller asked for).
const ALL_CODES: &[&str] = &[
    UNSUPPORTED_OPERATION,
    INVALID_PARAMS,
    PLATFORM_LOGIN_EXPIRED,
    PLATFORM_VERIFICATION_REQUIRED,
    PLATFORM_PROTOCOL_CHANGED,
    PLATFORM_REJECTED,
    PLATFORM_UNAVAILABLE,
    NOT_FOUND,
    "module_timeout",
    "module_crashed",
    "module_unavailable",
    "module_version_changed",
    "net_blocked",
    "quiet_hours",
];

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ModuleError {
    pub code: &'static str,
    pub message: String,
}

impl ModuleError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self::new(INVALID_PARAMS, message)
    }

    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::new(UNSUPPORTED_OPERATION, message)
    }

    pub fn login_expired(message: impl Into<String>) -> Self {
        Self::new(PLATFORM_LOGIN_EXPIRED, message)
    }

    pub fn verification_required(message: impl Into<String>) -> Self {
        Self::new(PLATFORM_VERIFICATION_REQUIRED, message)
    }

    pub fn protocol_changed(message: impl Into<String>) -> Self {
        Self::new(PLATFORM_PROTOCOL_CHANGED, message)
    }

    pub fn rejected(message: impl Into<String>) -> Self {
        Self::new(PLATFORM_REJECTED, message)
    }

    pub fn unavailable(message: impl Into<String>) -> Self {
        Self::new(PLATFORM_UNAVAILABLE, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(NOT_FOUND, message)
    }

    /// Error object exactly as it appears inside a JSON-RPC error response.
    pub fn to_json(&self) -> Value {
        json!({ "code": self.code, "message": self.message })
    }

    /// Intern a code that arrived over the wire so it can be echoed straight back.
    /// Any module API v1 code is kept verbatim; unknown codes degrade to
    /// `platform_unavailable` with the original code preserved in the message.
    pub fn from_wire(code: Option<&str>, message: &str) -> Self {
        let message = if message.trim().is_empty() {
            "宿主调用失败，且未提供说明".to_owned()
        } else {
            message.to_owned()
        };
        match code.and_then(intern_code) {
            Some(code) => Self::new(code, message),
            None => {
                let detail = match code {
                    Some(raw) => {
                        format!("{message}（宿主返回了 module api v1 未定义的错误码 {raw}）")
                    }
                    None => format!("{message}（宿主响应缺少错误码）"),
                };
                Self::new(PLATFORM_UNAVAILABLE, detail)
            }
        }
    }
}

pub fn intern_code(code: &str) -> Option<&'static str> {
    ALL_CODES.iter().copied().find(|known| *known == code)
}

impl std::fmt::Display for ModuleError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ModuleError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_serialize_with_a_string_code() {
        let error = ModuleError::invalid_params("缺少 session");
        let value = serde_json::to_value(&error).unwrap();
        assert_eq!(value["code"], "invalid_params");
        assert_eq!(value["code"].as_str().is_some(), true);
        assert_eq!(value["message"], "缺少 session");
    }

    #[test]
    fn known_host_codes_pass_through_verbatim() {
        let error = ModuleError::from_wire(Some("platform_verification_required"), "需要验证");
        assert_eq!(error.code, "platform_verification_required");
        assert_eq!(error.message, "需要验证");
    }

    #[test]
    fn unknown_host_codes_become_platform_unavailable() {
        let error = ModuleError::from_wire(Some("surprise"), "坏了");
        assert_eq!(error.code, "platform_unavailable");
        assert!(error.message.contains("surprise"));
        assert!(error.message.contains("坏了"));
    }
}
