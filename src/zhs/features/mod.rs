//! 智慧课程、作业考试、掌握度和课程活动的宿主适配。
//! 使用 MIT 授权的协议实现；HTTP、账号凭据、模型和浏览器仍由框架管理。
use super::{crypto, live, number};
use crate::{
    error::ModuleError,
    host::{Encoding, FetchRequest, Host, Stage},
};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
mod assessment;
mod mastery;
pub mod operations;
mod questions;
mod scores;
mod smart;
pub mod types;
use questions::{knowledge_points, merge_existing, parse_question, question_type, remote_id};
use types as providers;
use types::{Attempt, OptionItem, Paper, Question, Unit, WriteRequest};
type Result<T> = std::result::Result<T, ModuleError>;
const SMART: &str = "https://kg-ai-run.zhihuishu.com/run/gateway/t/stu";
const EXAM: &str = "https://newexam-api.zhihuishu.com/gateway/t/v1";
const HOMEWORK: &str = "https://studentexam-api.zhihuishu.com/studentExam/gateway/t/v1";
const FINAL: &str = "https://taurusexam-api.zhihuishu.com/taurusExam/gateway/t/v1";
const MASTERY: &str = "https://aistudy.zhihuishu.com/gateway/t/v1";

#[derive(Clone, Copy, PartialEq)]
enum PlatformOperation {
    Read,
    OpenStudy,
    SaveStudy,
}

pub struct FeatureClient<'a> {
    pub host: &'a dyn Host,
    pub keys: &'a live::Keys,
    pub session: &'a str,
    pub request: &'a str,
}

impl FeatureClient<'_> {
    fn milliseconds(&self) -> Result<i64> {
        Ok(self.host.now()?.saturating_mul(1000))
    }
    fn feature_request(
        &self,
        url: &str,
        payload: &Value,
        module: u8,
        method: &str,
        json_body: bool,
        operation: PlatformOperation,
    ) -> Result<Value> {
        let key = live::protocol_key(self.host, self.keys, self.session, module)?;
        let date = self.milliseconds()?;
        let mut input = payload.clone();
        let source = input.as_object_mut().and_then(|m| m.remove("__source"));
        if module != 3 {
            input["dateFormate"] = json!(date);
        }
        let mut body = json!({"secretStr":crypto::encode_query(&input, &key)?});
        if module != 3 {
            body["dateFormate"] = json!(date);
        }
        if let Some(source) = source {
            body["source"] = source;
        }
        let origin = if module == 3 || module == 5 {
            "https://onlineexamh5new.zhihuishu.com"
        } else if module == 6 || url.contains("aistudy") {
            "https://studywisdomh5.zhihuishu.com"
        } else {
            "https://onlineweb.zhihuishu.com"
        };
        let request = FetchRequest {
            session: self.session.into(),
            method: method.into(),
            url: url.into(),
            headers: [
                ("Origin".into(), origin.into()),
                ("Referer".into(), format!("{origin}/")),
            ]
            .into_iter()
            .collect(),
            body: Some(body),
            encoding: Some(if method == "GET" {
                Encoding::Query
            } else if json_body {
                Encoding::Json
            } else {
                Encoding::Form
            }),
            stage: Some(match operation {
                PlatformOperation::Read => Stage::Read,
                PlatformOperation::OpenStudy => Stage::OpenStudy,
                _ if url.ends_with("/submit") => Stage::Submit,
                _ => Stage::SaveStudy,
            }),
            commit: operation != PlatformOperation::Read,
        };
        let context = |mut error: ModuleError| {
            error.message = format!("{}；接口：{method} {url}", error.message);
            error
        };
        let response = crate::zhs::fetch(self.host, request).map_err(context)?;
        if response.body.trim().is_empty() && url.ends_with("/exam/user/submit") {
            return Ok(json!({"code":0,"http_acknowledged":true}));
        }
        let value: Value =
            serde_json::from_str(&response.body).map_err(|_| context(protocol_error()))?;
        feature_ok(&value, module == 3).map_err(context)?;
        Ok(value)
    }
    pub fn feature_write(&self, request: &WriteRequest) -> Result<Value> {
        let module = match request.encoding.as_str() {
            "zhs_shared_exam" => 3,
            "zhs_exam" => 5,
            "zhs_ai" => 6,
            "zhs_mastery" | "zhs_study" => 11,
            "json" => 0,
            _ => return Err(protocol_error()),
        };
        let reply = if module == 0 {
            let mut req =
                FetchRequest::post_json(self.session, &request.url, request.payload.clone());
            req.headers.insert(
                "Origin".into(),
                "https://studywisdomh5.zhihuishu.com".into(),
            );
            req.stage = Some(if request.stage == "submit" {
                Stage::Submit
            } else {
                Stage::SaveStudy
            });
            req.commit = true;
            crate::zhs::fetch_json(self.host, req)?
        } else {
            self.feature_request(
                &request.url,
                &request.payload,
                module,
                &request.method,
                module == 6,
                PlatformOperation::SaveStudy,
            )?
        };
        feature_ok(&reply, module == 3)?;
        if request.url.ends_with("saveStudentAnswer") || request.url.ends_with("/answer/submit") {
            match number(&reply["rt"]["statu"]) {
                Some(1) => {}
                Some(-10) => {
                    return Err(ModuleError::verification_required(
                        "智慧树要求作答验证，请先到官方页面处理",
                    ));
                }
                _ => return Err(protocol_error()),
            }
        }
        Ok(reply)
    }
}

fn protocol_error() -> ModuleError {
    ModuleError::protocol_changed("智慧树响应结构发生变化，请在官方页面核对")
}
fn login_expired() -> ModuleError {
    ModuleError::login_expired("智慧树登录已过期，请重新扫码")
}
fn feature_ok(value: &Value, shared_exam: bool) -> Result<()> {
    // 共享课作业考试以 status 判定成功，code 可能附带验证状态。
    let code = if shared_exam {
        value.get("status").and_then(number)
    } else {
        value
            .get("code")
            .and_then(number)
            .or_else(|| value.get("status").and_then(number))
    };
    let message = first(value, &["msg", "message", "errorMsg"])
        .chars()
        .take(400)
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>();
    let context = |mut error: ModuleError| {
        let mut details = vec![];
        for field in ["status", "code"] {
            if let Some(code) = value.get(field).and_then(number) {
                details.push(format!("{field}={code}"));
            }
        }
        if !message.is_empty() {
            details.push(format!("平台提示：{message}"));
        }
        if !details.is_empty() {
            error.message = format!("{}（{}）", error.message, details.join("，"));
        }
        error
    };
    if shared_exam && code.is_none() {
        return Err(context(ModuleError::protocol_changed(
            "智慧树作业考试响应缺少有效 status",
        )));
    }
    if matches!(code, Some(401 | 403 | 5008)) {
        return Err(context(login_expired()));
    }
    if code == Some(2004) {
        return Err(context(ModuleError::new(
            "no_questions",
            "此知识点暂无练习题",
        )));
    }
    if code == Some(2019) {
        return Err(context(ModuleError::rejected(
            "课程学习已结束，不能继续提升掌握度",
        )));
    }
    let rejected = code.is_some_and(|c| {
        if shared_exam {
            c != 200
        } else {
            ![0, 200].contains(&c)
        }
    }) || value["success"] == false;
    if code == Some(1010)
        || shared_exam && (code == Some(-12) || number(&value["code"]) == Some(-12))
        || rejected
            && ["验证", "滑块", "人脸"]
                .iter()
                .any(|hint| message.contains(hint))
    {
        return Err(context(ModuleError::verification_required(
            "平台要求额外验证，请在官方页面完成后继续",
        )));
    }
    if rejected {
        return Err(context(ModuleError::new(
            "platform_rejected",
            "智慧树拒绝本次操作，请核对开放状态、可用次数和验证提示",
        )));
    }
    Ok(())
}
pub fn text(value: &Value) -> String {
    value.as_str().map(str::to_owned).unwrap_or_else(|| {
        if value.is_null() {
            String::new()
        } else {
            value.to_string()
        }
    })
}
fn first(value: &Value, keys: &[&str]) -> String {
    keys.iter()
        .map(|k| text(&value[*k]))
        .find(|s| !s.is_empty())
        .unwrap_or_default()
}
fn component(value: &str) -> String {
    value
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
pub fn plain(html: &str) -> String {
    scraper::Html::parse_fragment(html)
        .root_element()
        .text()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}
pub fn rich_text(html: &str, pictures: &mut Vec<String>) -> String {
    let mut result = plain(html);
    let document = scraper::Html::parse_fragment(html);
    for img in document.select(&scraper::Selector::parse("img[src]").expect("静态选择器")) {
        if let Some(url) = img
            .value()
            .attr("src")
            .filter(|u| u.starts_with("https://"))
        {
            let index = if let Some(i) = pictures.iter().position(|s| s == url) {
                i
            } else {
                pictures.push(url.into());
                pictures.len() - 1
            };
            result.push_str(&format!(" [图 {}]", index + 1));
        }
    }
    result
}
fn timestamp(value: &Value) -> Option<i64> {
    if let Some(n) = number(value) {
        return Some(if n > 10_000_000_000 { n / 1000 } else { n });
    }
    let fields = value
        .as_str()?
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse::<i64>().ok())
        .collect::<Vec<_>>();
    if fields.len() < 3 {
        return None;
    }
    let (mut y, m, d) = (fields[0], fields[1], fields[2]);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    y -= i64::from(m <= 2);
    let era = y.div_euclid(400);
    let yo = y - era * 400;
    let mp = m + if m > 2 { -3 } else { 9 };
    let days = era * 146097 + yo * 365 + yo / 4 - yo / 100 + (153 * mp + 2) / 5 + d - 1 - 719468;
    Some(
        days * 86400
            + fields.get(3).copied().unwrap_or(0) * 3600
            + fields.get(4).copied().unwrap_or(0) * 60
            + fields.get(5).copied().unwrap_or(0)
            - 28800,
    )
}
mod net {
    pub use super::text;
    use super::*;
    pub fn decimal(v: &Value) -> Option<f64> {
        v.as_f64()
            .or_else(|| v.as_str()?.parse().ok())
            .filter(|n| n.is_finite())
    }
    pub fn data(v: &Value) -> Result<&Value> {
        v.get("data")
            .or_else(|| v.get("rt"))
            .or_else(|| v.get("result"))
            .filter(|v| !v.is_null())
            .ok_or_else(protocol_error)
    }
}

pub fn normalize_popup(raw: &Value) -> Result<types::Question> {
    questions::parse_question(raw, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_exam_uses_status_instead_of_the_auxiliary_code() {
        assert!(feature_ok(&json!({"status":"200","code":1,"rt":{}}), true).is_ok());
        let error =
            feature_ok(&json!({"status":"500","code":0,"msg":"试卷已截止"}), true).unwrap_err();
        assert_eq!(error.code, "platform_rejected");
        assert!(error.message.contains("status=500"));
        assert!(error.message.contains("试卷已截止"));
        assert!(feature_ok(&json!({"code":0,"rt":{}}), true).is_err());
        assert!(feature_ok(&json!({"code":0,"data":{}}), false).is_ok());
    }

    #[test]
    fn verification_keeps_the_platform_reason_and_is_never_treated_as_success() {
        for value in [
            json!({"status":200,"code":-12,"msg":"请完成验证"}),
            json!({"status":500,"msg":"请先完成滑块验证"}),
            json!({"code":1010,"message":"请完成人脸验证"}),
        ] {
            let error = feature_ok(&value, value.get("status").is_some()).unwrap_err();
            assert_eq!(error.code, "platform_verification_required");
            assert!(error.message.contains("平台提示："));
        }
        let error = feature_ok(&json!({"status":401,"msg":"登录已过期"}), true).unwrap_err();
        assert_eq!(error.code, "platform_login_expired");
        assert!(error.message.contains("status=401"));
    }
}
