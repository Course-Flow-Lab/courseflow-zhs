// 平台私有记录，仅保存在宿主加密状态中。
use super::{first, number, text};
pub use super::{plain, rich_text};
use serde::{Deserialize, Serialize};
use serde_json::Value;
pub fn unavailable(message: &str) -> crate::error::ModuleError {
    crate::error::ModuleError::new("platform_rejected", message)
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Course {
    pub id: String,
    pub recruit_id: String,
    pub name: String,
    pub kind: String,
    pub progress: Option<f64>,
    pub outline_available: bool,
    #[serde(default)]
    pub native: Value,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Unit {
    pub id: String,
    pub name: String,
    pub chapter_id: String,
    pub chapter_name: String,
    pub lesson_id: String,
    pub kind: String,
    pub duration_seconds: Option<i64>,
    pub watched_seconds: Option<i64>,
    pub state: String,
    pub video_id: Option<String>,
    #[serde(default)]
    pub native: Value,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Paper {
    pub id: String,
    pub name: String,
    pub category: String,
    pub available: bool,
    pub done: bool,
    #[serde(default)]
    pub can_redo: bool,
    pub reason: String,
    pub status_label: String,
    pub score: Option<f64>,
    pub attempts_left: Option<i64>,
    pub opens_at: Option<i64>,
    pub ends_at: Option<i64>,
    #[serde(default)]
    pub native: Value,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct OptionItem {
    pub id: String,
    pub text: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Question {
    pub id: String,
    pub text: String,
    pub kind: String,
    pub options: Vec<OptionItem>,
    pub answer: Vec<String>,
    #[serde(default)]
    pub correct_answer: Vec<String>,
    #[serde(default)]
    pub explanation: String,
    #[serde(default)]
    pub images: Vec<String>,
    #[serde(default)]
    pub blanks: usize,
    #[serde(default)]
    pub native: Value,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Attempt {
    pub name: String,
    pub id: String,
    pub paper_id: String,
    pub questions: Vec<Question>,
    pub expires_at: Option<i64>,
    pub native: Value,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct WriteRequest {
    pub url: String,
    pub method: String,
    pub encoding: String,
    pub payload: Value,
    pub stage: String,
}
pub fn validate_answer(q: &Question, answer: &[String]) -> bool {
    if answer
        .iter()
        .any(|s| s.trim().is_empty() || s.len() > 20000)
    {
        return false;
    }
    match q.kind.as_str() {
        "single" => answer.len() == 1 && q.options.iter().any(|o| o.id == answer[0]),
        "multiple" => {
            !answer.is_empty()
                && answer.iter().all(|a| q.options.iter().any(|o| &o.id == a))
                && answer
                    .iter()
                    .collect::<std::collections::HashSet<_>>()
                    .len()
                    == answer.len()
        }
        "judge" => answer.len() == 1 && ["true", "false"].contains(&answer[0].as_str()),
        "blank" => q.blanks > 0 && answer.len() == q.blanks,
        "essay" => answer.len() == 1,
        _ => false,
    }
}
pub fn answer_strings(kind: &str, value: &Value) -> Vec<String> {
    if value.is_null() {
        return vec![];
    }
    if let Some(values) = value.as_array() {
        return values.iter().map(text).collect();
    }
    let s = text(value);
    let s = s.trim();
    if s.is_empty() {
        return vec![];
    }
    if s.starts_with('[')
        && let Ok(values) = serde_json::from_str::<Vec<Value>>(s)
    {
        return values.iter().map(text).collect();
    }
    if kind == "multiple" {
        return s
            .replace("#@#", ",")
            .split([',', '，', ';'])
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .collect();
    }
    if kind == "judge" {
        return vec![
            match s {
                "1" | "true" | "正确" | "对" => "true",
                "0" | "false" | "错误" | "错" => "false",
                _ => s,
            }
            .to_owned(),
        ];
    }
    vec![s.to_owned()]
}
pub fn attach_review(question: &mut Question, raw: &Value, standard_answer_field: bool) {
    let explicit = raw.get("correctAnswer").or_else(|| raw.get("rightAnswer"));
    let provided = explicit.or_else(|| {
        if standard_answer_field {
            raw.get("answer").filter(|v| v.is_string() || v.is_array())
        } else {
            None
        }
    });
    let mut answer = provided
        .map(|v| answer_strings(&question.kind, v))
        .unwrap_or_default();
    if answer.is_empty() {
        let options = raw
            .get("questionOptions")
            .or_else(|| raw.get("optionVos"))
            .or_else(|| raw.get("dataArr"))
            .or_else(|| raw.get("dataJson"))
            .cloned()
            .unwrap_or(Value::Null);
        let options = if let Some(encoded) = options.as_str() {
            serde_json::from_str::<Value>(encoded).unwrap_or(Value::Null)
        } else {
            options
        };
        answer = options
            .as_array()
            .into_iter()
            .flatten()
            .filter(|o| {
                [&o["IsAnswer"], &o["isAnswer"]]
                    .iter()
                    .any(|v| **v == true || v.as_str() == Some("true"))
                    || number(&o["result"]) == Some(1)
            })
            .map(|o| first(o, &["id", "optionId", "SortOrder"]))
            .collect();
    }
    if !validate_answer(question, &answer)
        && ["single", "multiple"].contains(&question.kind.as_str())
    {
        let raw = answer.join("");
        if !raw.is_empty()
            && raw
                .bytes()
                .all(|b| b.is_ascii_uppercase() && ((b - b'A') as usize) < question.options.len())
        {
            answer = raw
                .bytes()
                .map(|b| question.options[(b - b'A') as usize].id.clone())
                .collect();
        }
    }
    if validate_answer(question, &answer) {
        question.correct_answer = answer;
    }
    if let Some(explanation) = raw
        .get("analysis")
        .or_else(|| raw.get("analyse"))
        .or_else(|| raw.get("explanation"))
        .and_then(Value::as_str)
    {
        question.explanation = plain(explanation);
    }
}
