//! Live 智慧树 protocol.
//!
//! 在线协议通过模块 API 实现，包括 RSA/AES 密钥交换（`crypto.rs`）、
//! `secretStr` 网关签名、
//! Passport SSO exchange, the shared-course outline with server-reported progress, and the
//! `prelearningNote` / `saveDatabaseIntervalTimeV2` study writes.
//!
//! The module never sees a cookie. Passport cookies live in the framework's challenge jar, learning
//! cookies in its session jar, and the module only passes the opaque handle back.

use std::collections::HashMap;
use std::sync::Mutex;

use base64::Engine;
use serde_json::{Value, json};

use crate::error::ModuleError;
use crate::host::{Encoding, FetchRequest, Host, Stage};

use super::catalog::{Course, Unit, UnitState};
use super::{check_code, crypto, endpoints, fetch_json, id_string, number};

/// Per (session, gateway module) AES keys. The framework may serve several accounts at once, so
/// keys are never global: one account's gateway key must not sign another account's requests.
#[derive(Default)]
pub struct Keys(Mutex<HashMap<(String, u8), Vec<u8>>>);

impl Keys {
    fn get(&self, session: &str, module: u8) -> Option<Vec<u8>> {
        self.0
            .lock()
            .ok()?
            .get(&(session.to_owned(), module))
            .cloned()
    }

    fn put(&self, session: &str, module: u8, key: Vec<u8>) {
        if let Ok(mut keys) = self.0.lock() {
            keys.insert((session.to_owned(), module), key);
        }
    }

    pub fn forget(&self, session: &str) {
        if let Ok(mut keys) = self.0.lock() {
            keys.retain(|(owner, _), _| owner != session);
        }
    }
}

pub fn protocol_key(
    host: &dyn Host,
    keys: &Keys,
    session: &str,
    module: u8,
) -> Result<Vec<u8>, ModuleError> {
    if let Some(key) = keys.get(session, module) {
        return Ok(key);
    }
    let public = crypto::bootstrap_public_key()?;
    let uid = crypto::encrypt_module_request(&public, module)?;
    let url = format!(
        "{}?uid={}",
        endpoints::KEY_SERVICE_URL,
        encode_component(&uid)
    );
    let reply = fetch_json(host, FetchRequest::get(session, url))?;
    let signed = reply
        .get("rt")
        .and_then(|rt| rt.get("sl"))
        .and_then(Value::as_str)
        .ok_or_else(|| ModuleError::protocol_changed("智慧树密钥响应缺少 rt.sl"))?;
    let key = crypto::recover_protocol_key(&public, signed)?;
    keys.put(session, module, key.clone());
    Ok(key)
}

/// One signed gateway call: `secretStr` + `dateFormate` form fields, Origin/Referer per gateway.
fn signed_post(
    host: &dyn Host,
    session: &str,
    base: &str,
    path: &str,
    payload: Value,
    key: &[u8],
    origin: &str,
    stage: Stage,
) -> Result<Value, ModuleError> {
    let mut payload = payload;
    let date = host.now()?.saturating_mul(1000);
    payload["dateFormate"] = json!(date);
    let secret = crypto::encode_query(&payload, key)?;
    let body = json!({ "secretStr": secret, "dateFormate": date });
    let url = format!("{base}{path}");
    let request = FetchRequest {
        session: session.to_owned(),
        method: "POST".to_owned(),
        url,
        headers: [
            ("Origin".to_owned(), origin.to_owned()),
            ("Referer".to_owned(), format!("{origin}/")),
        ]
        .into_iter()
        .collect(),
        body: Some(body),
        encoding: Some(Encoding::Form),
        stage: Some(stage),
        commit: false,
    };
    fetch_json(host, request)
}

fn encode_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

// --- login ------------------------------------------------------------------------

/// The SSO exchange after the QR is confirmed:
/// `abnormalLogin` (anti-risk check) → `passport/login?pwd&service` (cookie exchange) → verify.
pub fn finish_login(host: &dyn Host, session: &str, reply: &Value) -> Result<Value, ModuleError> {
    let once_password = reply
        .get("oncePassword")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ModuleError::protocol_changed("扫码结果缺少 oncePassword"))?;
    let uuid = id_string(&reply["uuid"])
        .ok_or_else(|| ModuleError::protocol_changed("扫码结果缺少 uuid"))?;

    let abnormal_url = format!(
        "{}{}",
        endpoints::PASSPORT_BASE,
        endpoints::ABNORMAL_LOGIN_PATH
    );
    let abnormal = fetch_json(
        host,
        FetchRequest {
            session: session.to_owned(),
            method: "POST".to_owned(),
            url: abnormal_url,
            headers: Default::default(),
            body: Some(json!({ "uuid": uuid })),
            encoding: Some(Encoding::Form),
            stage: Some(Stage::Read),
            commit: false,
        },
    )?;
    if number(&abnormal["status"]) != Some(0) {
        return Err(ModuleError::verification_required(
            "智慧树要求额外验证，请先到官方页面处理后重新扫码",
        ));
    }

    // The one-time exchange is never retried after an uncertain result.
    let service = format!("{}{}", endpoints::SERVICE_BASE, endpoints::GO_LOGIN_PATH);
    let login_url = format!(
        "{}{}?pwd={}&service={}",
        endpoints::PASSPORT_BASE,
        endpoints::PASSPORT_LOGIN_PATH,
        encode_component(once_password),
        encode_component(&format!(
            "{service}?fromurl={}",
            encode_component(endpoints::COURSE_LOGIN_FROM)
        )),
    );
    let _ = fetch_json_or_page(host, FetchRequest::get(session, login_url))?;

    let profile = verify(host, session)?;
    Ok(profile)
}

/// Some Passport responses are HTML redirects, so a non-JSON 2xx is not an error here.
fn fetch_json_or_page(
    host: &dyn Host,
    request: FetchRequest,
) -> Result<Option<Value>, ModuleError> {
    let response = super::fetch(host, request)?;
    Ok(serde_json::from_str(&response.body).ok())
}

/// `getLoginUserInfo`: also the session-restore check.
pub fn verify(host: &dyn Host, session: &str) -> Result<Value, ModuleError> {
    let time = host.now()?.saturating_mul(1000);
    let url = format!(
        "{}{}?time={time}",
        endpoints::SERVICE_BASE,
        endpoints::LOGIN_USER_INFO_PATH
    );
    let reply = fetch_json(host, FetchRequest::get(session, url))?;
    check_code(&reply, &[200])?;
    let profile = reply
        .get("result")
        .filter(|value| value.is_object())
        .ok_or_else(|| ModuleError::login_expired("智慧树会话未建立，请重新扫码"))?;
    let name = profile
        .get("realName")
        .or_else(|| profile.get("name"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("智慧树账号")
        .to_owned();
    Ok(json!({ "display_name": name }))
}

// --- catalogue --------------------------------------------------------------------

/// Shared courses (signed, paginated) plus the AI/智慧课程 list. Returns the courses and the
/// opaque per-course `secret` entries that the outline and study calls need.
pub fn courses(
    host: &dyn Host,
    keys: &Keys,
    session: &str,
) -> Result<(Vec<Course>, Value), ModuleError> {
    let key = protocol_key(host, keys, session, 13)?;
    let mut entries = serde_json::Map::new();
    let mut courses: Vec<Course> = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for page in 1..=100u32 {
        let reply = signed_post(
            host,
            session,
            endpoints::SERVICE_BASE,
            endpoints::SHARE_COURSES_PATH,
            json!({ "status": 0, "pageNo": page, "pageSize": 20 }),
            &key,
            "https://onlineweb.zhihuishu.com",
            Stage::Read,
        )?;
        check_code(&reply, &[200])?;
        let result = reply
            .get("result")
            .filter(|value| value.is_object())
            .ok_or_else(|| ModuleError::protocol_changed("智慧树课程响应缺少 result"))?;
        let total = number(&result["totalCount"]).unwrap_or(0).max(0) as usize;
        let rows: &[Value] = match result.get("courseOpenDtos") {
            Some(Value::Array(rows)) => rows,
            None | Some(Value::Null) if total == 0 => &[],
            _ => {
                return Err(ModuleError::protocol_changed(
                    "智慧树课程响应缺少 result.courseOpenDtos",
                ));
            }
        };
        let received = rows.len();
        for row in rows {
            let course = shared_course(row)?;
            if !seen.insert((course.id.clone(), course.recruit_id.clone())) {
                return Err(ModuleError::protocol_changed("智慧树课程列表出现重复课程"));
            }
            if let Some(entry) = row.get("secret").and_then(Value::as_str) {
                if !entry.is_empty() && entry.len() <= 4096 {
                    entries.insert(format!("{}/{}", course.id, course.recruit_id), json!(entry));
                    // Only courses with a `secret` can be opened; mirrors outline_available.
                }
            }
            courses.push(course);
        }
        if courses.len() >= total || received == 0 {
            break;
        }
    }

    // 智慧课程 / AI 优课. Optional: a failure here must not hide the shared courses.
    if let Ok(reply) = ai_courses(host, session, &key) {
        for row in reply {
            if let Some(course) = ai_course(&row) {
                if seen.insert((course.id.clone(), course.recruit_id.clone())) {
                    courses.push(course);
                }
            }
        }
    }

    Ok((courses, Value::Object(entries)))
}

fn ai_courses(host: &dyn Host, session: &str, key: &[u8]) -> Result<Vec<Value>, ModuleError> {
    let reply = signed_post(
        host,
        session,
        endpoints::SERVICE_BASE,
        endpoints::AI_COURSES_PATH,
        json!({ "status": 3 }),
        key,
        "https://onlineweb.zhihuishu.com",
        Stage::Read,
    )?;
    match number(&reply["code"]) {
        Some(200) => {}
        Some(401 | 403 | 5008) => return Err(ModuleError::login_expired("智慧树登录已失效")),
        _ => {
            return Err(ModuleError::protocol_changed(
                "智慧课程列表返回了未预期的 code",
            ));
        }
    }
    Ok(reply
        .get("rt")
        .or_else(|| reply.get("data"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}

fn shared_course(row: &Value) -> Result<Course, ModuleError> {
    let id = id_string(&row["courseId"])
        .or_else(|| id_string(&row["id"]))
        .ok_or_else(|| ModuleError::protocol_changed("课程缺少 courseId"))?;
    let recruit_id = id_string(&row["recruitId"])
        .ok_or_else(|| ModuleError::protocol_changed("课程缺少 recruitId"))?;
    let name = row["courseName"]
        .as_str()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ModuleError::protocol_changed("课程缺少 courseName"))?
        .to_owned();
    let progress = row["studyPercent"]
        .as_f64()
        .or_else(|| row["studyPercent"].as_str()?.parse().ok())
        .filter(|value| value.is_finite() && (0.0..=100.0).contains(value));
    let outline_available = row["secret"]
        .as_str()
        .is_some_and(|entry| !entry.is_empty() && entry.len() <= 2048);
    Ok(Course {
        id,
        recruit_id,
        name,
        kind: "shared".to_owned(),
        kind_label: Some("共享课".to_owned()),
        family: Some("shared".to_owned()),
        progress,
        outline_available,
    })
}

fn ai_course(row: &Value) -> Option<Course> {
    let id = id_string(&row["courseId"])?;
    let recruit_id = id_string(&row["classId"])?;
    if id.is_empty() || recruit_id.is_empty() {
        return None;
    }
    let name = row["courseName"]
        .as_str()
        .or_else(|| row["name"].as_str())
        .unwrap_or("智慧课程")
        .to_owned();
    let progress = row["schedule"]
        .as_f64()
        .filter(|value| (0.0..=100.0).contains(value));
    Some(Course {
        id,
        recruit_id,
        name,
        kind: "smart".to_owned(),
        kind_label: Some("智慧课程".to_owned()),
        family: Some("smart".to_owned()),
        progress,
        // The smart outline is a different gateway; not ported in this pass.
        outline_available: true,
    })
}

/// Shared-course outline with server-reported progress.
///
/// `videolist` needs the learning gateway session, which the Passport cookie exchange does not
/// create: on `401` the module performs the documented `login/gologin` bridge once and retries.
pub fn outline(
    host: &dyn Host,
    keys: &Keys,
    session: &str,
    course: &Course,
    entry: &str,
) -> Result<Vec<Unit>, ModuleError> {
    let key = protocol_key(host, keys, session, 11)?;
    let payload = json!({ "recruitAndCourseId": entry });
    let origin = "https://studyvideoh5.zhihuishu.com";

    let first = signed_post(
        host,
        session,
        endpoints::LEARNING_BASE,
        endpoints::OUTLINE_VIDEOLIST_PATH,
        payload.clone(),
        &key,
        origin,
        Stage::Read,
    );
    let reply = match first {
        Ok(reply) => match number(&reply["code"]) {
            Some(0) => reply,
            Some(401 | 403 | 5008) => {
                bridge_learning_session(host, session)?;
                signed_post(
                    host,
                    session,
                    endpoints::LEARNING_BASE,
                    endpoints::OUTLINE_VIDEOLIST_PATH,
                    payload,
                    &key,
                    origin,
                    Stage::Read,
                )?
            }
            Some(-2) => {
                return Err(ModuleError::new(
                    "platform_rejected",
                    "智慧树未开放这门课程的目录，请到官方页面核对课程状态",
                ));
            }
            Some(-12) => {
                return Err(ModuleError::verification_required(
                    "智慧树要求额外验证，请先到官方页面处理后重试",
                ));
            }
            Some(code) => {
                return Err(ModuleError::protocol_changed(format!(
                    "智慧树目录返回了未预期的 code {code}"
                )));
            }
            None => {
                return Err(ModuleError::protocol_changed("智慧树目录响应缺少 code"));
            }
        },
        Err(error) if error.code == crate::error::PLATFORM_LOGIN_EXPIRED => {
            bridge_learning_session(host, session)?;
            signed_post(
                host,
                session,
                endpoints::LEARNING_BASE,
                endpoints::OUTLINE_VIDEOLIST_PATH,
                payload,
                &key,
                origin,
                Stage::Read,
            )?
        }
        Err(error) => return Err(error),
    };

    let mut videos = parse_outline(course, &reply)?;
    apply_progress(host, session, course, &key, origin, &mut videos)?;
    Ok(units_from_parsed(&videos))
}

fn bridge_learning_session(host: &dyn Host, session: &str) -> Result<(), ModuleError> {
    let url = format!(
        "{}/login/gologin?fromurl={}",
        endpoints::LEARNING_BASE,
        encode_component("https://studyvideoh5.zhihuishu.com/stuStudy")
    );
    let _ = super::fetch(host, FetchRequest::get(session, url))?;
    Ok(())
}

/// The platform usually explains a refusal in `message`/`msg`; the reference prints it too.
pub fn platform_message(reply: &Value) -> String {
    reply
        .get("message")
        .or_else(|| reply.get("msg"))
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty())
        .unwrap_or("平台未给出说明")
        .to_owned()
}

fn learning_data<'a>(reply: &'a Value, endpoint: &str) -> Result<&'a Value, ModuleError> {
    let result = match number(&reply["code"]) {
        Some(0) => reply
            .get("data")
            .filter(|value| value.is_object())
            .ok_or_else(|| ModuleError::protocol_changed("学习网关响应缺少 data")),
        Some(401 | 403 | 5008) => Err(ModuleError::login_expired("智慧树登录已失效，请重新扫码")),
        Some(-2) => Err(ModuleError::new(
            "platform_rejected",
            "智慧树未开放这门课程，请到官方页面核对课程状态",
        )),
        Some(-12) => Err(ModuleError::verification_required(
            "智慧树要求额外验证，请先到官方页面处理后重试",
        )),
        Some(421) => Err(ModuleError::rejected(format!(
            "智慧树拒绝学习请求（code 421）：{}；请核对官方页面能否播放当前视频",
            platform_message(reply)
        ))),
        Some(code) => Err(ModuleError::protocol_changed(format!(
            "学习网关返回了未预期的 code {code}：{}",
            platform_message(reply)
        ))),
        None => Err(ModuleError::protocol_changed("学习网关响应缺少 code")),
    };
    result.map_err(|mut error| {
        error.message = format!("{}；接口：{endpoint}", error.message);
        error
    })
}

struct ParsedVideo {
    id: String,
    name: String,
    video_id: Option<String>,
    duration: Option<i64>,
    small_lesson: bool,
    chapter_id: String,
    chapter_name: String,
    lesson_id: String,
    progress: Option<Value>,
}

fn parse_outline(course: &Course, reply: &Value) -> Result<Vec<ParsedVideo>, ModuleError> {
    let data = learning_data(reply, endpoints::OUTLINE_VIDEOLIST_PATH)?;
    if id_string(&data["recruitId"]).as_deref() != Some(course.recruit_id.as_str()) {
        return Err(ModuleError::protocol_changed(
            "目录返回的 recruitId 与课程不一致",
        ));
    }
    let chapters = data["videoChapterDtos"]
        .as_array()
        .ok_or_else(|| ModuleError::protocol_changed("目录缺少 videoChapterDtos"))?;
    let mut videos = Vec::new();
    let mut nodes = 0usize;
    for chapter in chapters {
        let chapter_id = id_string(&chapter["id"])
            .ok_or_else(|| ModuleError::protocol_changed("章节缺少 id"))?;
        let chapter_name = display_name(chapter, "未命名章节");
        for lesson in array_or_empty(&chapter["videoLessons"])? {
            let lesson_id = id_string(&lesson["id"])
                .ok_or_else(|| ModuleError::protocol_changed("小节缺少 id"))?;
            let children = array_or_empty(&lesson["videoSmallLessons"])?;
            let small = !children.is_empty();
            for row in if small {
                children
            } else {
                std::slice::from_ref(lesson)
            } {
                let id = id_string(&row["id"])
                    .ok_or_else(|| ModuleError::protocol_changed("视频缺少 id"))?;
                videos.push(ParsedVideo {
                    id,
                    name: display_name(row, "未命名视频"),
                    video_id: id_string(&row["videoId"]),
                    duration: number(&row["videoSec"]).filter(|value| *value >= 0),
                    small_lesson: small,
                    chapter_id: chapter_id.clone(),
                    chapter_name: chapter_name.clone(),
                    lesson_id: lesson_id.clone(),
                    progress: None,
                });
                nodes += 1;
            }
            nodes += 1;
        }
        nodes += 1;
        if nodes > 5000 {
            return Err(ModuleError::protocol_changed("目录节点过多"));
        }
    }
    Ok(videos)
}

fn apply_progress(
    host: &dyn Host,
    session: &str,
    course: &Course,
    key: &[u8],
    origin: &str,
    videos: &mut [ParsedVideo],
) -> Result<(), ModuleError> {
    let lesson_ids: Vec<Value> = videos
        .iter()
        .map(|video| remote_id(&video.lesson_id))
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    if lesson_ids.is_empty() {
        return Ok(());
    }
    let small_ids: Vec<Value> = videos
        .iter()
        .filter(|video| video.small_lesson)
        .map(|video| remote_id(&video.id))
        .collect();
    let mut payload = json!({
        "recruitId": remote_id(&course.recruit_id),
        "lessonIds": lesson_ids,
    });
    if !small_ids.is_empty() {
        payload["lessonVideoIds"] = json!(small_ids);
    }
    let reply = signed_post(
        host,
        session,
        endpoints::LEARNING_BASE,
        endpoints::OUTLINE_PROGRESS_PATH,
        payload,
        key,
        origin,
        Stage::Read,
    )?;
    let data = learning_data(&reply, endpoints::OUTLINE_PROGRESS_PATH)?;
    for key_name in ["lesson", "lv"] {
        if !data[key_name].is_null() && !data[key_name].is_object() {
            return Err(ModuleError::protocol_changed("进度响应结构不正确"));
        }
    }
    for video in videos.iter_mut() {
        let bucket = if video.small_lesson { "lv" } else { "lesson" };
        let record = &data[bucket][&video.id];
        video.progress = Some(record.clone());
    }
    Ok(())
}

fn display_name(row: &Value, fallback: &str) -> String {
    row["name"]
        .as_str()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(fallback)
        .to_owned()
}

fn array_or_empty(value: &Value) -> Result<&[Value], ModuleError> {
    match value {
        Value::Null => Ok(&[]),
        Value::Array(rows) => Ok(rows),
        _ => Err(ModuleError::protocol_changed("目录字段不是数组")),
    }
}

/// Convert the parsed outline plus its progress records into contract `Unit`s.
fn units_from_parsed(videos: &[ParsedVideo]) -> Vec<Unit> {
    videos
        .iter()
        .map(|video| {
            let record = video.progress.as_ref();
            let watch_state = record.and_then(|record| number(&record["watchState"]));
            let watched = record
                .and_then(|record| number(&record["studyTotalTime"]))
                .filter(|value| *value >= 0);
            let state = match watch_state {
                Some(0) => UnitState::NotStarted,
                Some(1) => UnitState::Completed,
                Some(2) => UnitState::InProgress,
                _ => UnitState::Unknown,
            };
            let duration = video.duration.filter(|value| *value > 0);
            let selectable = duration.is_some();
            Unit {
                id: video.id.clone(),
                name: video.name.clone(),
                chapter_id: video.chapter_id.clone(),
                chapter_name: video.chapter_name.clone(),
                lesson_id: video.lesson_id.clone(),
                kind: "video".to_owned(),
                duration_seconds: video.duration,
                watched_seconds: watched,
                state,
                video_id: video.video_id.clone(),
                selectable,
                selectable_note: if selectable {
                    None
                } else {
                    Some("平台未提供这个视频的时长，暂时不能自动学习".to_owned())
                },
            }
        })
        .collect()
}

// --- study ------------------------------------------------------------------------

#[derive(Debug)]
pub struct Prepared {
    pub record_id: String,
    pub position: i64,
    pub total: i64,
    pub duration: i64,
    pub media_id: String,
    pub chapter_id: String,
    pub lesson_id: String,
    pub small_lesson_id: Option<String>,
    pub limited: bool,
    pub school_id: Value,
    /// Question points from `loadVideoPointerInfo`, ascending by time: `(seconds, questionIds)`.
    pub questions: Vec<(i64, Value)>,
    /// Earliest non-question overlay (theme/picture/knowledge card). The reference stops there.
    pub blocking_at: Option<i64>,
}

pub enum Preparation {
    Ready(Prepared),
    Captcha(String),
}

pub enum SaveResult {
    Saved,
    Captcha(String),
}

impl Prepared {
    pub fn to_checkpoint(&self) -> Value {
        json!({
            "phase": "running",
            "record_id": self.record_id,
            "position": self.position,
            "total": self.total,
            "questions": self.questions.iter().map(|(at, ids)| json!({"at": at, "ids": ids})).collect::<Vec<_>>(),
            "blocking_at": self.blocking_at,
            "duration": self.duration,
            "media_id": self.media_id,
            "chapter_id": self.chapter_id,
            "lesson_id": self.lesson_id,
            "small_lesson_id": self.small_lesson_id,
            "limited": self.limited,
            "school_id": self.school_id,
        })
    }

    pub fn from_checkpoint(value: &Value) -> Result<Self, ModuleError> {
        let text = |key: &str| -> Result<String, ModuleError> {
            value
                .get(key)
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| ModuleError::protocol_changed(format!("学习检查点缺少 {key}")))
        };
        let number_of = |key: &str| -> Result<i64, ModuleError> {
            value
                .get(key)
                .and_then(number)
                .ok_or_else(|| ModuleError::protocol_changed(format!("学习检查点缺少 {key}")))
        };
        Ok(Self {
            record_id: text("record_id")?,
            position: number_of("position")?,
            total: number_of("total")?,
            questions: value
                .get("questions")
                .and_then(Value::as_array)
                .map(|rows| {
                    rows.iter()
                        .filter_map(|row| Some((number(&row["at"])?, row.get("ids")?.clone())))
                        .collect()
                })
                .unwrap_or_default(),
            blocking_at: value.get("blocking_at").and_then(number),
            duration: number_of("duration")?,
            media_id: text("media_id")?,
            chapter_id: text("chapter_id")?,
            lesson_id: text("lesson_id")?,
            small_lesson_id: value
                .get("small_lesson_id")
                .and_then(Value::as_str)
                .map(str::to_owned),
            limited: value
                .get("limited")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            school_id: value.get("school_id").cloned().unwrap_or(json!(0)),
        })
    }

    pub fn advance(&mut self, seconds: i64) {
        self.position += seconds;
        self.total += seconds;
    }

    /// Question points that are due at the current position.
    pub fn due_questions(&self) -> Vec<&(i64, Value)> {
        self.questions
            .iter()
            .filter(|(at, _)| *at <= self.position)
            .collect()
    }

    /// The next boundary: a question, a blocking overlay, or the end of the video.
    pub fn boundary(&self) -> i64 {
        let question = self
            .questions
            .iter()
            .map(|(at, _)| *at)
            .find(|at| *at > self.position)
            .unwrap_or(self.duration);
        let blocking = self
            .blocking_at
            .filter(|at| *at > self.position)
            .unwrap_or(self.duration);
        question.min(blocking).min(self.duration)
    }
}

fn small_lesson_id(unit: &Unit) -> Option<&str> {
    (unit.id != unit.lesson_id).then_some(unit.id.as_str())
}

/// `queryCourse` → chapter gate → optional time limit → popup pointers → `prelearningNote`.
#[allow(clippy::too_many_arguments)]
pub fn prepare(
    host: &dyn Host,
    keys: &Keys,
    session: &str,
    _account_raw: &str,
    course: &Course,
    entry: &str,
    unit: &Unit,
    earlier_incomplete: bool,
) -> Result<Preparation, ModuleError> {
    let key = protocol_key(host, keys, session, 11)?;
    let origin = "https://studyvideoh5.zhihuishu.com";
    let duration = unit
        .duration_seconds
        .filter(|value| *value > 0)
        .ok_or_else(|| ModuleError::new("platform_rejected", "视频时长不可用，请刷新目录后再试"))?;
    let media_id = unit
        .video_id
        .clone()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ModuleError::protocol_changed("视频缺少 videoId"))?;

    let policy_reply = signed_post(
        host,
        session,
        endpoints::LEARNING_BASE,
        endpoints::STUDY_QUERY_COURSE_PATH,
        json!({ "recruitAndCourseId": entry }),
        &key,
        origin,
        Stage::Read,
    )?;
    let policy = learning_data(&policy_reply, endpoints::STUDY_QUERY_COURSE_PATH)?;
    if id_string(&policy["recruitId"]).as_deref() != Some(course.recruit_id.as_str())
        || id_string(&policy["courseInfo"]["courseId"]).as_deref() != Some(course.id.as_str())
    {
        return Err(ModuleError::protocol_changed(
            "课程策略返回的课程与请求不一致",
        ));
    }
    if number(&policy["studyStatus"]) != Some(0) {
        return Err(ModuleError::new(
            "platform_rejected",
            "课程当前不开放学习，继续观看可能不计进度，请到官方页面核对",
        ));
    }
    let can_jump = flag(&policy["courseInfo"]["canJumpChapter"])?;
    if !can_jump && earlier_incomplete {
        return Err(ModuleError::new(
            "platform_rejected",
            "这门课程要求先完成前面的章节，请选择更早的视频",
        ));
    }
    let school_id = policy
        .get("schoolId")
        .filter(|value| !value.is_null())
        .cloned()
        .unwrap_or(json!(0));
    let limited = number(&policy["courseInfo"]["type"]) == Some(1);
    if limited {
        study_limit(host, session, &course.recruit_id, &school_id, &key, origin)?;
    }

    let mut popup = json!({
        "courseId": remote_id(&course.id),
        "recruitId": remote_id(&course.recruit_id),
        "lessonId": remote_id(&unit.lesson_id),
    });
    if let Some(id) = small_lesson_id(unit) {
        popup["lessonVideoId"] = remote_id(id);
    }
    let pointers = signed_post(
        host,
        session,
        endpoints::LEARNING_BASE,
        endpoints::STUDY_POPUP_PATH,
        popup,
        &key,
        origin,
        Stage::Read,
    )?;
    let pointers = learning_data(&pointers, endpoints::STUDY_POPUP_PATH)?;
    let mut questions: Vec<(i64, Value)> = Vec::new();
    for row in array_or_empty(&pointers["questionPoint"])? {
        let at = number(&row["timeSec"])
            .filter(|value| *value >= 0)
            .ok_or_else(|| ModuleError::protocol_changed("弹题缺少 timeSec"))?;
        let ids = row
            .get("questionIds")
            .cloned()
            .filter(|value| !value.is_null())
            .ok_or_else(|| ModuleError::protocol_changed("弹题缺少 questionIds"))?;
        questions.push((at, ids));
    }
    questions.sort_by_key(|(at, _)| *at);
    let mut blocking_at: Option<i64> = None;
    for field in ["videoThemeDtos", "popupPictureDtos", "knowledgeCardDtos"] {
        for row in array_or_empty(&pointers[field])? {
            let at = number(&row["timeSec"])
                .filter(|value| *value >= 0)
                .ok_or_else(|| ModuleError::protocol_changed("互动点缺少 timeSec"))?;
            blocking_at = Some(blocking_at.map_or(at, |previous| previous.min(at)));
        }
    }

    let mut input = json!({
        "ccCourseId": remote_id(&course.id),
        "chapterId": remote_id(&unit.chapter_id),
        "isApply": policy.get("isApply").filter(|value| number(value).is_some_and(|n| n != 0)).cloned().unwrap_or(json!(1)),
        "lessonId": remote_id(&unit.lesson_id),
        "recruitId": remote_id(&course.recruit_id),
        "videoId": remote_id(&media_id),
    });
    if let Some(id) = small_lesson_id(unit) {
        input["lessonVideoId"] = remote_id(id);
    }
    let reply = signed_post(
        host,
        session,
        endpoints::LEARNING_BASE,
        endpoints::STUDY_OPEN_PATH,
        input,
        &key,
        origin,
        Stage::OpenStudy,
    )?;
    let requires_captcha = number(&reply["code"]) == Some(-12);
    let data = if requires_captcha {
        &reply["data"]
    } else {
        learning_data(&reply, endpoints::STUDY_OPEN_PATH)?
    };
    if flag(&data["isFace"])? {
        return Err(ModuleError::verification_required(
            "智慧树要求人脸验证，请先到官方页面处理",
        ));
    }
    if requires_captcha || flag(&data["isSlide"])? {
        return Ok(Preparation::Captcha(captcha_ev(
            course,
            &unit.chapter_id,
            &unit.lesson_id,
            &media_id,
            small_lesson_id(unit),
        )));
    }
    let record = &data["studiedLessonDto"];
    let record_id =
        id_string(&record["id"]).ok_or_else(|| ModuleError::protocol_changed("视频记录缺少 id"))?;
    let position = number(&record["learnTimeSec"])
        .filter(|value| *value >= 0 && *value <= duration)
        .ok_or_else(|| ModuleError::protocol_changed("视频记录缺少 learnTimeSec"))?;
    let total = number(&record["studyTotalTime"])
        .filter(|value| *value >= 0)
        .ok_or_else(|| ModuleError::protocol_changed("视频记录缺少 studyTotalTime"))?;

    Ok(Preparation::Ready(Prepared {
        record_id,
        position,
        total,
        duration,
        media_id,
        chapter_id: unit.chapter_id.clone(),
        lesson_id: unit.lesson_id.clone(),
        small_lesson_id: small_lesson_id(unit).map(str::to_owned),
        limited,
        school_id,
        questions,
        blocking_at,
    }))
}

fn study_limit(
    host: &dyn Host,
    session: &str,
    recruit_id: &str,
    school_id: &Value,
    key: &[u8],
    origin: &str,
) -> Result<(), ModuleError> {
    let mut payload = json!({ "recruitId": remote_id(recruit_id), "schoolId": school_id });
    let date = host.now()?.saturating_mul(1000);
    payload["dateFormate"] = json!(date);
    let secret = crypto::encode_query(&payload, key)?;
    let url = format!(
        "{}{}?secretStr={}&dateFormate={date}",
        endpoints::LEARNING_BASE,
        endpoints::STUDY_LIMIT_PATH,
        encode_component(&secret)
    );
    let reply = fetch_json(
        host,
        FetchRequest {
            session: session.to_owned(),
            method: "GET".to_owned(),
            url,
            headers: [
                ("Origin".to_owned(), origin.to_owned()),
                ("Referer".to_owned(), format!("{origin}/")),
            ]
            .into_iter()
            .collect(),
            body: None,
            encoding: None,
            stage: Some(Stage::Read),
            commit: false,
        },
    )?;
    let data = learning_data(&reply, endpoints::STUDY_LIMIT_PATH)?;
    if !flag(&data["studyTimeDto"]["canBeStudyNow"])? {
        return Err(ModuleError::new(
            "platform_rejected",
            "平台当前不允许学习，可能已达到时长限额或不在开放时段，请到官方页面核对",
        ));
    }
    Ok(())
}

/// Read one popup question set and answer it from the platform's own option marks
/// (`questionOptions[].result == 1`). Returns `Ok(false)` when a question has no marked answer,
/// which needs a human or a model — the step then reports `needs_human`.
pub fn answer_popup(
    host: &dyn Host,
    keys: &Keys,
    session: &str,
    course: &Course,
    unit: &Unit,
    question_ids: &Value,
) -> Result<bool, ModuleError> {
    Ok(popup_answers(host, keys, session, course, unit, question_ids, &json!({}))?.is_empty())
}
pub fn popup_answers(
    host: &dyn Host,
    keys: &Keys,
    session: &str,
    course: &Course,
    unit: &Unit,
    question_ids: &Value,
    supplied: &Value,
) -> Result<Vec<super::features::types::Question>, ModuleError> {
    let key = protocol_key(host, keys, session, 11)?;
    let origin = "https://studyvideoh5.zhihuishu.com";
    let mut payload = json!({
        "lessonId": remote_id(&unit.lesson_id),
        "questionIds": question_ids,
    });
    if let Some(id) = small_lesson_id(unit) {
        payload["lessonVideoId"] = remote_id(id);
    }
    let reply = signed_post(
        host,
        session,
        endpoints::LEARNING_BASE,
        endpoints::POPUP_EXAM_PATH,
        payload,
        &key,
        origin,
        Stage::Read,
    )?;
    let data = learning_data(&reply, endpoints::POPUP_EXAM_PATH)?;
    let items = data["lessonTestQuestionUseInterfaceDtos"]
        .as_array()
        .ok_or_else(|| ModuleError::protocol_changed("弹题响应缺少题目数组"))?;
    if items.is_empty() {
        return Err(ModuleError::protocol_changed("弹题没有可处理的题目"));
    }
    let mut pending = Vec::new();
    let mut writes = Vec::new();
    for item in items {
        let raw = &item["testQuestion"];
        let id = id_string(&raw["questionId"])
            .or_else(|| id_string(&raw["id"]))
            .ok_or_else(|| ModuleError::protocol_changed("弹题缺少 questionId"))?;
        let provided: Vec<String> = array_or_empty(&raw["questionOptions"])?
            .iter()
            .filter(|option| number(&option["result"]) == Some(1))
            .filter_map(|option| id_string(&option["id"]))
            .collect();
        let answer = if provided.is_empty() {
            let mut raw = raw.clone();
            raw["eid"] = json!(id);
            let question = super::features::normalize_popup(&raw)?;
            let manual: Vec<String> = supplied
                .get(&id)
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            if !super::features::types::validate_answer(&question, &manual) {
                pending.push(question);
                continue;
            }
            manual
        } else {
            provided
        };
        writes.push((id, answer));
    }
    if !pending.is_empty() {
        return Ok(pending);
    }
    for (id, answer) in writes {
        let mut payload = json!({
            "courseId": remote_id(&course.id),
            "recruitId": remote_id(&course.recruit_id),
            "testQuestionId": remote_id(&id),
            "isCurrent": "1",
            "lessonId": remote_id(&unit.lesson_id),
            "answer": answer.join(","),
            "testType": 0,
        });
        if let Some(id) = small_lesson_id(unit) {
            payload["lessonVideoId"] = remote_id(id);
        }
        let reply = signed_post(
            host,
            session,
            endpoints::LEARNING_BASE,
            endpoints::POPUP_SAVE_PATH,
            payload,
            &key,
            origin,
            Stage::SaveStudy,
        )?;
        match number(&reply["code"]) {
            Some(0) => {}
            Some(401 | 403 | 5008) => {
                return Err(ModuleError::login_expired("智慧树登录已失效，请重新扫码"));
            }
            Some(code) => {
                return Err(ModuleError::protocol_changed(format!(
                    "保存弹题答案返回了未预期的 code {code}"
                )));
            }
            None => return Err(ModuleError::protocol_changed("保存弹题答案响应缺少 code")),
        }
    }
    Ok(vec![])
}

/// `saveDatabaseIntervalTimeV2`: one chunk of watched seconds with its watch points.
#[allow(clippy::too_many_arguments)]
pub fn save(
    host: &dyn Host,
    keys: &Keys,
    session: &str,
    account_raw: &str,
    course: &Course,
    prepared: &Prepared,
    seconds: i64,
) -> Result<SaveResult, ModuleError> {
    if !(1..=300).contains(&seconds) {
        return Err(ModuleError::invalid_params("保存时长超出范围"));
    }
    let key = protocol_key(host, keys, session, 11)?;
    let origin = "https://studyvideoh5.zhihuishu.com";
    if prepared.limited {
        study_limit(
            host,
            session,
            &course.recruit_id,
            &prepared.school_id,
            &key,
            origin,
        )?;
    }
    let after = prepared.position + seconds;
    let points = watch_points(prepared.position, seconds);
    let fields = vec![
        course.recruit_id.clone(),
        prepared.lesson_id.clone(),
        prepared
            .small_lesson_id
            .clone()
            .unwrap_or_else(|| "0".to_owned()),
        prepared.media_id.clone(),
        prepared.chapter_id.clone(),
        "0".to_owned(),
        seconds.to_string(),
        (prepared.total + seconds).to_string(),
        position_text(after),
        format!("{account_raw}zhs"),
    ];
    let input = json!({
        "ewssw": points,
        "sdsew": encode_study_fields(&fields),
        "zwsds": base64::engine::general_purpose::STANDARD.encode(prepared.record_id.as_bytes()),
        "courseId": remote_id(&course.id),
    });
    let reply = signed_post(
        host,
        session,
        endpoints::LEARNING_BASE,
        endpoints::STUDY_SAVE_PATH,
        input,
        &key,
        origin,
        Stage::SaveStudy,
    )?;
    match number(&reply["code"]) {
        Some(0) => Ok(SaveResult::Saved),
        Some(401 | 403 | 5008) => Err(ModuleError::login_expired("智慧树登录已失效，请重新扫码")),
        Some(-9) => Err(ModuleError::new(
            "platform_rejected",
            "课程当前不开放学习，继续观看可能不计进度，请到官方页面核对",
        )),
        Some(-10) => Err(ModuleError::new(
            "platform_rejected",
            "平台检测到另一个学习页面，请关闭其他播放后重新尝试",
        )),
        Some(-12) if flag(&reply["data"]["isFace"])? => Err(ModuleError::verification_required(
            "智慧树要求人脸验证，请先到官方页面处理",
        )),
        Some(-12) => Ok(SaveResult::Captcha(captcha_ev(
            course,
            &prepared.chapter_id,
            &prepared.lesson_id,
            &prepared.media_id,
            prepared.small_lesson_id.as_deref(),
        ))),
        Some(code) => Err(ModuleError::protocol_changed(format!(
            "保存学习记录返回了未预期的 code {code}"
        ))),
        None => Err(ModuleError::protocol_changed("保存学习记录响应缺少 code")),
    }
}

/// The learning gateway binds a validation token to these five video context fields.
/// Keep the order and values identical to the official `evLog` built after
/// `prelearningNoteApi`: recruit, lesson, small lesson, video, chapter.
fn captcha_ev(
    course: &Course,
    chapter: &str,
    lesson: &str,
    media: &str,
    small: Option<&str>,
) -> String {
    encode_study_fields(&[
        course.recruit_id.clone(),
        lesson.to_owned(),
        small.unwrap_or("0").to_owned(),
        media.to_owned(),
        chapter.to_owned(),
    ])
}

pub fn confirm_captcha(
    host: &dyn Host,
    keys: &Keys,
    session: &str,
    token: &str,
    ev: &str,
) -> Result<(), ModuleError> {
    let key = protocol_key(host, keys, session, 11)?;
    let reply = signed_post(
        host,
        session,
        endpoints::LEARNING_BASE,
        endpoints::STUDY_VALIDATE_SLIDE_PATH,
        json!({"token":token,"ev":ev,"checkType":1}),
        &key,
        "https://studyvideoh5.zhihuishu.com",
        Stage::OpenStudy,
    )?;
    let data = learning_data(&reply, endpoints::STUDY_VALIDATE_SLIDE_PATH)?;
    if number(&data["status"]) == Some(200) && data["pass"] == true {
        return Ok(());
    }
    Err(ModuleError::verification_required(
        "智慧树未确认滑块验证通过，请在官方页面完成后继续",
    ))
}

/// The client sends one point every two seconds: `(total + tick) / 5 + 2`.
fn watch_points(total: i64, seconds: i64) -> String {
    let mut points = String::new();
    for tick in 1..=seconds {
        if tick % 2 != 0 {
            continue;
        }
        if points.is_empty() {
            points.push_str("0,1,");
        } else {
            points.push(',');
        }
        points.push_str(&((total + tick) / 5 + 2).to_string());
    }
    if points.is_empty() {
        points.push_str("0,1");
    }
    points
}

fn encode_study_fields(fields: &[String]) -> String {
    fields
        .join(";")
        .as_bytes()
        .iter()
        .enumerate()
        .map(|(index, byte)| format!("{:02x}", byte ^ b"zzpttjd"[index % 7]))
        .collect()
}

fn position_text(seconds: i64) -> String {
    format!(
        "0{}:{:02}:{:02}",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    )
}

fn flag(value: &Value) -> Result<bool, ModuleError> {
    match value {
        Value::Null => Ok(false),
        Value::Bool(value) => Ok(*value),
        _ => match number(value) {
            Some(0) => Ok(false),
            Some(1) => Ok(true),
            _ => Err(ModuleError::protocol_changed("布尔字段不是 0/1")),
        },
    }
}

fn remote_id(id: &str) -> Value {
    id.parse::<u64>()
        .map_or_else(|_| json!(id), |value| json!(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::FetchResponse;

    #[test]
    fn watch_points_follow_the_client_recipe() {
        assert_eq!(watch_points(0, 30), "0,1,2,2,3,3,4,4,4,5,5,6,6,6,7,7,8");
        assert_eq!(watch_points(120, 4), "0,1,26,26");
    }

    #[test]
    fn study_fields_are_hex_xored_with_the_public_mask() {
        let encoded = encode_study_fields(&["1".to_owned(), "2".to_owned()]);
        // '1' ^ 'z' = 0x4b, ';' ^ 'z' = 0x41, '2' ^ 'p' = 0x42
        assert_eq!(encoded, "4b4142");
    }

    #[test]
    fn position_text_is_zero_padded() {
        assert_eq!(position_text(0), "00:00:00");
        assert_eq!(position_text(3661), "01:01:01");
    }

    fn prepared_at(position: i64) -> Prepared {
        Prepared {
            record_id: "record-1".to_owned(),
            position,
            total: position,
            duration: 600,
            media_id: "v1".to_owned(),
            chapter_id: "c1".to_owned(),
            lesson_id: "l1".to_owned(),
            small_lesson_id: None,
            limited: false,
            school_id: json!(0),
            questions: vec![(120, json!("q1")), (300, json!("q2"))],
            blocking_at: Some(480),
        }
    }

    #[test]
    fn boundary_is_the_next_question_then_the_overlay_then_the_end() {
        assert_eq!(prepared_at(0).boundary(), 120);
        assert_eq!(prepared_at(120).boundary(), 300, "已答的弹题不再挡路");
        assert_eq!(prepared_at(400).boundary(), 480, "下一次是遮挡层");
        assert_eq!(prepared_at(480).boundary(), 600, "遮挡层之后只剩片尾");
    }

    #[test]
    fn only_due_questions_are_reported() {
        let prepared = prepared_at(150);
        let due = prepared.due_questions();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].1, json!("q1"));
    }

    #[test]
    fn shared_course_rows_map_to_the_contract_type() {
        let row = json!({
            "courseId": 1000123456u64,
            "recruitId": 2000987654u64,
            "courseName": "大学生心理健康教育",
            "studyPercent": 37.5,
            "secret": "entry-token"
        });
        let course = shared_course(&row).unwrap();
        assert_eq!(course.id, "1000123456");
        assert_eq!(course.recruit_id, "2000987654");
        assert_eq!(course.kind, "shared");
        assert_eq!(course.family.as_deref(), Some("shared"));
        assert_eq!(course.progress, Some(37.5));
        assert!(course.outline_available, "课程带 secret 才能打开目录");
    }

    #[test]
    fn ai_course_rows_map_to_a_smart_course_with_an_outline() {
        let row = json!({
            "courseId": "3000112233",
            "classId": "4000445566",
            "courseName": "人工智能导论",
            "schedule": 12.0
        });
        let course = ai_course(&row).expect("AI 课程应被识别");
        assert_eq!(course.kind, "smart");
        assert_eq!(course.family.as_deref(), Some("smart"));
        assert_eq!(course.progress, Some(12.0));
        assert!(course.outline_available);
    }

    #[test]
    fn url_component_encoding_is_strict() {
        assert_eq!(encode_component("a b/c+d="), "a%20b%2Fc%2Bd%3D");
    }
    struct FeatureHost {
        replies: Mutex<std::collections::VecDeque<Value>>,
        requests: Mutex<Vec<FetchRequest>>,
        state: Mutex<HashMap<(String, String), Value>>,
    }

    struct CaptchaHost {
        network: FeatureHost,
        snapshots: Mutex<std::collections::VecDeque<Value>>,
        calls: Mutex<Vec<(String, Value)>>,
        now: std::sync::atomic::AtomicI64,
        available: bool,
    }
    impl CaptchaHost {
        fn new(replies: Vec<Value>, snapshots: Vec<Value>) -> Self {
            Self {
                network: FeatureHost {
                    replies: Mutex::new(replies.into()),
                    requests: Mutex::new(vec![]),
                    state: Mutex::new(HashMap::from([(
                        ("catalog".into(), "entries".into()),
                        json!({"1/2":"entry"}),
                    )])),
                },
                snapshots: Mutex::new(snapshots.into()),
                calls: Mutex::new(vec![]),
                now: std::sync::atomic::AtomicI64::new(1000),
                available: true,
            }
        }
    }
    impl Host for CaptchaHost {
        fn service(&self, method: &str, params: Value) -> Result<Value, ModuleError> {
            self.calls.lock().unwrap().push((method.into(), params));
            match method {
                "cw.captcha.status" => Ok(json!({"available":self.available})),
                "cw.browser.open" => Ok(json!({"page":"captcha-page"})),
                "cw.browser.evaluate" => Ok(self
                    .snapshots
                    .lock()
                    .unwrap()
                    .pop_front()
                    .expect("预置页面响应")),
                "cw.browser.image" => Ok(json!({"image":"c3ludGhldGlj","mime":"image/png"})),
                "cw.captcha.slide_match" => {
                    Ok(json!({"target":[122,90,179,138],"target_x":2,"target_y":89}))
                }
                "cw.browser.mouse" | "cw.browser.close" => Ok(json!({})),
                _ => Err(ModuleError::unsupported(method)),
            }
        }
        fn now(&self) -> Result<i64, ModuleError> {
            Ok(self.now.load(std::sync::atomic::Ordering::SeqCst))
        }
        fn fetch(&self, request: &FetchRequest) -> Result<FetchResponse, ModuleError> {
            self.network.fetch(request)
        }
        fn state_get(&self, n: &str, k: &str, s: &str) -> Result<Value, ModuleError> {
            self.network.state_get(n, k, s)
        }
        fn state_set(&self, n: &str, k: &str, v: &Value, s: &str) -> Result<(), ModuleError> {
            self.network.state_set(n, k, v, s)
        }
    }

    fn captcha_course_unit() -> (Course, Unit) {
        (serde_json::from_value(json!({"id":"1","recruit_id":"2","name":"合成课程","kind":"shared","outline_available":true})).unwrap(),
         serde_json::from_value(json!({"id":"20","name":"合成视频","chapter_id":"10","chapter_name":"第一章","lesson_id":"20","kind":"video","duration_seconds":600,"watched_seconds":0,"state":"not_started","video_id":"99","selectable":true})).unwrap())
    }
    fn captcha_puzzle() -> Value {
        json!({"state":"puzzle","background_url":"https://synthetic.invalid/bg.png","target_url":"https://synthetic.invalid/piece.png",
            "width":160,"natural_width":320,"piece_left":5,"slider_x":30,"slider_y":200})
    }

    #[test]
    fn captcha_ev_uses_the_official_video_context_fields() {
        let (course, unit) = captcha_course_unit();
        assert_eq!(
            captcha_ev(&course, &unit.chapter_id, &unit.lesson_id, "99", Some("77")),
            encode_study_fields(&[
                "2".to_owned(),
                "20".to_owned(),
                "77".to_owned(),
                "99".to_owned(),
                "10".to_owned(),
            ])
        );
    }
    fn learning_policy() -> Value {
        json!({"code":0,"data":{"recruitId":2,"studyStatus":0,"courseInfo":{"courseId":1,"canJumpChapter":true,"type":0}}})
    }
    fn captcha_api() -> super::super::Zhs {
        let api = super::super::Zhs::new(super::super::Settings {
            source: super::super::Source::Live,
        });
        api.keys.put("s", 11, b"0123456789abcdef".to_vec());
        api
    }

    #[test]
    fn captcha_on_open_matches_scaled_gap_and_requires_gateway_confirmation() {
        let api = captcha_api();
        let (course, unit) = captcha_course_unit();
        let host = CaptchaHost::new(
            vec![
                learning_policy(),
                json!({"code":0,"data":{"questionPoint":[]}}),
                json!({"code":0,"data":{"isSlide":true}}),
                json!({"code":0,"data":{"status":200,"pass":true}}),
                learning_policy(),
                json!({"code":0,"data":{"questionPoint":[]}}),
                json!({"code":0,"data":{"studiedLessonDto":{"id":7,"learnTimeSec":0,"studyTotalTime":0}}}),
            ],
            vec![
                captcha_puzzle(),
                json!({"state":"verified","token":"synthetic-validation-token"}),
            ],
        );
        let options = json!({"execution_id":"job/1"});
        let mut result = api
            .live_step(&host, "s", &course, &unit, None, &options)
            .unwrap();
        assert_eq!(result["checkpoint"]["phase"], "captcha");
        for _ in 0..3 {
            result = api
                .live_step(
                    &host,
                    "s",
                    &course,
                    &unit,
                    Some(&result["checkpoint"]),
                    &options,
                )
                .unwrap();
        }
        assert_eq!(result["checkpoint"]["phase"], "captcha_retry");
        assert!(!result.to_string().contains("synthetic-validation-token"));
        let calls = host.calls.lock().unwrap();
        let drag = &calls
            .iter()
            .find(|(method, _)| method == "cw.browser.mouse")
            .unwrap()
            .1;
        assert_eq!(
            drag["action"]["to"]["x"], 95.5,
            "透明边框和 CSS 缩放后应拖动 55 像素，并补偿 10.5 像素的滑块启动阈值"
        );
        assert_eq!(calls.last().unwrap().0, "cw.browser.close");
        drop(calls);
        let requests = host.network.requests.lock().unwrap();
        assert_eq!(
            requests
                .iter()
                .filter(|r| r.url.ends_with("validateSlideToken"))
                .count(),
            1
        );
        assert!(
            requests
                .iter()
                .all(|r| !r.url.ends_with("saveDatabaseIntervalTimeV2"))
        );
        drop(requests);
        result = api
            .live_step(
                &host,
                "s",
                &course,
                &unit,
                Some(&result["checkpoint"]),
                &options,
            )
            .unwrap();
        assert_eq!(result["checkpoint"]["save_after"], 1030);
        assert_eq!(result["checkpoint"]["position"], 0);
    }

    #[test]
    fn captcha_on_save_does_not_advance_progress_or_credit_time_spent_verifying() {
        let api = captcha_api();
        let (course, unit) = captcha_course_unit();
        let host = CaptchaHost::new(
            vec![
                json!({"code":-12}),
                json!({"code":0,"data":{"status":200,"pass":true}}),
                json!({"code":-12}),
            ],
            vec![
                captcha_puzzle(),
                json!({"state":"verified","token":"synthetic-token"}),
            ],
        );
        let mut cp = prepared_at(0).to_checkpoint();
        cp["save_after"] = json!(900);
        cp["execution_id"] = json!("job/1");
        let options = json!({"execution_id":"job/1"});
        let mut result = api
            .live_step(&host, "s", &course, &unit, Some(&cp), &options)
            .unwrap();
        assert_eq!(result["checkpoint"]["resume"]["position"], 0);
        for _ in 0..3 {
            result = api
                .live_step(
                    &host,
                    "s",
                    &course,
                    &unit,
                    Some(&result["checkpoint"]),
                    &options,
                )
                .unwrap();
        }
        host.now.store(1050, std::sync::atomic::Ordering::SeqCst);
        result = api
            .live_step(
                &host,
                "s",
                &course,
                &unit,
                Some(&result["checkpoint"]),
                &options,
            )
            .unwrap();
        assert_eq!(result["checkpoint"]["save_after"], 1080);
        assert_eq!(result["checkpoint"]["position"], 0);
        host.now.store(1080, std::sync::atomic::Ordering::SeqCst);
        result = api
            .live_step(
                &host,
                "s",
                &course,
                &unit,
                Some(&result["checkpoint"]),
                &options,
            )
            .unwrap();
        assert_eq!(result["status"], "needs_human", "同一拒绝最多自动验证一次");
        assert_eq!(result["checkpoint"]["position"], 0);
        assert_eq!(
            host.calls
                .lock()
                .unwrap()
                .iter()
                .filter(|(method, _)| method == "cw.browser.open")
                .count(),
            1
        );
    }

    #[test]
    fn captcha_rejection_unsupported_challenge_and_missing_backend_pause_and_close() {
        let api = captcha_api();
        let (course, unit) = captcha_course_unit();
        for scenario in ["rejected", "unsupported", "unavailable", "timeout"] {
            let mut host = CaptchaHost::new(
                vec![json!({"code":0,"data":{"status":200,"pass":false}})],
                vec![if scenario == "unsupported" {
                    json!({"state":"unsupported"})
                } else {
                    json!({"state":"verified","token":"synthetic-token"})
                }],
            );
            host.available = scenario != "unavailable";
            let mut result = super::super::captcha::start(
                "context".into(),
                Value::Null,
                "job/1",
                &course,
                &unit,
                1000,
            );
            result = api
                .live_step(
                    &host,
                    "s",
                    &course,
                    &unit,
                    Some(&result["checkpoint"]),
                    &json!({"execution_id":"job/1"}),
                )
                .unwrap();
            if scenario != "unavailable" {
                if scenario == "timeout" {
                    host.now.store(1061, std::sync::atomic::Ordering::SeqCst);
                }
                result = api
                    .live_step(
                        &host,
                        "s",
                        &course,
                        &unit,
                        Some(&result["checkpoint"]),
                        &json!({"execution_id":"job/1"}),
                    )
                    .unwrap();
                assert_eq!(
                    host.calls.lock().unwrap().last().unwrap().0,
                    "cw.browser.close"
                );
            }
            assert_eq!(result["status"], "needs_human", "{scenario}");
            assert!(!result.to_string().contains("synthetic-token"));
        }
    }

    #[test]
    fn face_verification_never_opens_the_automatic_captcha_solver() {
        let api = captcha_api();
        let (course, unit) = captcha_course_unit();
        let host = CaptchaHost::new(
            vec![
                learning_policy(),
                json!({"code":0,"data":{"questionPoint":[]}}),
                json!({"code":0,"data":{"isFace":true,"isSlide":true}}),
            ],
            vec![],
        );
        let error = api
            .live_step(
                &host,
                "s",
                &course,
                &unit,
                None,
                &json!({"execution_id":"job/1"}),
            )
            .unwrap_err();
        assert_eq!(error.code, "platform_verification_required");
        assert!(host.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn explicit_prelearning_captcha_rejection_preserves_the_video_context() {
        let api = captcha_api();
        let (course, unit) = captcha_course_unit();
        let host = CaptchaHost::new(
            vec![
                learning_policy(),
                json!({"code":0,"data":{"questionPoint":[]}}),
                json!({"code":-12}),
            ],
            vec![],
        );
        let result = api
            .live_step(
                &host,
                "s",
                &course,
                &unit,
                None,
                &json!({"execution_id":"job/1"}),
            )
            .unwrap();
        assert_eq!(result["checkpoint"]["phase"], "captcha");
        assert_eq!(
            result["checkpoint"]["ev"],
            captcha_ev(&course, "10", "20", "99", None)
        );
        assert!(host.calls.lock().unwrap().is_empty());
    }
    impl Host for FeatureHost {
        fn now(&self) -> Result<i64, ModuleError> {
            Ok(1000)
        }
        fn fetch(&self, r: &FetchRequest) -> Result<FetchResponse, ModuleError> {
            self.requests.lock().unwrap().push(r.clone());
            let v = self.replies.lock().unwrap().pop_front().expect("预置响应");
            Ok(FetchResponse {
                status: 200,
                headers: Default::default(),
                body: v.to_string(),
            })
        }
        fn state_get(&self, n: &str, k: &str, _s: &str) -> Result<Value, ModuleError> {
            Ok(self
                .state
                .lock()
                .unwrap()
                .get(&(n.into(), k.into()))
                .cloned()
                .unwrap_or(Value::Null))
        }
        fn state_set(&self, n: &str, k: &str, v: &Value, _s: &str) -> Result<(), ModuleError> {
            self.state
                .lock()
                .unwrap()
                .insert((n.into(), k.into()), v.clone());
            Ok(())
        }
        fn log(&self, _l: &str, _m: &str, _v: Value) {}
    }

    #[test]
    fn shared_study_does_not_send_a_parent_lesson_as_a_small_lesson() {
        use aes::cipher::{BlockDecryptMut, KeyIvInit, block_padding::Pkcs7};
        const KEY: &[u8; 16] = b"0123456789abcdef";
        for small in [false, true] {
            let host = FeatureHost {
                replies: Mutex::new(std::collections::VecDeque::from([
                    json!({"code":0,"data":{"recruitId":2,"studyStatus":0,"courseInfo":{"courseId":1,"canJumpChapter":true,"type":0}}}),
                    json!({"code":0,"data":{"questionPoint":[]}}),
                    json!({"code":0,"data":{"isSlide":false,"isFace":false,"studiedLessonDto":{"id":7,"learnTimeSec":0,"studyTotalTime":0}}}),
                    json!({"code":0,"data":{"lessonTestQuestionUseInterfaceDtos":[{"testQuestion":{"questionId":40,"questionOptions":[{"id":50,"result":1},{"id":51,"result":0}]}}]}}),
                    json!({"code":0}),
                ])),
                requests: Mutex::new(vec![]),
                state: Mutex::new(HashMap::new()),
            };
            let keys = Keys::default();
            keys.put("s", 11, KEY.to_vec());
            let course: Course = serde_json::from_value(json!({"id":"1","recruit_id":"2","name":"合成课程","kind":"shared","outline_available":true})).unwrap();
            let unit: Unit = serde_json::from_value(json!({"id":if small{"30"}else{"20"},"name":"合成视频","chapter_id":"10","chapter_name":"第一章","lesson_id":"20","kind":"video","duration_seconds":60,"watched_seconds":0,"state":"not_started","video_id":"99","selectable":true})).unwrap();
            let Preparation::Ready(prepared) =
                prepare(&host, &keys, "s", "", &course, "entry", &unit, false).unwrap()
            else {
                panic!("不应要求验证码")
            };
            assert_eq!(
                prepared.small_lesson_id.as_deref(),
                if small { Some("30") } else { None }
            );
            assert!(answer_popup(&host, &keys, "s", &course, &unit, &json!([40])).unwrap());
            let requests = host.requests.lock().unwrap();
            assert_eq!(requests.len(), 5);
            for request in requests.iter().skip(1) {
                let encrypted = base64::engine::general_purpose::STANDARD
                    .decode(
                        request.body.as_ref().unwrap()["secretStr"]
                            .as_str()
                            .unwrap(),
                    )
                    .unwrap();
                let bytes =
                    cbc::Decryptor::<aes::Aes128>::new_from_slices(KEY, b"1g3qqdh4jvbskb9x")
                        .unwrap()
                        .decrypt_padded_vec_mut::<Pkcs7>(&encrypted)
                        .unwrap();
                let payload: Value = serde_json::from_slice(&bytes).unwrap();
                assert_eq!(payload["lessonId"], 20);
                assert_eq!(
                    payload.get("lessonVideoId").and_then(Value::as_u64),
                    if small { Some(30) } else { None },
                    "{}",
                    request.url
                );
                if request.url.ends_with("prelearningNote") {
                    assert_eq!(payload["videoId"], 99);
                }
            }
        }
    }

    #[test]
    fn learning_permission_errors_preserve_the_failing_endpoint() {
        let reply = json!({"code":421,"message":"该账号无权限!"});
        for endpoint in [
            endpoints::STUDY_QUERY_COURSE_PATH,
            endpoints::STUDY_POPUP_PATH,
            endpoints::STUDY_OPEN_PATH,
        ] {
            let error = learning_data(&reply, endpoint).unwrap_err();
            assert_eq!(error.code, "platform_rejected");
            assert!(error.message.contains("421"));
            assert!(error.message.contains("该账号无权限"));
            assert!(error.message.contains(endpoint));
        }
    }

    #[test]
    fn smart_paper_reads_in_batches_without_reopening_or_writing_readonly() {
        use crate::zhs::features::{FeatureClient, types};
        let replies=std::iter::once(json!({"code":0,"data":{}})).chain(std::iter::once(json!({"code":0,"data":{"questionCount":8,"partSheetVos":[{"questionSheetVos":(1..=8).map(|n|json!({"questionId":n,"version":1})).collect::<Vec<_>>()}]}}))).chain((1..=8).map(|n|json!({"code":0,"data":{"questionInfo":{"id":n,"content":"题干","questionType":1,"optionVos":[{"id":"a","content":"甲"},{"id":"b","content":"乙"}],"userAnswer":"a"}}}))).collect();
        let host = FeatureHost {
            replies: Mutex::new(replies),
            requests: Mutex::new(vec![]),
            state: Mutex::new(HashMap::new()),
        };
        let keys = Keys::default();
        keys.put("s", 5, b"0123456789abcdef".to_vec());
        let client = FeatureClient {
            host: &host,
            keys: &keys,
            session: "s",
            request: "job/0",
        };
        let course:types::Course=serde_json::from_value(json!({"id":"1","recruit_id":"2","name":"课程","kind":"smart","progress":null,"outline_available":true})).unwrap();
        let paper:types::Paper=serde_json::from_value(json!({"id":"p","name":"作业","category":"作业","available":true,"done":false,"reason":"","status_label":"","score":null,"attempts_left":null,"opens_at":null,"ends_at":null,"native":{"examTestId":"3","paperId":"4","_exam":false}})).unwrap();
        let first = client.feature_open(&course, &paper).unwrap();
        assert_eq!(first.questions.len(), 6);
        assert_eq!(first.native["partial"], true);
        let second = client.feature_open(&course, &paper).unwrap();
        assert_eq!(second.questions.len(), 8);
        assert_eq!(second.native["partial"], false);
        assert_eq!(
            host.requests
                .lock()
                .unwrap()
                .iter()
                .filter(|r| r.url.ends_with("openExam"))
                .count(),
            1
        );
        assert_eq!(second.questions[0].answer, vec!["a"]);
        let mut readonly = paper.clone();
        readonly.done = true;
        let before = host.requests.lock().unwrap().len();
        let view = client.feature_open(&course, &readonly).unwrap();
        assert_eq!(view.questions.len(), 8);
        assert_eq!(host.requests.lock().unwrap().len(), before);
        assert!(
            client
                .feature_writes(&course, &view, &HashMap::new(), false)
                .is_err()
        );
        // 开卷结果未知时不能再次发送 openExam。
        host.state_set("exam_open", "1/2/p/unknown", &json!({"started":true}), "s")
            .unwrap();
        let unknown = FeatureClient {
            host: &host,
            keys: &keys,
            session: "s",
            request: "unknown",
        };
        assert!(
            unknown
                .feature_open(&course, &paper)
                .err()
                .unwrap()
                .message
                .contains("不会重复开卷")
        );
        assert_eq!(host.requests.lock().unwrap().len(), before);
    }

    #[test]
    fn shared_papers_read_existing_answers_through_the_current_gateway() {
        use crate::zhs::features::{FeatureClient, types};
        for exam in [false, true] {
            for has_answers in [0, 1, 2] {
                let mut replies = std::collections::VecDeque::from([
                    json!({"status":"200","rt":{"schoolId":8}}),
                    json!({"status":"200","code":1,"rt":{"examBase":{"questionCount":1,"workExamParts":[{"questionDtos":[{"eid":"q1","questionType":1,"name":"合成题干","questionOptions":[{"id":"a","content":"甲"},{"id":"b","content":"乙"}]}]}]}}}),
                    json!({"status":"200","rt":has_answers}),
                ]);
                if has_answers == 1 {
                    replies.push_back(json!({"status":"200","rt":{"q1":{"answer":"a"}}}));
                }
                let host = FeatureHost {
                    replies: Mutex::new(replies),
                    requests: Mutex::new(vec![]),
                    state: Mutex::new(HashMap::new()),
                };
                let keys = Keys::default();
                keys.put("s", 3, b"0123456789abcdef".to_vec());
                let client = FeatureClient {
                    host: &host,
                    keys: &keys,
                    session: "s",
                    request: "job/0",
                };
                let course: types::Course = serde_json::from_value(json!({"id":"1","recruit_id":"2","name":"合成课程","kind":"shared","progress":null,"outline_available":true})).unwrap();
                let paper: types::Paper = serde_json::from_value(json!({"id":"p","name":"合成试卷","category":"作业","available":true,"done":false,"reason":"","status_label":"","score":null,"attempts_left":null,"opens_at":null,"ends_at":null,"native":{"_exam_id":"3","_student":"4","_exam":exam}})).unwrap();
                let opened = client.feature_open(&course, &paper);
                if has_answers == 2 {
                    assert_eq!(opened.err().unwrap().code, "platform_protocol_changed");
                    assert_eq!(host.requests.lock().unwrap().len(), 3);
                    continue;
                }
                let attempt = opened.unwrap();
                assert_eq!(attempt.questions.len(), 1);
                assert_eq!(
                    attempt.questions[0].answer,
                    if has_answers == 1 { vec!["a"] } else { vec![] }
                );
                let requests = host.requests.lock().unwrap();
                let base = if exam {
                    "https://taurusexam-api.zhihuishu.com/taurusExam/gateway/t/v1"
                } else {
                    "https://studentexam-api.zhihuishu.com/studentExam/gateway/t/v1"
                };
                assert_eq!(requests[2].url, format!("{base}/answer/hasAnswer"));
                if has_answers == 1 {
                    assert_eq!(
                        requests[3].url,
                        format!(
                            "{base}/answer/{}",
                            if exam {
                                "getStuAnswerInfo"
                            } else {
                                "getStuAnswerInfoNew"
                            }
                        )
                    );
                }
                assert_eq!(requests.len(), if has_answers == 1 { 4 } else { 3 });
                assert!(
                    requests
                        .iter()
                        .skip(2)
                        .all(|r| matches!(r.stage, Some(Stage::Read)) && !r.commit)
                );
            }
        }
    }

    #[test]
    fn shared_homework_decodes_object_types_and_saves_numeric_type_ids() {
        use crate::zhs::features::{FeatureClient, types};
        let rows = [14, 2, 1].into_iter().enumerate().map(|(i, id)| json!({"eid":format!("q{i}"),"questionType":{"id":id,"name":"合成题型"},"name":"合成题干","questionOptions":[{"id":"a","content":"甲"},{"id":"b","content":"乙"}]})).collect::<Vec<_>>();
        let host = FeatureHost {
            replies: Mutex::new(std::collections::VecDeque::from([
                json!({"status":200,"rt":{"schoolId":8}}),
                json!({"status":200,"rt":{"examBase":{"questionCount":3,"workExamParts":[{"questionDtos":rows}]}}}),
                json!({"status":200,"rt":0}),
            ])),
            requests: Mutex::new(vec![]),
            state: Mutex::new(HashMap::new()),
        };
        let keys = Keys::default();
        keys.put("s", 3, b"0123456789abcdef".to_vec());
        let client = FeatureClient {
            host: &host,
            keys: &keys,
            session: "s",
            request: "synthetic/0",
        };
        let course: types::Course = serde_json::from_value(json!({"id":"1","recruit_id":"2","name":"合成课程","kind":"shared","progress":null,"outline_available":true})).unwrap();
        let paper: types::Paper = serde_json::from_value(json!({"id":"p","name":"合成作业","category":"作业","available":true,"done":false,"reason":"","status_label":"","score":null,"attempts_left":null,"opens_at":null,"ends_at":null,"native":{"_exam_id":"3","_student":"4","_exam":false}})).unwrap();
        let attempt = client.feature_open(&course, &paper).unwrap();
        assert_eq!(
            attempt
                .questions
                .iter()
                .map(|q| q.kind.as_str())
                .collect::<Vec<_>>(),
            vec!["single", "multiple", "single"]
        );
        let answers = attempt
            .questions
            .iter()
            .map(|q| (q.id.clone(), vec!["a".to_owned()]))
            .collect::<HashMap<_, _>>();
        let writes = client
            .feature_writes(&course, &attempt, &answers, false)
            .unwrap();
        assert_eq!(writes.len(), 3);
        for (request, expected) in writes.iter().zip([14, 2, 1]) {
            let items: serde_json::Value =
                serde_json::from_str(request.payload["stuExamAnswer"].as_str().unwrap()).unwrap();
            assert_eq!(items[0]["questionType"], expected);
            assert_eq!(request.stage, "draft");
        }
    }
}
