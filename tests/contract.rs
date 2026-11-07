//! Contract tests against a mock host: no network, no account, no child process.
//!
//! These double as the reference for module API v1 conformance:
//! descriptor/manifest agreement, wire types, fixture study semantics, typed errors
//! and a real NDJSON round trip through the reader/dispatch functions.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Write;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use courseflow_zhs::error::ModuleError;
use courseflow_zhs::host::{FetchRequest, FetchResponse, Host};
use courseflow_zhs::platform::{self, PlatformApi};
use courseflow_zhs::rpc::{self, Handler, RpcClient};
use courseflow_zhs::zhs::{Settings, Source, Zhs};

// --- mock host --------------------------------------------------------------------

#[derive(Default)]
struct MockHost {
    now: AtomicI64,
    responses: Mutex<VecDeque<Result<FetchResponse, ModuleError>>>,
    fetches: Mutex<Vec<FetchRequest>>,
    state: Mutex<HashMap<(String, String), Value>>,
    logs: Mutex<Vec<(String, String)>>,
    browser_snapshot: Mutex<Value>,
    browser_calls: Mutex<Vec<String>>,
}

impl MockHost {
    fn at(now: i64) -> Self {
        let host = Self::default();
        host.now.store(now, Ordering::SeqCst);
        host
    }

    fn respond(&self, response: FetchResponse) {
        self.responses.lock().unwrap().push_back(Ok(response));
    }

    fn respond_error(&self, error: ModuleError) {
        self.responses.lock().unwrap().push_back(Err(error));
    }

    fn fetches(&self) -> Vec<FetchRequest> {
        self.fetches.lock().unwrap().clone()
    }
}

impl Host for MockHost {
    fn service(&self, method: &str, _params: Value) -> Result<Value, ModuleError> {
        self.browser_calls.lock().unwrap().push(method.to_owned());
        match method {
            "cw.browser.open" => Ok(json!({"page":"official-page"})),
            "cw.browser.evaluate" => Ok(self.browser_snapshot.lock().unwrap().clone()),
            _ => Err(ModuleError::unsupported(method)),
        }
    }

    fn fetch(&self, request: &FetchRequest) -> Result<FetchResponse, ModuleError> {
        self.fetches.lock().unwrap().push(request.clone());
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Err(ModuleError::unavailable("测试宿主没有预置响应")))
    }

    fn state_get(&self, namespace: &str, key: &str, _session: &str) -> Result<Value, ModuleError> {
        Ok(self
            .state
            .lock()
            .unwrap()
            .get(&(namespace.to_owned(), key.to_owned()))
            .cloned()
            .unwrap_or(Value::Null))
    }

    fn state_set(
        &self,
        namespace: &str,
        key: &str,
        value: &Value,
        _session: &str,
    ) -> Result<(), ModuleError> {
        self.state
            .lock()
            .unwrap()
            .insert((namespace.to_owned(), key.to_owned()), value.clone());
        Ok(())
    }

    fn now(&self) -> Result<i64, ModuleError> {
        Ok(self.now.load(Ordering::SeqCst))
    }

    fn log(&self, level: &str, message: &str, _fields: Value) {
        self.logs
            .lock()
            .unwrap()
            .push((level.to_owned(), message.to_owned()));
    }
}

// --- helpers ----------------------------------------------------------------------

fn fixture() -> Zhs {
    Zhs::new(Settings {
        source: Source::Fixture,
    })
}

fn live() -> Zhs {
    Zhs::new(Settings {
        source: Source::Live,
    })
}

fn call(
    api: &dyn PlatformApi,
    host: &dyn Host,
    method: &str,
    params: Value,
) -> Result<Value, ModuleError> {
    platform::dispatch(api, host, method, &params)
}

fn expect_error(result: Result<Value, ModuleError>) -> ModuleError {
    match result {
        Ok(value) => panic!("预期返回错误，实际返回 {value}"),
        Err(error) => error,
    }
}

fn fixture_course(api: &Zhs, host: &MockHost) -> Value {
    let result = call(api, host, "catalog.courses", json!({ "session": "s_test" })).unwrap();
    result["courses"][0].clone()
}

fn fixture_units(api: &Zhs, host: &MockHost, course: &Value) -> Vec<Value> {
    let result = call(
        api,
        host,
        "catalog.units",
        json!({ "session": "s_test", "course": course }),
    )
    .unwrap();
    result["units"].as_array().unwrap().clone()
}

// --- 1. descriptor matches the manifest ------------------------------------------

#[test]
fn descriptor_matches_manifest() {
    let manifest: Value = serde_json::from_str(include_str!("../courseflow.module.json")).unwrap();
    let api = fixture();
    let host = MockHost::default();
    let result = call(
        &api,
        &host,
        "platform.describe",
        json!({ "module_api": 1, "framework_version": "0.1.0" }),
    )
    .unwrap();

    assert_eq!(result["platform"], manifest["platform"]);
    assert_eq!(result["module"]["module_api"], manifest["moduleApi"]);
    assert_eq!(result["module"]["name"], "courseflow-zhs");
    assert_eq!(result["module"]["version"], manifest["version"]);
    assert_eq!(result["platform"]["id"], manifest["id"]);
    assert_eq!(result["platform"]["name"], manifest["name"]);
    assert_eq!(result["platform"]["login"], json!(["qr"]));
    assert_eq!(
        result["platform"]["operations"],
        json!([
            "study", "answer", "review", "meeting", "mastery", "qa", "ask", "trial"
        ])
    );
    assert_eq!(
        result["platform"]["unit_kinds"],
        json!(["video", "document"])
    );
    assert_eq!(result["platform"]["max_speed"].as_f64(), Some(1.5));

    assert_eq!(manifest["schemaVersion"], 1);
    assert_eq!(manifest["moduleApi"], 1);
    assert_eq!(manifest["runtime"]["engine"], "binary");
    assert_eq!(
        manifest["runtime"]["entry"],
        "target/release/courseflow-zhs"
    );
    // `when` keys are framework-evaluated and must use the documented vocabulary.
    let tab = &result["platform"]["presentation"]["tabs"][1];
    let action = &result["platform"]["presentation"]["actions"][0];
    assert_eq!(tab["when"]["course.kind"], "shared");
    assert_eq!(action["when"]["course.kind"], "shared");

    // The wire is documented in snake_case; no camelCase or `native` fields leak.
    for path in ["module", "platform"] {
        for key in result[path].as_object().unwrap().keys() {
            assert!(
                key.chars().all(|character| character.is_ascii_lowercase()
                    || character.is_ascii_digit()
                    || character == '_'),
                "非 snake_case 字段：{path}.{key}"
            );
        }
    }
    assert!(result.to_string().find("native").is_none());
}

// --- 2. course fixture ------------------------------------------------------------

#[test]
fn course_fixture_parses_two_snake_case_courses() {
    let api = fixture();
    let host = MockHost::default();
    let result = call(
        &api,
        &host,
        "catalog.courses",
        json!({ "session": "s_test" }),
    )
    .unwrap();
    let courses = result["courses"].as_array().unwrap();
    assert_eq!(courses.len(), 2);

    let shared = &courses[0];
    assert_eq!(shared["id"], "1000123456");
    assert_eq!(shared["recruit_id"], "2000987654");
    assert_eq!(shared["name"], "大学生心理健康教育");
    assert_eq!(shared["kind"], "shared");
    assert_eq!(shared["family"], "shared");
    assert_eq!(shared["progress"].as_f64(), Some(37.5));
    assert_eq!(shared["outline_available"], true);

    let smart = &courses[1];
    assert_eq!(smart["id"], "3000112233");
    assert_eq!(smart["recruit_id"], "4000445566");
    assert_eq!(smart["kind"], "smart");
    assert_eq!(smart["outline_available"], true);

    for course in courses {
        for key in [
            "id",
            "recruit_id",
            "name",
            "kind",
            "progress",
            "outline_available",
        ] {
            assert!(course.get(key).is_some(), "缺少字段 {key}");
        }
        assert!(course.get("native").is_none());
        for key in course.as_object().unwrap().keys() {
            assert!(
                key.chars().all(|character| character.is_ascii_lowercase()
                    || character.is_ascii_digit()
                    || character == '_'),
                "非 snake_case 字段：{key}"
            );
        }
        // The wire types deliberately have no `native` escape hatch (§5).
        assert!(course.to_string().find("native").is_none());
    }
}

// --- 3. unit fixture --------------------------------------------------------------

#[test]
fn unit_fixture_marks_only_the_document_unselectable() {
    let api = fixture();
    let host = MockHost::default();
    let course = fixture_course(&api, &host);
    let units = fixture_units(&api, &host, &course);
    assert_eq!(units.len(), 3);
    assert_eq!(
        units
            .iter()
            .map(|unit| unit["kind"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["video", "video", "document"]
    );

    let chapters: HashSet<&str> = units
        .iter()
        .map(|unit| unit["chapter_id"].as_str().unwrap())
        .collect();
    assert_eq!(chapters.len(), 2, "2 个视频 + 1 个文档应分布在 2 个章节");

    for unit in &units {
        assert!(unit.get("native").is_none());
        for key in [
            "id",
            "name",
            "chapter_id",
            "chapter_name",
            "lesson_id",
            "kind",
            "duration_seconds",
            "watched_seconds",
            "state",
            "video_id",
            "selectable",
        ] {
            assert!(unit.get(key).is_some(), "缺少字段 {key}");
        }
        let is_document = unit["kind"] == "document";
        assert_eq!(unit["selectable"].as_bool(), Some(!is_document));
        if is_document {
            assert_eq!(unit["duration_seconds"].as_i64(), Some(0));
            assert!(
                unit["selectable_note"]
                    .as_str()
                    .is_some_and(|note| !note.is_empty())
            );
        } else {
            assert!(unit["selectable_note"].is_null());
            assert!(
                unit["duration_seconds"].as_i64().unwrap() > 0,
                "视频单元应有时长"
            );
        }
        for key in unit.as_object().unwrap().keys() {
            assert!(
                key.chars().all(|character| character.is_ascii_lowercase()
                    || character.is_ascii_digit()
                    || character == '_'),
                "非 snake_case 字段：{key}"
            );
        }
    }
}

// --- 4. study.step semantics ------------------------------------------------------

#[test]
fn study_step_is_monotonic_idempotent_and_completes() {
    let api = fixture();
    let host = MockHost::default();
    let course = fixture_course(&api, &host);
    let units = fixture_units(&api, &host, &course);
    let video = units
        .iter()
        .find(|unit| unit["id"] == "9001")
        .unwrap()
        .clone();
    assert_eq!(video["duration_seconds"].as_i64(), Some(480));

    let step = |checkpoint: Value| {
        call(
            &api,
            &host,
            "study.step",
            json!({
                "session": "s_test",
                "course": course,
                "unit": video,
                "checkpoint": checkpoint,
                "options": {},
            }),
        )
        .unwrap()
    };

    let first = step(Value::Null);
    assert_eq!(first["status"], "in_progress");
    assert_eq!(first["progress"]["watched_seconds"].as_i64(), Some(60));
    assert_eq!(first["wait_ms"].as_u64(), Some(1000));
    assert_eq!(first["checkpoint"], json!({ "seconds": 60 }));

    // Replaying the same checkpoint yields exactly the same next state.
    let replay_a = step(json!({ "seconds": 60 }));
    let replay_b = step(json!({ "seconds": 60 }));
    assert_eq!(replay_a, replay_b);
    assert_eq!(replay_a["progress"]["watched_seconds"].as_i64(), Some(120));

    let mut checkpoint = Value::Null;
    let mut seen = Vec::new();
    let mut steps = 0;
    loop {
        steps += 1;
        assert!(steps <= 32, "480 秒的视频应在 32 步内完成");
        let result = step(checkpoint.clone());
        match result["status"].as_str().unwrap() {
            "completed" => {
                assert_eq!(result["evidence"]["watched_seconds"].as_i64(), Some(480));
                break;
            }
            "in_progress" => {
                let watched = result["progress"]["watched_seconds"].as_i64().unwrap();
                if let Some(previous) = seen.last() {
                    assert!(watched > *previous, "watched_seconds 必须单调递增");
                }
                seen.push(watched);
                checkpoint = result["checkpoint"].clone();
            }
            other => panic!("unexpected status {other}"),
        }
    }
    assert_eq!(seen, vec![60, 120, 180, 240, 300, 360, 420]);
    assert_eq!(steps, 8);

    // The document has duration 0 and must complete on the first step.
    let document = units
        .iter()
        .find(|unit| unit["kind"] == "document")
        .unwrap()
        .clone();
    let completed = call(
        &api,
        &host,
        "study.step",
        json!({
            "session": "s_test",
            "course": course,
            "unit": document,
            "checkpoint": null,
            "options": {},
        }),
    )
    .unwrap();
    assert_eq!(completed["status"], "completed");
    assert_eq!(completed["evidence"]["watched_seconds"].as_i64(), Some(0));
}

// --- 5. unknown methods and bad params --------------------------------------------

#[test]
fn unknown_method_is_a_typed_unsupported_operation() {
    let api = fixture();
    let host = MockHost::default();
    let error = expect_error(call(&api, &host, "answer.paper", json!({})));
    assert_eq!(error.code, "unsupported_operation");
    assert!(error.message.contains("answer.paper"));

    let error = expect_error(call(&api, &host, "catalog.courses", json!({})));
    assert_eq!(error.code, "invalid_params");
    assert!(error.message.contains("session"));
}

// --- auth fixture -----------------------------------------------------------------

#[test]
fn fixture_auth_flow_is_self_contained() {
    let api = fixture();
    let host = MockHost::at(1_700_000_000);
    let begin = call(
        &api,
        &host,
        "auth.begin",
        json!({ "method": "qr", "session": "ch_test" }),
    )
    .unwrap();
    assert_eq!(begin["challenge"]["id"], "ch_test");
    assert_eq!(begin["challenge"]["kind"], "qr");
    assert_eq!(begin["challenge"]["expires_at"], 1_700_000_120);
    assert!(
        begin["challenge"]["image"]
            .as_str()
            .unwrap()
            .starts_with("data:image/png;base64,")
    );

    let poll = || {
        call(
            &api,
            &host,
            "auth.poll",
            json!({ "challenge": "ch_test", "session": "ch_test" }),
        )
        .unwrap()
    };
    assert_eq!(poll()["state"], "waiting");
    assert_eq!(poll()["state"], "connected");

    let complete = call(
        &api,
        &host,
        "auth.complete",
        json!({ "challenge": "ch_test", "session": "ch_test" }),
    )
    .unwrap();
    assert_eq!(complete["display_name"], "演示学生");
    assert_eq!(complete["account_raw"], "cf-demo-10001");

    let verify = call(&api, &host, "auth.verify", json!({ "session": "s_test" })).unwrap();
    assert_eq!(verify["display_name"], "演示学生");

    host.now.store(1_700_000_120, Ordering::SeqCst);
    assert_eq!(poll()["state"], "expired");
}

// --- live branches (mock network) -------------------------------------------------

#[test]
fn a_completed_unit_is_not_opened_again() {
    let api = live();
    let host = MockHost::default();
    let course = json!({
        "id": "1000119877",
        "recruit_id": "426135",
        "name": "国家安全教育",
        "kind": "shared",
        "kind_label": "共享课",
        "family": "shared",
        "progress": 100.0,
        "outline_available": true
    });
    let unit = json!({
        "id": "1002187213",
        "name": "第一讲",
        "chapter_id": "1001303932",
        "chapter_name": "第一章",
        "lesson_id": "1001303932",
        "kind": "video",
        "duration_seconds": 1053,
        "watched_seconds": 1079,
        "state": "completed",
        "video_id": "v1",
        "selectable": true
    });

    let result = call(
        &api,
        &host,
        "study.step",
        json!({
            "session": "s_test",
            "course": course,
            "unit": unit,
            "checkpoint": null,
            "options": {},
        }),
    )
    .unwrap();

    assert_eq!(result["status"], "completed");
    assert_eq!(result["evidence"]["already_completed"], true);
    // Asking the gateway to open a finished video is what answered 421 for a real account.
    assert!(
        host.fetches().is_empty(),
        "已完成的单元不应再向平台发起任何请求"
    );
}

#[test]
fn extension_methods_validate_context_before_network_access() {
    let api = live();
    let host = MockHost::default();

    // 已实现的方法先检查所属课程，未知方法明确拒绝。
    for method in ["catalog.papers", "exam.open", "activity.step"] {
        let error = expect_error(call(&api, &host, method, json!({ "session": "s_test" })));
        assert_eq!(error.code, "invalid_params", "{method}");
    }
    assert!(host.fetches().is_empty(), "缺少上下文时不应发起网络调用");

    assert_eq!(
        expect_error(call(
            &api,
            &host,
            "route.invoke",
            json!({"session":"s_test"})
        ))
        .code,
        "unsupported_operation"
    );

    // A live challenge that this process never issued cannot be completed; that is a typed
    // session error, not a panic and not a network call.
    let complete = expect_error(call(
        &api,
        &host,
        "auth.complete",
        json!({ "challenge": "ch_unknown", "session": "ch_unknown" }),
    ));
    assert_eq!(complete.code, "platform_login_expired");
    assert!(host.fetches().is_empty());
}

#[test]
fn live_net_fetch_errors_are_mapped_straight_through() {
    let api = live();
    let host = MockHost::default();
    host.respond_error(ModuleError::new(
        "platform_verification_required",
        "智慧树要求验证",
    ));

    let error = expect_error(call(
        &api,
        &host,
        "auth.begin",
        json!({ "method": "qr", "session": "ch_test" }),
    ));
    assert_eq!(error.code, "platform_verification_required");
    assert_eq!(error.message, "智慧树要求验证");

    let fetches = host.fetches();
    assert_eq!(fetches.len(), 1);
    assert_eq!(fetches[0].method, "GET");
    assert_eq!(fetches[0].url, "https://passport.zhihuishu.com/login");
    assert_eq!(fetches[0].session, "ch_test");
}

#[test]
fn live_course_call_starts_with_the_gateway_key_exchange() {
    let api = live();
    let host = MockHost::default();

    // The real bootstrap public key has no private counterpart in this repository, so the mock
    // cannot forge a valid `rt.sl`. What this test pins is the shape of the first request and the
    // typed failure — the parsing itself is covered by the unit tests in live.rs.
    host.respond(FetchResponse {
        status: 200,
        headers: Default::default(),
        body: json!({ "rt": { "sl": "not-a-valid-signature" } }).to_string(),
    });

    let error = expect_error(call(
        &api,
        &host,
        "catalog.courses",
        json!({ "session": "s_test" }),
    ));
    assert_eq!(error.code, "platform_protocol_changed");

    let fetches = host.fetches();
    assert_eq!(fetches.len(), 1);
    assert_eq!(fetches[0].method, "GET");
    assert!(
        fetches[0]
            .url
            .starts_with("https://appcomm-user.zhihuishu.com/app-commserv-user/c/has?uid="),
        "第一个请求应是网关密钥交换：{}",
        fetches[0].url
    );
    // The RSA ciphertext is URL-encoded base64, never a raw JSON body.
    assert!(
        fetches[0].url.contains("%2B")
            || fetches[0].url.contains("%2F")
            || fetches[0].url.contains("%3D")
    );
    assert_eq!(fetches[0].session, "s_test");
    // Omitted stage means `read` per the contract; the request must never be a write stage.
    assert!(
        fetches[0].stage.is_none() || fetches[0].stage == Some(courseflow_zhs::host::Stage::Read)
    );
}

/// A `Write` sink shared with the test so the NDJSON round trip can inspect stdout.
#[derive(Clone, Default)]
struct SharedSink(Arc<Mutex<Vec<u8>>>);

impl Write for SharedSink {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn ndjson_round_trip_through_the_real_reader() {
    let api = Arc::new(fixture());
    let host = Arc::new(MockHost::at(1_700_000_000));
    let handler: Arc<Handler> = Arc::new(move |method: &str, params: &Value| {
        platform::dispatch(api.as_ref(), host.as_ref(), method, params)
    });

    let sink = SharedSink::default();
    let client = Arc::new(RpcClient::new(sink.clone()));
    let input = [
        json!({"jsonrpc":"2.0","id":1,"method":"platform.describe","params":{"module_api":1}})
            .to_string(),
        // A notification must not produce a line on stdout.
        json!({"jsonrpc":"2.0","method":"cw.log","params":{"level":"info","message":"hello"}})
            .to_string(),
        json!({"jsonrpc":"2.0","id":2,"method":"catalog.courses","params":{"session":"s_test"}})
            .to_string(),
        json!({"jsonrpc":"2.0","id":3,"method":"no.such.method","params":{}}).to_string(),
    ]
    .join("\n")
        + "\n";

    let reader = rpc::serve(client.clone(), std::io::Cursor::new(input), handler).unwrap();
    reader.join().unwrap();

    let raw = String::from_utf8(sink.0.lock().unwrap().clone()).unwrap();
    let lines: Vec<&str> = raw.lines().collect();
    assert_eq!(
        lines.len(),
        3,
        "stdout 只能有每个请求一行协议输出，实际：{raw}"
    );
    let responses: Vec<Value> = lines
        .iter()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    // Handlers run on their own threads, so completion order is not request order:
    // the framework correlates by id, and so does this test.
    let by_id: HashMap<u64, Value> = responses
        .iter()
        .map(|response| (response["id"].as_u64().unwrap(), response.clone()))
        .collect();
    assert_eq!(by_id.len(), 3);
    assert!(
        responses
            .iter()
            .all(|response| response["jsonrpc"] == "2.0")
    );

    assert_eq!(by_id[&1]["result"]["platform"]["id"], "zhs");
    assert_eq!(by_id[&2]["result"]["courses"].as_array().unwrap().len(), 2);
    assert_eq!(by_id[&3]["error"]["code"], "unsupported_operation");
}

#[test]
fn resumed_generation_waits_again_and_does_not_credit_paused_time() {
    let api = live();
    let host = MockHost::at(1000);
    let course = fixture_course(&fixture(), &host);
    let unit = fixture_units(&fixture(), &host, &course)[0].clone();
    let checkpoint = json!({"phase":"running","record_id":"record","position":30,"total":30,"questions":[],"blocking_at":null,"duration":480,"media_id":"media","chapter_id":"chapter","lesson_id":"lesson","small_lesson_id":null,"limited":false,"school_id":0,"execution_id":"old","save_after":900});
    let first = call(&api,&host,"study.step",json!({"session":"s_test","course":course,"unit":unit,"checkpoint":checkpoint,"options":{"execution_id":"new"}})).unwrap();
    assert_eq!(first["status"], "in_progress");
    assert_eq!(first["checkpoint"]["position"], 30);
    assert_eq!(first["checkpoint"]["save_after"], 1030);
    host.now.store(1010, Ordering::SeqCst);
    let next = call(&api,&host,"study.step",json!({"session":"s_test","course":course,"unit":unit,"checkpoint":first["checkpoint"],"options":{"execution_id":"new"}})).unwrap();
    assert_eq!(next["wait_ms"], 20000);
    assert_eq!(next["progress"]["watched_seconds"], 30);
    assert!(host.fetches().is_empty(), "等待真实时长前不能保存进度");
}

#[test]
fn review_completion_preserves_original_total_and_reports_budget() {
    let api = live();
    let host = MockHost::at(1000);
    let course = fixture_course(&fixture(), &host);
    let mut unit = fixture_units(&fixture(), &host, &course)[0].clone();
    unit["state"] = json!("completed");
    let checkpoint = json!({"phase":"running","record_id":"record","position":60,"total":540,"questions":[],"blocking_at":null,"duration":60,"media_id":"media","chapter_id":"chapter","lesson_id":"lesson","small_lesson_id":null,"limited":false,"school_id":0});
    let result=call(&api,&host,"study.step",json!({"session":"s_test","course":course,"unit":unit,"checkpoint":checkpoint,"options":{"operation":"review","review_seconds":60,"speedup":true}})).unwrap();
    assert_eq!(result["status"], "completed");
    assert_eq!(result["checkpoint"]["total"], 540);
    assert_eq!(result["evidence"]["reviewed_seconds"], 60);
    assert_eq!(result["evidence"]["original_state_preserved"], true);
    assert!(host.fetches().is_empty());
}

#[test]
fn popup_normalization_keeps_images_and_unknown_types_require_manual_handling() {
    use courseflow_zhs::zhs::features::{normalize_popup, types::validate_answer};
    let question=normalize_popup(&json!({"eid":"q","questionType":1,"name":"<p>题干<img src=\"https://example.zhihuishu.com/question.png\"></p>","questionOptions":[{"id":"a","content":"甲"},{"id":"b","content":"乙"}]})).unwrap();
    assert_eq!(question.kind, "single");
    assert!(question.text.contains("题干"));
    assert!(!question.text.contains("<p>"));
    assert_eq!(
        question.images,
        vec!["https://example.zhihuishu.com/question.png"]
    );
    assert!(validate_answer(&question, &["a".into()]));
    let unsupported =
        normalize_popup(&json!({"eid":"q2","questionType":999,"name":"未知题型"})).unwrap();
    assert_eq!(unsupported.kind, "unsupported");
    assert!(!validate_answer(&unsupported, &["猜测".into()]));
}

fn meeting_context(host: &MockHost) -> Value {
    let course = fixture_course(&fixture(), host);
    let key = format!(
        "{}/{}/meetings",
        course["id"].as_str().unwrap(),
        course["recruit_id"].as_str().unwrap()
    );
    host.state_set("features", &key, &json!([{"liveCourseId":"meeting","taskType":1,"taskStatus":3,"replayCourseUrl":"https://lc.zhihuishu.com/live/vod_room.html?id=synthetic"}]), "s_test").unwrap();
    json!({"session":"s_test","course":course,"operation":"meeting","item_id":"meeting","checkpoint":null,"options":{"execution_id":"run"}})
}
fn meeting_reply(host: &MockHost, completed: bool) {
    host.respond(FetchResponse {
        status: 200,
        headers: Default::default(),
        body: format!(
            "courseflow({});",
            json!({"signInStatus":if completed {2} else {1},"courseEndStatus":1,"openStatus":2})
        ),
    });
}
#[test]
fn meeting_waits_for_sso_then_requires_platform_attendance_confirmation() {
    let api = live();
    let host = MockHost::at(1000);
    let mut params = meeting_context(&host);
    *host.browser_snapshot.lock().unwrap() = json!({"auth_redirect":true});
    meeting_reply(&host, false);
    let pending = call(&api, &host, "activity.step", params.clone()).unwrap();
    assert_eq!(pending["status"], "in_progress");
    assert!(pending["detail"].as_str().unwrap().contains("登录态"));
    params["checkpoint"] = pending["checkpoint"].clone();
    host.now.store(1005, Ordering::SeqCst);
    *host.browser_snapshot.lock().unwrap() = json!({"key":"video","position":5,"percent":"10%"});
    meeting_reply(&host, false);
    let playing = call(&api, &host, "activity.step", params.clone()).unwrap();
    assert_eq!(playing["status"], "in_progress");
    assert!(playing["checkpoint"]["auth_wait_since"].is_null());
    params["checkpoint"] = playing["checkpoint"].clone();
    meeting_reply(&host, true);
    assert_eq!(
        call(&api, &host, "activity.step", params).unwrap()["status"],
        "completed"
    );
    assert_eq!(
        host.browser_calls
            .lock()
            .unwrap()
            .iter()
            .filter(|m| m.as_str() == "cw.browser.open")
            .count(),
        1
    );
}
#[test]
fn meeting_persistent_login_redirect_is_typed_expiry_instead_of_a_skipped_activity() {
    let api = live();
    let host = MockHost::at(1000);
    let mut params = meeting_context(&host);
    *host.browser_snapshot.lock().unwrap() = json!({"auth_redirect":true});
    meeting_reply(&host, false);
    let pending = call(&api, &host, "activity.step", params.clone()).unwrap();
    params["checkpoint"] = pending["checkpoint"].clone();
    host.now.store(1031, Ordering::SeqCst);
    meeting_reply(&host, false);
    assert_eq!(
        expect_error(call(&api, &host, "activity.step", params)).code,
        "platform_login_expired"
    );
}

#[test]
fn meeting_loading_timeout_is_distinct_from_playback_stall_and_preserves_retry() {
    let api = live();
    let host = MockHost::at(1000);
    let mut params = meeting_context(&host);
    *host.browser_snapshot.lock().unwrap() =
        json!({"loading":true,"phase":"history","readiness":{"history":false}});
    meeting_reply(&host, false);
    let loading = call(&api, &host, "activity.step", params.clone()).unwrap();
    assert!(loading["detail"].as_str().unwrap().contains("观看历史接口"));
    params["checkpoint"] = loading["checkpoint"].clone();
    host.now.store(1061, Ordering::SeqCst);
    meeting_reply(&host, false);
    let paused = call(&api, &host, "activity.step", params.clone()).unwrap();
    assert_eq!(paused["status"], "needs_human");
    assert!(paused["reason"].as_str().unwrap().contains("初始化超时"));
    assert_eq!(paused["diagnostic"]["phase"], "history");
    params["checkpoint"] = paused["checkpoint"].clone();
    params["options"]["execution_id"] = json!("retry");
    meeting_reply(&host, false);
    let retry = call(&api, &host, "activity.step", params).unwrap();
    assert_eq!(retry["status"], "in_progress");
    assert_eq!(retry["checkpoint"]["loading_since"], 1061);
}

#[test]
fn meeting_requires_actual_time_progress_even_when_player_flags_change() {
    let api = live();
    let host = MockHost::at(1000);
    let mut params = meeting_context(&host);
    *host.browser_snapshot.lock().unwrap() =
        json!({"key":"video","position":12,"percent":"10%","media":{"paused":false}});
    meeting_reply(&host, false);
    let playing = call(&api, &host, "activity.step", params.clone()).unwrap();
    params["checkpoint"] = playing["checkpoint"].clone();
    host.now.store(1181, Ordering::SeqCst);
    *host.browser_snapshot.lock().unwrap() =
        json!({"key":"video","position":12,"percent":"11%","media":{"paused":true}});
    meeting_reply(&host, false);
    let paused = call(&api, &host, "activity.step", params).unwrap();
    assert_eq!(paused["status"], "needs_human");
    assert!(paused["reason"].as_str().unwrap().contains("三分钟"));
    assert_eq!(paused["diagnostic"]["media"]["paused"], true);
}
