//! Fixture `study.step`: the real checkpoint semantics without any network.
//!
//! `checkpoint` is opaque to the framework and stored per job. This draft defines it
//! as `{"seconds": n}`: the server-reported position that the next step starts from.
//! Each step advances 60 s, capped at the unit duration. A unit whose duration is 0
//! completes on the first step. The function is pure: replaying the same checkpoint
//! returns the same next state, which makes it safe to re-invoke after a restart.

use serde_json::{json, Value};

use crate::error::ModuleError;

use super::catalog::Unit;

pub const STEP_SECONDS: i64 = 60;
pub const WAIT_MS: u64 = 1000;

pub fn fixture_step(unit: &Unit, checkpoint: Option<&Value>) -> Result<Value, ModuleError> {
    let duration = unit.duration_seconds.unwrap_or(0).max(0);
    let start = read_checkpoint(checkpoint)?.clamp(0, duration);
    let watched = start.saturating_add(STEP_SECONDS).min(duration);

    if watched >= duration {
        Ok(json!({
            "status": "completed",
            "evidence": { "watched_seconds": watched },
        }))
    } else {
        Ok(json!({
            "status": "in_progress",
            "progress": { "watched_seconds": watched },
            "wait_ms": WAIT_MS,
            "checkpoint": { "seconds": watched },
        }))
    }
}

fn read_checkpoint(checkpoint: Option<&Value>) -> Result<i64, ModuleError> {
    match checkpoint {
        None | Some(Value::Null) => Ok(0),
        Some(Value::Object(map)) => match map.get("seconds") {
            None | Some(Value::Null) => Ok(0),
            Some(Value::Number(number)) => number
                .as_i64()
                .ok_or_else(|| ModuleError::invalid_params("checkpoint.seconds 必须是整数")),
            Some(_) => Err(ModuleError::invalid_params("checkpoint.seconds 必须是整数")),
        },
        Some(_) => Err(ModuleError::invalid_params(
            "checkpoint 必须是 JSON 对象或 null",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zhs::catalog::{Unit, UnitState};

    fn unit(duration_seconds: Option<i64>) -> Unit {
        Unit {
            id: "u1".to_owned(),
            name: "视频".to_owned(),
            chapter_id: "c1".to_owned(),
            chapter_name: "第一章".to_owned(),
            lesson_id: "l1".to_owned(),
            kind: "video".to_owned(),
            duration_seconds,
            watched_seconds: None,
            state: UnitState::NotStarted,
            video_id: None,
            selectable: true,
            selectable_note: None,
        }
    }

    #[test]
    fn steps_are_monotonic_and_idempotent() {
        let unit = unit(Some(180));
        let first = fixture_step(&unit, None).unwrap();
        assert_eq!(first["status"], "in_progress");
        assert_eq!(first["progress"]["watched_seconds"], 60);

        let checkpoint = first["checkpoint"].clone();
        let replay = fixture_step(&unit, Some(&checkpoint)).unwrap();
        let again = fixture_step(&unit, Some(&checkpoint)).unwrap();
        assert_eq!(replay, again);
        assert_eq!(replay["progress"]["watched_seconds"], 120);

        let last = fixture_step(&unit, Some(&json!({ "seconds": 120 }))).unwrap();
        assert_eq!(last["status"], "completed");
        assert_eq!(last["evidence"]["watched_seconds"], 180);
    }

    #[test]
    fn zero_duration_completes_on_the_first_step() {
        let result = fixture_step(&unit(Some(0)), None).unwrap();
        assert_eq!(result["status"], "completed");
        assert_eq!(result["evidence"]["watched_seconds"], 0);
    }

    #[test]
    fn garbage_checkpoints_are_rejected() {
        let error = fixture_step(&unit(Some(60)), Some(&json!("nope"))).unwrap_err();
        assert_eq!(error.code, "invalid_params");
    }
}
