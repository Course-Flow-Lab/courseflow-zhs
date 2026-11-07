# courseflow-zhs

智慧树 (Zhihuishu) platform module for [Courseflow](../courseflow-framework). It is a
long-lived child process that speaks newline-delimited JSON-RPC 2.0 on stdin/stdout and
implements **module API v1** (`moduleApi: 1`).

The framework owns HTTP, sessions, storage and time. This process never opens a socket,
never reads a cookie, token or password, and never touches the data directory. Everything
that reaches the outside world goes through `cw.net.fetch`, `cw.state`, `cw.clock.now` and
`cw.log`.

| | |
| --- | --- |
| Platform id | `zhs` |
| Platform name | 智慧树 |
| Login | `qr` |
| Operations | `study`, `answer`, `review`, `meeting`, `mastery`, `qa`, `ask` |
| Unit kinds | `video`, `document` |
| Max speed | `1.5` |
| Manifest | [`courseflow.module.json`](courseflow.module.json) |
| Binary | `target/release/courseflow-zhs` |
| Dependencies | `serde`, `serde_json`, `rsa`, `aes`, `cbc`, `base64`, `rand` (protocol crypto only; no HTTP stack) |

## Status of this draft

**Live mode is the default.** A real 智慧树 account can scan a real QR, list its courses, open a
shared-course outline and study videos. Fixture mode remains for tests and demos without an
account (`CF_MODULE_SETTINGS={"source":"fixture"}`).

Implemented against the real platform:

- `platform.describe` (descriptor asserted equal to the manifest by the contract tests)
- `auth.begin` — GET `passport/login` (cookie seed) then the real QR image + `qrToken`
- `auth.poll` — real status mapping (`-1` waiting, `0` scanned, `1` connected, `2` expired,
  `3` cancelled)
- `auth.complete` — the Passport SSO exchange: `abnormalLogin` anti-risk check →
  `passport/login?pwd&service` (one-time, never retried) → `getLoginUserInfo`
- `auth.verify` — `getLoginUserInfo`, also the session-restore check
- `catalog.courses` — signed `queryShareCourseInfo` with pagination, plus the
  `queryStudentAICourseList` 智慧课程 list
- `catalog.units` — signed `learning/videolist`, the `login/gologin` bridge when the learning
  session is missing, and `queryStuyInfo` progress merged per video
- `study.step` — a unit the platform already reports as `completed` returns `completed` without
  any gateway call (opening a finished video is what answers `421`); otherwise
  `learning/queryCourse` policy gate, chapter gate, time-limit check,
  `popupAnswer/loadVideoPointerInfo` interaction points, `prelearningNote` open, then
  `saveDatabaseIntervalTimeV2` in 30-second chunks with the client's watch-point recipe.
  Video popups are answered from the platform's own option marks before the next save.

Protocol crypto lives in `src/zhs/crypto.rs`: the RSA bootstrap key exchange that yields a
per-session AES-128 key, and the AES-128-CBC `secretStr` request signing. Both are covered by
known-answer tests (an OpenSSL-derived AES vector and a synthetic RSA-signed block).

Still `unsupported_operation`, with a message naming what is missing:

- popup questions are answered **only** when the platform marks the correct option
  (`questionOptions[].result == 1`); an unmarked question returns `needs_human`
  (`interaction_popup`) instead of guessing, and a theme/picture/knowledge-card overlay returns
  `needs_human` (`interaction_overlay`), as the reference does
- 智慧课程 (smart) outlines and study, papers/exams, mastery, meetings, and the Q&A board
  (the reference drives a real browser for Q&A; module API v1 has no browser service and the
  manifest declares `browser: false`)

The port checklist below records each of these against the reference implementation.

## Build, test, install

Requires a Rust toolchain with edition 2021 support (verified on Rust 1.100). No network
is needed to run tests or the fixture mode.

```bash
cargo build --release
cargo test
cargo test --release
```

The release binary is `target/release/courseflow-zhs`; that exact relative path is
what the manifest's `runtime.entry` expects (the framework sets the working directory to
the module directory).

Install it into a `courseflow-framework` checkout:

```bash
cd ../courseflow-framework
npm run cli -- module add ../courseflow-zhs     # live by default
npm run cli -- module list
npm run cli -- module config zhs '{"source":"fixture"}'   # only for offline tests
```

The CLI validates the manifest, checks `moduleApi`/`engines.framework`, pins the declared
files (`courseflow.module.json` + `runtime.entry`; there are no `runtime.files`) by sha256
in `modules.lock.json`, and spawns the binary with the environment below. A directory
install copies only those declared files, not `target/`; the fixtures are embedded in the
binary, so nothing else is needed at runtime.

## Configuration: `CF_MODULE_SETTINGS`

The framework spawns the module with only `PATH`, `TZ=UTC`, `CF_MODULE_ID` and
`CF_MODULE_SETTINGS` (a JSON object from `modules.lock.json`, `{}` when unset).
`CF_MODULE_ID` is ignored by this module. The only setting is:

| Key | Values | Default | Meaning |
| --- | --- | --- | --- |
| `source` | `"fixture"` or `"live"` | `"live"` | Where platform data comes from; `fixture` is for tests and demos without an account |

- `fixture` performs **no network at all**. `catalog.*` and `study.step` are deterministic
  recorded data; login uses an embedded QR image; the only host calls are
  `cw.clock.now` (for QR expiry) and none for the other fixture methods.
- `live` attempts the real 智慧树 endpoints through `cw.net.fetch`. Only the subset
  listed above answers; everything else is a typed `unsupported_operation`.

Malformed JSON, a non-object, or an unknown `source` falls back to `fixture` with a
warning on **stderr**; the module never refuses to start because of settings.

## Run it by hand

stdout is protocol-only (one JSON object per line). Human-readable logs go to stderr.
On stdin EOF the process drains in-flight handlers for at most 5 s, flushes stdout and
exits `0`.

Describe (exactly one protocol line and nothing else on stdout):

```bash
printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"platform.describe","params":{"module_api":1,"framework_version":"0.1.0"}}' \
  | CF_MODULE_SETTINGS='{"source":"fixture"}' ./target/release/courseflow-zhs
```

Courses:

```bash
printf '%s\n' '{"jsonrpc":"2.0","id":2,"method":"catalog.courses","params":{"session":"s_test"}}' \
  | CF_MODULE_SETTINGS='{"source":"fixture"}' ./target/release/courseflow-zhs
```

A `study.step` sequence for unit `9001` (480 s); each invocation starts from the previous
checkpoint, 60 s per step:

```bash
COURSE='{"id":"1000123456","recruit_id":"2000987654","name":"大学生心理健康教育","kind":"shared","kind_label":"共享课","family":"shared","progress":37.5,"outline_available":true}'
UNIT='{"id":"9001","name":"1.1 什么是心理健康","chapter_id":"5001","chapter_name":"第一章 心理健康概述","lesson_id":"6001","kind":"video","duration_seconds":480,"watched_seconds":312,"state":"in_progress","video_id":"v1001","selectable":true}'
for CP in 'null' '{"seconds":60}' '{"seconds":120}' '{"seconds":180}' '{"seconds":240}' '{"seconds":300}' '{"seconds":360}' '{"seconds":420}'; do
  printf '{"jsonrpc":"2.0","id":1,"method":"study.step","params":{"session":"s_test","course":%s,"unit":%s,"checkpoint":%s,"options":{}}}\n' "$COURSE" "$UNIT" "$CP" \
    | CF_MODULE_SETTINGS='{"source":"fixture"}' ./target/release/courseflow-zhs
done
```

Result: seven `in_progress` steps (`watched_seconds` 60…420) and a final `completed` with
`evidence.watched_seconds = 480`.

## Methods

| Method | fixture | live | Notes |
| --- | --- | --- | --- |
| `platform.describe` | ✅ | ✅ | No `source` involved; matches the manifest exactly |
| `auth.begin` | ✅ | ✅ | Returns `{challenge:{id, kind:"qr", image, expires_at}}`; challenge id is the opaque framework session handle |
| `auth.poll` | ✅ | ✅ | fixture: 1st poll `waiting`, 2nd+ `connected`, `expired` after `expires_at`. live: maps platform status -1/0/1/2/3 → waiting/scanned/connected/expired/failed |
| `auth.complete` | ✅ | ✅ | fixture returns `演示学生` / `cf-demo-10001`; live runs `abnormalLogin` → `passport/login` → `getLoginUserInfo` and returns `{display_name, account_raw: uuid}` |
| `auth.verify` | ✅ | ✅ | fixture returns `演示学生`; live calls `getLoginUserInfo` |
| `catalog.courses` | ✅ | ✅ | live signs `queryShareCourseInfo` per page and merges `queryStudentAICourseList`; per-course `secret` entries go to module storage |
| `catalog.units` | ✅ | ✅ | live: signed `learning/videolist` (+ `login/gologin` bridge when needed) merged with `queryStuyInfo` progress |
| `study.step` | ✅ | ✅ | live: `queryCourse` gate, popup pointers, `prelearningNote`, then 30 s `saveDatabaseIntervalTimeV2` chunks; stops at an interaction point with `needs_human` |
| anything else | ❌ | ❌ | `unsupported_operation` naming the method |

Example error response (still one JSON line):

```json
{"jsonrpc":"2.0","id":4,"error":{"code":"unsupported_operation","message":"方法 answer.paper 尚未在本模块 v1 草案中实现；已实现：platform.describe、auth.begin/poll/complete/verify、catalog.courses/units、study.step"}}
```

## Fixture data and semantics

Both fixture files are embedded at compile time with `include_str!`, so the binary does
not depend on the working directory or on files shipped next to it.

### `fixtures/courses.json`

A recorded snapshot that keeps both platform responses separate:

- `shared.result.courseOpenDtos` — raw `queryShareCourseInfo` shape. Maps
  `courseId`/`recruitId`/`courseName`/`studyPercent`/`secret` to a `Course` with
  `kind: "shared"`, `kind_label: "共享课"`, `family: "shared"` and
  `outline_available = secret present`.
- `smart.rt` — raw `queryStudentAICourseList` shape. Maps
  `courseId`/`classId`/`courseName`/`schedule` to a `Course` with `kind: "smart"`,
  `kind_label: "AI课"`, no `family`, and `outline_available: false` (this draft does not
  model the AI knowledge outline).

The two courses are:

| id | kind | progress | outline_available |
| --- | --- | --- | --- |
| `1000123456` / recruit `2000987654` | `shared` | 37.5 | true |
| `3000112233` / recruit `4000445566` | `smart` | 12.0 | false |

### `fixtures/units.json`

A normalized recording of `learning/videolist` + `learning/queryStuyInfo`: two chapters,
two video units (`9001`, `9002`) and one document (`9003`). Mapping rules:

- `chapter_id`/`chapter_name` and `lesson_id` come from the enclosing chapter/lesson.
- `state` is one of `not_started`, `in_progress`, `completed`, `unknown`.
- `selectable = duration_seconds > 0`; the framework never infers this itself.
- The document has `duration_seconds: 0`, so it is the only unit with
  `selectable: false` and it carries a `selectable_note`.

### `study.step` checkpoint

`checkpoint` is opaque to the framework and stored per job. This draft defines it as
`{"seconds": n}`:

1. read `checkpoint.seconds`, defaulting to `0` when the checkpoint is `null`/missing;
2. add 60, capped at `unit.duration_seconds` (missing/negative durations are treated as
   0);
3. while below the duration, return
   `{"status":"in_progress","progress":{"watched_seconds":n},"wait_ms":1000,"checkpoint":{"seconds":n}}`;
4. on reaching the duration, return
   `{"status":"completed","evidence":{"watched_seconds":n}}`.

The function is pure, so replaying the same checkpoint returns the same next state — safe
after a framework restart. A zero-duration unit (e.g. the fixture document) completes on
the first step with `watched_seconds: 0`.

### `auth.*` in fixture mode

- The challenge id **is** the opaque `session` handle the framework passed to
  `auth.begin`; the module neither parses nor validates the prefix.
- `image` is an inline `data:image/png;base64,…` 116×116 demo QR (not a scannable code).
- `expires_at = cw.clock.now + 120`.
- Poll transition: first poll `waiting`, every poll after that `connected`; once
  `now >= expires_at`, `expired` wins even for an already-connected challenge.
- `auth.complete` returns `{"display_name":"演示学生","account_raw":"cf-demo-10001"}`;
  `auth.verify` returns `{"display_name":"演示学生"}`. The framework digests
  `account_raw`; it is stable and is not an email/cookie/token.
- Challenges live only in this process. After a module restart the framework must call
  `auth.begin` again; unknown fixture challenges are re-created lazily to keep the demo
  alive, while unknown live challenges return `platform_login_expired`.

## Protocol invariants

- **Reader thread** owns stdin. Requests are dispatched onto their **own thread**, so a
  handler may block on a host response without deadlocking the reader.
- Host responses complete a `SyncSender` held in a `Mutex<HashMap<u64, …>>`.
- stdout writes go through one `Mutex<BufWriter<Stdout>>`; each JSON line is written in
  full and flushed immediately.
- **Ids:** framework ids are echoed unchanged. Module-generated `cw.*` ids are
  monotonically increasing from 1, never reused; the directions are independent, so
  request id `1` from the framework and host-call id `1` do not collide.
- **Time budget:** a handler gets 28 s. Each host call waits at most the smaller of 25 s
  and the remaining budget; on timeout the handler returns `platform_unavailable` in time
  for the framework's 30 s limit.
- **Errors:** `cw.net.fetch` (and other host) error codes are mapped straight through.
  All module API v1 codes are interned and kept verbatim; an unknown code degrades to
  `platform_unavailable` with the original code in the message.
- **Shutdown:** stdin EOF stops the reader; in-flight handlers get up to 5 s to write
  their responses, then stdout is flushed and the process exits `0`.
- No `cw.log` notification is emitted in this draft, so a single request produces exactly
  one stdout line. Human logging always goes to stderr.
- Responses may complete **out of order** because each request has its own thread; the
  framework correlates by `id`, as JSON-RPC requires.

## Errors emitted

| Code | When |
| --- | --- |
| `unsupported_operation` | method or live capability not implemented |
| `invalid_params` | missing/blank `session`, bad `course`/`unit`/`checkpoint`/`options`, wrong `module_api` |
| `platform_login_expired` | QR challenge gone, live session rejected (401/403/5008 from platforms) |
| `platform_verification_required` | live `cw.net.fetch`/platform asks for a challenge |
| `platform_protocol_changed` | response shape/JSON/fixture shape not understood |
| `platform_unavailable` | host call timeout, non-2xx HTTP, write failure |
| `platform_rejected` | live refusals: closed course, chapter gate, study time limit, missing outline entry |
| `not_found` | reserved (not emitted yet) |

## Port checklist

Reference implementation: `../courseweave`. Paths and function names below were read from
that checkout; line numbers are as of this draft. Status is about **live** behavior unless
stated otherwise.

| 智慧树 function | Reference to read | Draft status |
| --- | --- | --- |
| QR image begin | `../courseweave/src/zhs.rs:164-202` (`qr_code`: GET `passport/login` then `qrCodeLogin/getLoginQrImg`, PNG magic check, `qrToken`) | ✅ live `auth.begin` (real PNG + `qrToken`) and fixture |
| QR state polling | `../courseweave/src/zhs.rs:204-218` (`poll`), `:594-603` (`login_state`, statuses -1/0/1/2/3) | ✅ live `auth.poll` and fixture |
| SSO login completion | `../courseweave/src/zhs.rs:220-305` (`finish_login`: `oncePassword`, `user/abnormalLogin`, `passport/login?pwd&service`, then verify) | ✅ live `auth.complete`: `abnormalLogin` → `passport/login` → `getLoginUserInfo`; one-time exchange never retried |
| Session verification | `../courseweave/src/zhs.rs:307-341` (`verify`: GET `gateway/f/v1/login/getLoginUserInfo?time=…`) | ✅ live `auth.verify` and fixture |
| Account identity | `../courseweave/src/zhs/study.rs:106-127` (`platform_uuid`/`account_key`, reads `CASLOGC` cookie) | ⚠️ deliberately not ported: a module must not read cookies. The framework returns the identity it stored at `auth.complete` through `cw.session.identity` (`account_raw` = QR `uuid`) |
| Shared course list | `../courseweave/src/zhs.rs:343-382` (`shared_courses`: `protocol_key(13)`, AES `secretStr` form, `status/pageNo/pageSize` pagination), `:546-592` (`parse_courses`) | ✅ live signed `queryShareCourseInfo` with pagination, and fixture |
| AI / smart course list | `../courseweave/src/zhs/features.rs:85-134` (`feature_courses` adds `student/queryStudentAICourseList` with `status:3`, module 13) | ✅ live `queryStudentAICourseList` merged into the course list; the smart *outline* is still unported |
| Protocol-key handshake | `../courseweave/src/zhs.rs:384-411` (`protocol_key`), `:498-537` (`bootstrap_public_key`, `recover_protocol_key`), `:539-544` (`encode_query`, AES-128-CBC) | ✅ live in `src/zhs/crypto.rs` (RSA PKCS#1 v1.5 bootstrap + AES-128-CBC `secretStr`), covered by known-answer tests |
| Shared outline + progress | `../courseweave/src/zhs/outline.rs:64-153` (`shared_outline`, `learning_post`; module 11 `learning/videolist`, optional `learning/login/gologin` bridge), `:175-297` (`parse_outline`, `parse_video`, `apply_progress`; `queryStuyInfo`, small-lesson handling) | ✅ live `catalog.units`: `videolist`, `login/gologin` bridge, `queryStuyInfo` merge, and fixture |
| Smart knowledge outline | `../courseweave/src/zhs/features.rs:146-253` (`feature_units`: module 6 `knowledge-study/course-basic`, `resources/list-knowledge-resource`, `resources-lab/get-video-time`) | ❌ live returns `unsupported_operation`; the fixture carries one smart course |
| Study writes | `../courseweave/src/zhs/study.rs:129-222` (`prepare_study`: `learning/queryCourse`, chapter lock, `popupAnswer/loadVideoPointerInfo`, `learning/prelearningNote`), `:224-260` (`save_study`: `saveDatabaseIntervalTimeV2`, obfuscated `sdsew`/`zwsds`/`ewssw`), `:262-320` (`study_post`, `study_limits`); `../courseweave/src/zhs/features.rs:254-340` (`smart_study_write`, `feature_write`) | ✅ live `study.step`: `queryCourse`, chapter gate, time limit, popup pointers, `prelearningNote`, `saveDatabaseIntervalTimeV2` chunks |
| Popups / interaction | `../courseweave/src/zhs/features/popup.rs:3,12` (`shared_pointers`, `shared_popup`) | ✅ question points are answered from the platform's marked option when one exists; unmarked questions and non-question overlays return `needs_human` (no model call, unlike the reference) |
| Papers / exams | `../courseweave/src/zhs/features/assessment.rs:3-232` (`feature_exam_sso`, `feature_papers`, `feature_open`), `:509` (`feature_submitted`); gateways `newexam-api`, `studentexam-api`, `taurusexam-api`, smart `kg-ai-run` task list; modules 3/5/6 | ❌ not ported; `catalog.papers` / `exam.*` return `unsupported_operation` |
| Mastery | `../courseweave/src/zhs/features/mastery.rs:3-188` (`mastery_papers`, `mastery_open`, `mastery_writes`, `mastery_submitted`, `graded_answers`; `queryCourseMapInfo`, `queryMasteryNodeInfo`, `getAllMasteryInfos`) | ❌ not ported |
| Meetings / live classes | `../courseweave/src/zhs/features/activity.rs:3,36` (`meeting_list`, `meeting_status`; `stuonline.zhihuishu.com`, `im.zhihuishu.com`) | ❌ not ported |
| Q&A board | `../courseweave/src/jobs/activity/qa.rs:8-100` (`Page::open`, `request`, `questions`), `:301` (`board`), `:323` (`drafts`); `QA_GATEWAY` = `creditqa-api.zhihuishu.com` | ❌ not ported. The reference drives a real browser to `qah5.zhihuishu.com`; module API v1 has no browser service and the manifest declares `browser: false` — a framework-side decision is needed |
| Answer orchestration | `../courseweave/src/jobs/answer.rs:674,704,756,774` (`get_answers`, `save_answers`, `confirm_submit`, `reconcile`), `../courseweave/src/jobs/learning.rs` (`run`) | ❌ not ported; this is framework job logic rather than a platform wire protocol |
| Presentation tabs/actions | manifest `presentation`, `when` predicates | ✅ in manifest and `platform.describe`; evaluation is the framework's job |

Everything above shares one missing primitive: the signed request wrapper
`../courseweave/src/zhs/features.rs:20-84` (`feature_request`) plus `encode_query`/
`protocol_key`. Porting that first unlocks course list, outline, exams, mastery and study
writes.

## Framework-side notes

1. **Environment:** exactly `CF_MODULE_SETTINGS` (JSON object) is read; `CF_MODULE_ID` is
   ignored; no other environment variable matters. Working directory must be the module
   directory because the manifest entry is relative.
2. **IDs:** framework request ids are echoed exactly (any JSON value is accepted, though
   the spec uses positive integers). Module `cw.*` call ids are monotonic from 1 and are
   never reused.
3. **Time:** fixture `auth.begin`/`auth.poll` call `cw.clock.now`; any harness must answer
   those even in fixture mode. The whole handler is budgeted at 28 s and each host call at
   ≤25 s.
4. **Sessions:** the module stores nothing about cookies. `auth.begin` gets a `ch_…`
   handle and `auth.poll`/`auth.complete` pass it back untouched. Live QR state
   (`qrToken`, expiry) is process memory, so a restart mid-login means `auth.begin`
   again.
5. **Identity:** `account_raw` is `cf-demo-10001` in fixture mode; in live mode it would
   come from the SSO exchange once ported. The framework should hash it as
   `zhs/account/<sha256>`.
6. **Output:** stdout is protocol-only and responses may be out of request order (threads).
   Correlate by `id`. stderr is all human logs. No `cw.log` notifications are emitted.
7. **Shutdown:** stdin EOF ⇒ drain ≤5 s ⇒ flush ⇒ exit 0. SIGTERM/SIGKILL handling stays
   with the framework.
8. **Live blockers:** (a) the module may only depend on `serde`/`serde_json`, so RSA/AES
   signing cannot be implemented without a dependency decision; (b) the Q&A reference
   needs a browser, which the current API exposes only as `permissions.browser: false`
   and no `cw.browser` service; (c) live smart-course data needs the module-6 gateway.
9. **Fixture safety:** fixtures are compiled in; installing/copying just the directory
   (manifest + binary) is enough. `source` defaults to fixture, so an unconfigured install
   never touches the network.

## Tests

`cargo test` runs 21 tests (11 unit + 10 contract), with no network and no account:

- descriptor equals `courseflow.module.json` (`platform`, module id/name/version/api,
  `when` keys, snake_case, no `native`)
- course fixture: 2 courses, shared/smart fields, progress, `outline_available`
- unit fixture: 3 units, 2 chapters, `selectable` false only for the document, note present
- `study.step`: monotonic 60 s steps, completion at duration, zero-duration first-step
  completion, replaying a checkpoint is idempotent, bad checkpoints rejected
- unknown method → `unsupported_operation`; missing session → `invalid_params`
- fixture auth flow: waiting → connected → expired, complete/verify identity
- live unported methods: typed errors, no fetch performed
- live `cw.net.fetch` errors pass through unchanged
- live course POST shape (`fromurl`, Origin, JSON encoding) and parse
- real NDJSON round trip through `rpc::serve` with the actual reader/dispatch (order
  insensitive, notification produces no stdout line)
