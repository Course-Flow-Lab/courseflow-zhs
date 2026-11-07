//! Course and unit mapping.
//!
//! Fixture mode parses the recorded platform snapshots embedded at compile time:
//! `fixtures/courses.json` (share-course + AI-course list) and `fixtures/units.json`
//! (shared-course outline). Live mode only implements the first course-list call in
//! this draft; everything else is a typed `unsupported_operation`.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::ModuleError;

use super::id_string;

pub const COURSES_FIXTURE: &str = include_str!("../../fixtures/courses.json");
pub const UNITS_FIXTURE: &str = include_str!("../../fixtures/units.json");

/// Wire type from module API v1 §5. Field names are snake_case on purpose.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Course {
    pub id: String,
    pub recruit_id: String,
    pub name: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family: Option<String>,
    #[serde(default)]
    pub progress: Option<f64>,
    pub outline_available: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnitState {
    NotStarted,
    InProgress,
    Completed,
    Unknown,
}

/// Wire type from module API v1 §5. There is deliberately no `native` field.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Unit {
    pub id: String,
    pub name: String,
    pub chapter_id: String,
    pub chapter_name: String,
    pub lesson_id: String,
    pub kind: String,
    #[serde(default)]
    pub duration_seconds: Option<i64>,
    #[serde(default)]
    pub watched_seconds: Option<i64>,
    pub state: UnitState,
    #[serde(default)]
    pub video_id: Option<String>,
    pub selectable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selectable_note: Option<String>,
}

// --- fixture ----------------------------------------------------------------------

pub fn fixture_courses() -> Result<Value, ModuleError> {
    let raw = load_fixture(COURSES_FIXTURE, "fixtures/courses.json")?;
    let shared = raw
        .pointer("/shared/result/courseOpenDtos")
        .and_then(Value::as_array)
        .ok_or_else(|| fixture_shape("/shared/result/courseOpenDtos"))?;
    let smart = raw
        .pointer("/smart/rt")
        .and_then(Value::as_array)
        .ok_or_else(|| fixture_shape("/smart/rt"))?;

    let mut courses = Vec::with_capacity(shared.len() + smart.len());
    for row in shared {
        courses.push(shared_course(row)?);
    }
    for row in smart {
        courses.push(smart_course(row)?);
    }
    Ok(json!({ "courses": courses }))
}

pub fn fixture_units(_course: &Course) -> Result<Value, ModuleError> {
    let raw = load_fixture(UNITS_FIXTURE, "fixtures/units.json")?;
    let chapters = raw
        .get("chapters")
        .and_then(Value::as_array)
        .ok_or_else(|| fixture_shape("/chapters"))?;

    let mut units = Vec::new();
    for chapter in chapters {
        let chapter_id = id_string(&chapter["id"]).ok_or_else(|| fixture_shape("chapters[].id"))?;
        let chapter_name = display_name(chapter, "未命名章节");
        let lessons = chapter
            .get("lessons")
            .and_then(Value::as_array)
            .ok_or_else(|| fixture_shape("chapters[].lessons"))?;
        for lesson in lessons {
            let lesson_id =
                id_string(&lesson["id"]).ok_or_else(|| fixture_shape("lessons[].id"))?;
            let lesson_name = display_name(lesson, "未命名小节");
            let items = lesson
                .get("units")
                .and_then(Value::as_array)
                .ok_or_else(|| fixture_shape("lessons[].units"))?;
            for item in items {
                units.push(unit(
                    item,
                    &chapter_id,
                    &chapter_name,
                    &lesson_id,
                    &lesson_name,
                )?);
            }
        }
    }
    Ok(json!({ "units": units }))
}

fn shared_course(row: &Value) -> Result<Course, ModuleError> {
    let id = id_string(&row["courseId"])
        .or_else(|| id_string(&row["id"]))
        .ok_or_else(|| platform_shape("共享课缺少 courseId"))?;
    let recruit_id =
        id_string(&row["recruitId"]).ok_or_else(|| platform_shape("共享课缺少 recruitId"))?;
    let name = row["courseName"]
        .as_str()
        .filter(|name| !name.trim().is_empty())
        .ok_or_else(|| platform_shape("共享课缺少 courseName"))?
        .to_owned();
    Ok(Course {
        id,
        recruit_id,
        name,
        kind: "shared".to_owned(),
        kind_label: Some("共享课".to_owned()),
        family: Some("shared".to_owned()),
        progress: percent(&row["studyPercent"]),
        outline_available: row
            .get("secret")
            .and_then(Value::as_str)
            .is_some_and(|secret| !secret.is_empty()),
    })
}

fn smart_course(row: &Value) -> Result<Course, ModuleError> {
    let id = id_string(&row["courseId"]).ok_or_else(|| platform_shape("智慧课缺少 courseId"))?;
    let recruit_id =
        id_string(&row["classId"]).ok_or_else(|| platform_shape("智慧课缺少 classId"))?;
    let name = row
        .get("courseName")
        .or_else(|| row.get("name"))
        .and_then(Value::as_str)
        .filter(|name| !name.trim().is_empty())
        .ok_or_else(|| platform_shape("智慧课缺少 courseName"))?
        .to_owned();
    Ok(Course {
        id,
        recruit_id,
        name,
        kind: "smart".to_owned(),
        kind_label: Some("AI课".to_owned()),
        family: None,
        progress: percent(&row["schedule"]),
        // The v1 fixture deliberately reports no outline for the smart course:
        // the AI knowledge-outline handshake (protocol module 6) is not ported yet.
        outline_available: false,
    })
}

fn unit(
    item: &Value,
    chapter_id: &str,
    chapter_name: &str,
    lesson_id: &str,
    lesson_name: &str,
) -> Result<Unit, ModuleError> {
    let id = id_string(&item["id"]).ok_or_else(|| fixture_shape("units[].id"))?;
    let kind = item
        .get("kind")
        .and_then(Value::as_str)
        .filter(|kind| !kind.trim().is_empty())
        .ok_or_else(|| fixture_shape("units[].kind"))?
        .to_owned();
    let duration_seconds = item.get("duration_seconds").and_then(Value::as_i64);
    let watched_seconds = item.get("watched_seconds").and_then(Value::as_i64);
    let state =
        serde_json::from_value::<UnitState>(item.get("state").cloned().unwrap_or(Value::Null))
            .map_err(|_| {
                ModuleError::protocol_changed(format!("fixture 单元 {id} 的 state 不是已知状态"))
            })?;
    let selectable = duration_seconds.is_some_and(|seconds| seconds > 0);
    let selectable_note = if selectable {
        None
    } else {
        Some(match kind.as_str() {
            "document" => "文档单元没有可自动学习的时长，请在智慧树官网阅读".to_owned(),
            _ => "该单元没有可自动学习的时长，无法自动学习".to_owned(),
        })
    };
    Ok(Unit {
        id,
        name: display_name(item, lesson_name),
        chapter_id: chapter_id.to_owned(),
        chapter_name: chapter_name.to_owned(),
        lesson_id: lesson_id.to_owned(),
        kind,
        duration_seconds,
        watched_seconds,
        state,
        video_id: item
            .get("video_id")
            .and_then(Value::as_str)
            .map(str::to_owned),
        selectable,
        selectable_note,
    })
}

// --- live -------------------------------------------------------------------------

/// One real `queryShareCourseInfo` POST through `cw.net.fetch`.
///
/// Note: the reference client signs the form body with a per-module AES key obtained
/// by RSA (`shared_courses` in `courseweave/src/zhs.rs`). This draft has no crypto
/// dependency, so it sends the documented JSON `fromurl` payload unsigned; see the
/// README port checklist. Any framework error code is propagated verbatim.

// --- helpers ----------------------------------------------------------------------

fn load_fixture(text: &str, name: &str) -> Result<Value, ModuleError> {
    serde_json::from_str(text)
        .map_err(|error| ModuleError::protocol_changed(format!("{name} 无法解析：{error}")))
}

fn fixture_shape(pointer: &str) -> ModuleError {
    ModuleError::protocol_changed(format!("fixture 缺少数组 {pointer}"))
}

fn platform_shape(message: &str) -> ModuleError {
    ModuleError::protocol_changed(format!("智慧树课程数据不完整：{message}"))
}

fn display_name(value: &Value, fallback: &str) -> String {
    value["name"]
        .as_str()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or(fallback)
        .to_owned()
}

fn percent(value: &Value) -> Option<f64> {
    let number = value
        .as_f64()
        .or_else(|| value.as_str()?.parse::<f64>().ok())?;
    (number.is_finite() && (0.0..=100.0).contains(&number)).then_some(number)
}
