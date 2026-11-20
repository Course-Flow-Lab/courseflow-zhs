//! Bounded, checkpointed verification: browser widget → local matching → gateway confirmation.
use super::{
    catalog::{Course, Unit},
    live::{self, Keys},
};
use crate::{error::ModuleError, host::Host};
use serde_json::{Value, json};

const PAGE_SCRIPT: &str = include_str!("captcha.js");

/// 易盾滑块在组件开始移动拼图前会先吃掉约 10 CSS 像素的指针位移。平台校验的是
/// 拼图最终位置，因此只拖动识别出的缺口距离会短 10 像素左右并返回 300(unpass)。
/// 该值在 `captcha.js` 固定的 320px 弹窗上校准；改动弹窗宽度后需要重新验证。
const SLIDER_DEAD_ZONE: f64 = 10.5;

fn progress(checkpoint: Value) -> Value {
    let position = checkpoint["resume"]["position"]
        .as_i64()
        .or_else(|| checkpoint["position"].as_i64())
        .or_else(|| checkpoint["watched_seconds"].as_i64())
        .unwrap_or(0);
    json!({"status":"in_progress","detail":"正在完成智慧树滑块验证","wait_ms":500,"checkpoint":checkpoint,"progress":{"watched_seconds":position}})
}

pub fn start(
    ev: String,
    resume: Value,
    generation: &str,
    course: &Course,
    unit: &Unit,
    now: i64,
) -> Value {
    progress(
        json!({"phase":"captcha","step":"open","ev":ev,"resume":resume,
        "execution_id":generation,"started_at":now,"course_id":course.id,"recruit_id":course.recruit_id,"unit_id":unit.id,"watched_seconds":unit.watched_seconds.unwrap_or(0)}),
    )
}

fn resume(checkpoint: &Value) -> Value {
    let mut cp = if checkpoint["resume"].is_object() {
        checkpoint["resume"].clone()
    } else if checkpoint["phase"] == "running" {
        checkpoint.clone()
    } else {
        json!({"phase":"captcha_retry"})
    };
    cp.as_object_mut().unwrap().remove("save_after");
    cp["captcha_retry"] = json!(true);
    cp["execution_id"] = checkpoint["execution_id"].clone();
    cp
}

pub fn needs_human(checkpoint: &Value, reason: &str) -> Value {
    json!({"status":"needs_human","reason":reason,"checkpoint":resume(checkpoint)})
}

fn close(host: &dyn Host, session: &str, checkpoint: &Value) {
    if let Some(page) = checkpoint["page"].as_str() {
        let _ = host.service("cw.browser.close", json!({"session":session,"page":page}));
    }
}

pub fn step(
    host: &dyn Host,
    keys: &Keys,
    session: &str,
    course: &Course,
    unit: &Unit,
    checkpoint: &Value,
    generation: &str,
) -> Result<Value, ModuleError> {
    if checkpoint["course_id"] != course.id
        || checkpoint["recruit_id"] != course.recruit_id
        || checkpoint["unit_id"] != unit.id
    {
        return Err(ModuleError::invalid_params("验证码检查点与当前视频不一致"));
    }
    let mut cp = checkpoint.clone();
    if cp["execution_id"].as_str().unwrap_or("") != generation {
        close(host, session, &cp);
        cp["step"] = json!("open");
        cp["page"] = Value::Null;
        cp["execution_id"] = json!(generation);
        cp["started_at"] = json!(host.now()?);
    }
    let result = advance(host, keys, session, &mut cp);
    match result {
        Ok(value) => Ok(value),
        Err(error) => {
            close(host, session, &cp);
            if ["platform_login_expired", "task_cancelled", "quiet_hours"].contains(&error.code) {
                return Err(error);
            }
            Ok(needs_human(
                &cp,
                &format!("自动滑块验证未完成：{}", error.message),
            ))
        }
    }
}

fn advance(
    host: &dyn Host,
    keys: &Keys,
    session: &str,
    cp: &mut Value,
) -> Result<Value, ModuleError> {
    if host.now()? - cp["started_at"].as_i64().unwrap_or(0) > 60 {
        return Err(ModuleError::verification_required(
            "验证码已超时，请在官方页面完成后继续",
        ));
    }
    if cp["step"] == "open" {
        let available = host.service("cw.captcha.status", json!({"session":session}))?;
        if available["available"] != true {
            return Err(ModuleError::verification_required(
                "服务器未启用验证码识别服务",
            ));
        }
        let opened = host.service("cw.browser.open", json!({"session":session,"url":"https://studyvideoh5.zhihuishu.com/stuStudy","purpose":"captcha","stage":"open_study"}))?;
        cp["page"] = opened
            .get("page")
            .filter(|v| v.as_str().is_some_and(|s| !s.is_empty()))
            .cloned()
            .ok_or_else(|| ModuleError::protocol_changed("验证码浏览器未返回页面编号"))?;
        cp["step"] = json!("match");
        return Ok(progress(cp.clone()));
    }
    let page = cp["page"]
        .as_str()
        .ok_or_else(|| ModuleError::verification_required("验证码页面已失效"))?
        .to_owned();
    let script = if cp["step"] == "confirm" {
        format!("window.__courseflowCaptchaWaitToken=true;{PAGE_SCRIPT}")
    } else {
        PAGE_SCRIPT.to_owned()
    };
    let snapshot = host.service(
        "cw.browser.evaluate",
        json!({"session":session,"page":page,"expression":script,"stage":"open_study"}),
    )?;
    if snapshot["loading"] == true {
        return Ok(progress(cp.clone()));
    }
    match snapshot["state"].as_str() {
        Some("loading") => Ok(progress(cp.clone())),
        Some("verified") => {
            let token = snapshot["token"]
                .as_str()
                .filter(|s| !s.is_empty() && s.len() <= 8192)
                .ok_or_else(|| ModuleError::verification_required("验证码没有返回有效令牌"))?;
            let ev = cp["ev"]
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| ModuleError::verification_required("验证码缺少视频上下文"))?;
            // A recognized gap or widget callback alone never proves platform acceptance.
            live::confirm_captcha(host, keys, session, token, ev)?;
            close(host, session, cp);
            let mut value = progress(resume(cp));
            value["detail"] = json!("智慧树已确认验证通过，正在恢复学习");
            Ok(value)
        }
        Some("puzzle") if cp["step"] == "match" => {
            let target = host.service(
                "cw.browser.image",
                json!({"session":session,"page":page,"url":snapshot["target_url"]}),
            )?;
            let background = host.service(
                "cw.browser.image",
                json!({"session":session,"page":page,"url":snapshot["background_url"]}),
            )?;
            let matched = host.service("cw.captcha.slide_match", json!({"session":session,"target_image":target["image"],"background_image":background["image"]}))?;
            let (x, y, distance) = drag(&snapshot, &matched)?;
            host.service(
                "cw.browser.mouse",
                json!({"session":session,"page":page,"stage":"open_study",
                "action":{"type":"drag","from":{"x":x,"y":y},"to":{"x":x+distance,"y":y}}}),
            )?;
            cp["step"] = json!("confirm");
            Ok(progress(cp.clone()))
        }
        Some("unsupported") => Err(ModuleError::verification_required(
            "当前为图形点选或其他验证，请在官方页面完成",
        )),
        _ => Err(ModuleError::verification_required(
            "官方验证码组件未通过验证，请在官方页面完成",
        )),
    }
}

fn drag(snapshot: &Value, matched: &Value) -> Result<(f64, f64, f64), ModuleError> {
    let number = |value: &Value| {
        value
            .as_f64()
            .filter(|n| n.is_finite())
            .ok_or_else(|| ModuleError::verification_required("验证码缺少有效图片坐标"))
    };
    let width = number(&snapshot["width"])?;
    let natural = number(&snapshot["natural_width"])?;
    let gap = number(&matched["target"][0])? - number(&matched["target_x"])?;
    let distance = gap * width / natural - number(&snapshot["piece_left"])? + SLIDER_DEAD_ZONE;
    if width <= 0.0
        || natural <= 0.0
        || !distance.is_finite()
        || distance <= 0.0
        || distance >= width
    {
        return Err(ModuleError::verification_required(
            "滑块识别坐标超出图片范围",
        ));
    }
    Ok((
        number(&snapshot["slider_x"])?,
        number(&snapshot["slider_y"])?,
        distance,
    ))
}

#[cfg(test)]
mod tests {
    #[test]
    fn page_script_uses_the_official_study_wrapper() {
        // The study page calls ableCaptchaCommon with capType
        // 'ImageFocusCombine', which selects the jigsaw branch.
        // 4da305... is the 'space' fallback and presents a click challenge.
        assert!(super::PAGE_SCRIPT.contains("initAbleYidunFallback"));
        assert!(super::PAGE_SCRIPT.contains("ImageFocusCombine"));
        assert!(!super::PAGE_SCRIPT.contains("4da3050565514a35a50541b0e1f54538"));
    }
}
