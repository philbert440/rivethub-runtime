use std::path::Path;
use std::path::PathBuf;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use libtest_mimic::Arguments;
use libtest_mimic::Completion;
use libtest_mimic::Failed;
use libtest_mimic::Trial;
use libtest_mimic::run;
use serde_json::json;
use sha2::Digest as _;
use sha2::Sha256;

use testkit::Frame;
use testkit::Handshake;
use testkit::HookFile;
use testkit::HookOutcome;
use testkit::HookPost;
use testkit::Json;
use testkit::Mismatch;
use testkit::PgOutcome;
use testkit::PgStep;
use testkit::ProviderTurn;
use testkit::Response;
use testkit::Thrown;
use testkit::WsCall;
use testkit::WsReply;
use testkit::ws::FrameRecord;

const RECIPE_SHA256: &str = "fe4ea99d152e21bd03361ecfb7f49c081a2f125d7742919376e9988e3eea9a86";
const DEVIATION_CASE: &str = "deviation-server-header";
const DEVIATION_REASON: &str =
    "deliberate deviation: the recorded X-Recorded-Only header is not reproduced";

fn fixture(name: &str) -> PathBuf {
    testkit::golden_path(env!("CARGO_MANIFEST_DIR"), "tests/fixtures").join(name)
}

fn masks() -> testkit::Masks {
    testkit::Masks {
        port: "5174".to_string(),
        hostname: Some("gold-host".to_string()),
        tmp: Some("/tmp/golden".to_string()),
    }
}

fn must_load<T>(loaded: Result<T, testkit::Error>, what: &str) -> Result<T, Failed> {
    loaded.map_err(|error| Failed::from(format!("{what}: {error}")))
}

fn must_find<'a, T>(found: Option<&'a T>, what: &str) -> Result<&'a T, Failed> {
    found.ok_or_else(|| Failed::from(format!("{what}: not found in the fixture")))
}

fn must_pass(outcome: Result<(), Mismatch>) -> Result<(), Failed> {
    match outcome {
        Ok(()) => Ok(()),
        Err(mismatch) => Err(Failed::from(format!(
            "{}: {}",
            mismatch.id, mismatch.detail
        ))),
    }
}

fn must_fail(outcome: Result<(), Mismatch>, why: &str) -> Result<Mismatch, Failed> {
    match outcome {
        Ok(()) => Err(Failed::from(format!(
            "{why}: expected a mismatch, got a match"
        ))),
        Err(mismatch) => Ok(mismatch),
    }
}

fn assert_detail(mismatch: &Mismatch, needle: &str, what: &str) -> Result<(), Failed> {
    if mismatch.detail.contains(needle) {
        Ok(())
    } else {
        Err(Failed::from(format!(
            "{what}: expected {needle:?} in the detail, got: {}",
            mismatch.detail
        )))
    }
}

fn load_failed(trial: String, error: testkit::Error) -> Trial {
    Trial::test(trial, move || Err(Failed::from(error.to_string())))
}

fn http_healthz() -> Response {
    Response {
        status: 200,
        headers: vec![
            ("Content-Type".to_string(), "application/json".to_string()),
            (
                "Date".to_string(),
                "Mon, 12 Oct 2026 00:00:00 GMT".to_string(),
            ),
        ],
        body: br#"{"ok":true,"sessions":0,"name":"gold-host"}"#.to_vec(),
    }
}

#[expect(
    clippy::expect_used,
    reason = "the png body is a constant in this test; a decode failure must fail the trial, not compare an empty body"
)]
fn http_png() -> Response {
    let body = BASE64
        .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==")
        .expect("the test png constant decodes");
    Response {
        status: 200,
        headers: vec![("Content-Type".to_string(), "image/png".to_string())],
        body,
    }
}

fn http_handler(request: &testkit::Request) -> Response {
    if request.target == "/healthz" {
        similar_asserts::assert_eq!(request.method, "GET");
        http_healthz()
    } else {
        similar_asserts::assert_eq!(request.target, "/pixel.png");
        http_png()
    }
}

fn add_http(trials: &mut Vec<Trial>, path: &Path) {
    match testkit::http::cases(path) {
        Ok(cases) => {
            for case in cases {
                let trial = format!("http {}", case.id);
                let masks = masks();
                let deviation = case.id.ends_with(DEVIATION_CASE);
                trials.push(Trial::ignorable_test(trial, move || {
                    if deviation {
                        return Ok(Completion::ignored_with(DEVIATION_REASON));
                    }
                    must_pass(testkit::http::replay(&case, &masks, http_handler))?;
                    Ok(Completion::Completed)
                }));
            }
        }
        Err(error) => trials.push(load_failed(format!("http load {}", path.display()), error)),
    }
}

fn build_recipe_body(recipe: &str) -> Option<Vec<u8>> {
    let (count_text, _) = recipe.split_once(" x ")?;
    let count = count_text.parse::<usize>().ok()?;
    let mut parts = Vec::with_capacity(count);
    for i in 0..count {
        let activity = if i % 2 == 1 { "thinking" } else { "idle" };
        let index = i64::try_from(i).ok()?;
        parts.push(format!(
            "{{\"v\":1,\"session\":\"golden-batch\",\"type\":\"activity\",\"activity\":\"{activity}\",\"ts\":{}}}",
            1_760_000_100_000_i64 + index
        ));
    }
    Some(format!("[{}]", parts.join(",")).into_bytes())
}

fn add_recipe(trials: &mut Vec<Trial>, path: &Path) {
    match testkit::http::cases(path) {
        Ok(cases) => {
            for case in cases {
                let trial = format!("http recipe {}", case.id);
                let masks = masks();
                trials.push(Trial::test(trial, move || {
                    must_pass(testkit::http::replay(&case, &masks, |request| {
                        let Some(recipe) = request.recipe.as_deref() else {
                            return Response {
                                status: 500,
                                headers: Vec::new(),
                                body: Vec::new(),
                            };
                        };
                        similar_asserts::assert_eq!(request.body, Vec::<u8>::new());
                        let Some(body) = build_recipe_body(recipe) else {
                            return Response {
                                status: 500,
                                headers: Vec::new(),
                                body: Vec::new(),
                            };
                        };
                        let digest = Sha256::digest(&body);
                        similar_asserts::assert_eq!(format!("{digest:x}"), RECIPE_SHA256);
                        Response {
                            status: 400,
                            headers: vec![(
                                "Content-Type".to_string(),
                                "application/json".to_string(),
                            )],
                            body: br#"{"error":"expected an array of 1-100 events"}"#.to_vec(),
                        }
                    }))
                }));
            }
        }
        Err(error) => trials.push(load_failed(
            format!("recipe load {}", path.display()),
            error,
        )),
    }
}

fn recorded_frames(frames: &[FrameRecord]) -> Vec<Frame> {
    frames
        .iter()
        .filter_map(|recorded| match recorded {
            FrameRecord::Text { text } => Some(Frame::Text(text.clone())),
            FrameRecord::Binary {
                text,
                base64,
                replay,
            } => {
                let data = if let Some(text) = text {
                    Some(text.as_bytes().to_vec())
                } else {
                    base64
                        .as_deref()
                        .and_then(|encoded| BASE64.decode(encoded).ok())
                };
                data.map(|data| Frame::Binary {
                    data,
                    replay: *replay,
                })
            }
            FrameRecord::Close { code, reason } => Some(Frame::Close {
                code: *code,
                reason: reason.clone(),
            }),
            FrameRecord::TcpClosed => Some(Frame::TcpClosed),
        })
        .collect()
}

fn echo_harness(step: &testkit::ws::StepRecord) -> impl FnMut(WsCall) -> WsReply + '_ {
    move |call| match call {
        WsCall::Upgrade(_) => {
            let Some(connect) = &step.connect else {
                return WsReply::Done;
            };
            if connect.handshake.no_response {
                return WsReply::Handshake(Handshake {
                    status: None,
                    headers: Vec::new(),
                    body: Vec::new(),
                });
            }
            WsReply::Handshake(Handshake {
                status: Some(testkit::http::status_of(&connect.handshake.status_line)),
                headers: connect.handshake.headers.clone(),
                body: connect
                    .handshake
                    .body
                    .clone()
                    .unwrap_or_default()
                    .into_bytes(),
            })
        }
        WsCall::Http(_) => {
            let Some(http) = &step.http else {
                return WsReply::Done;
            };
            let body = if let Some(text) = &http.response.body {
                text.as_bytes().to_vec()
            } else if let Some(encoded) = &http.response.body_base64 {
                BASE64.decode(encoded).unwrap_or_default()
            } else {
                Vec::new()
            };
            WsReply::Http(Response {
                status: testkit::http::status_of(&http.response.status_line),
                headers: http.response.headers.clone(),
                body,
            })
        }
        WsCall::Take { client } => WsReply::Frames(
            step.frames
                .get(&client)
                .map_or_else(Vec::new, |frames| recorded_frames(frames)),
        ),
        WsCall::Sent(_) | WsCall::ClientClose { .. } => WsReply::Done,
    }
}

fn add_ws(trials: &mut Vec<Trial>, path: &Path) {
    match testkit::ws::steps(path) {
        Ok(steps) => {
            for step in steps {
                let trial = format!("ws {}", step.id);
                let masks = masks();
                trials.push(Trial::test(trial, move || {
                    let mut harness = echo_harness(&step.record);
                    must_pass(testkit::ws::replay(&step, &masks, &mut harness))
                }));
            }
        }
        Err(error) => trials.push(load_failed(format!("ws load {}", path.display()), error)),
    }
}

fn hook_outcome(session_key: &str, log_session: &str) -> HookOutcome {
    HookOutcome {
        exit: 0,
        stdout: String::new(),
        stderr: String::new(),
        posts: vec![HookPost {
            url: "/api/capture".to_string(),
            content_type: "application/json".to_string(),
            body: format!(
                "{{\"session_key\":\"{session_key}\",\"settings\":{{\"last_event\":\"UserPromptSubmit\",\"last_ingest_at\":\"2026-11-01T09:00:00.000Z\"}},\"messages\":[{{\"event_id\":\"{session_key}:u1\",\"metadata\":{{\"ingest_key\":\"1799999999999-abcdef123\"}}}}]}}"
            ),
        }],
        files: vec![HookFile {
            path: ".rivetos/capture.log".to_string(),
            mode: "644".to_string(),
            content: format!("{log_session} UserPromptSubmit 2026-11-01T09:00:00.123Z +1 msg\n"),
        }],
    }
}

fn add_capture(trials: &mut Vec<Trial>, path: &Path) {
    match testkit::capture::entries(path) {
        Ok(entries) => {
            for entry in entries {
                let trial = format!("capture hook/{}", entry.index);
                trials.push(Trial::test(trial, move || {
                    let outcome = testkit::capture::replay(&entry, |_event, _stdin| HookOutcome {
                        exit: entry.exit,
                        stdout: entry.stdout.clone(),
                        stderr: entry.stderr.clone(),
                        posts: entry
                            .posts
                            .iter()
                            .map(|post| HookPost {
                                url: post.url.clone(),
                                content_type: post.content_type.clone(),
                                body: post.body.clone(),
                            })
                            .collect(),
                        files: entry
                            .files
                            .iter()
                            .map(|file| HookFile {
                                path: file.path.clone(),
                                mode: file.mode.clone(),
                                content: file.content.clone(),
                            })
                            .collect(),
                    });
                    must_pass(outcome)
                }));
            }
        }
        Err(error) => trials.push(load_failed(
            format!("capture load {}", path.display()),
            error,
        )),
    }
}

fn pg_outcome(_call: &PgStep) -> PgOutcome {
    let conversation = "99999999-8888-4333-8444-555555555555";
    let message = "77777777-6666-4333-8444-555555555555";
    PgOutcome {
        result: Some(json!({
            "sessionKey": "claude-code:33333333-3333-3333-3333-333333333333",
            "conversationId": conversation,
            "created": true
        })),
        error: None,
        rows: Some(json!({
            "ros_conversations": [
                { "id": conversation, "session_key": "claude-code:33333333-3333-3333-3333-333333333333", "created_at": "2026-10-10 12:00:00+00" }
            ],
            "ros_messages": [
                { "id": message, "conversation_id": conversation, "created_at": "2026-10-12T10:15:30.123456+00:00" }
            ]
        })),
    }
}

fn add_capture_pg(trials: &mut Vec<Trial>, path: &Path) {
    match testkit::capture_pg::scenarios(path) {
        Ok(list) => {
            for scenario in list {
                let trial = format!("pg {}", scenario.name);
                trials.push(Trial::test(trial, move || {
                    must_pass(testkit::capture_pg::replay_scenario(
                        &scenario, None, pg_outcome,
                    ))
                }));
            }
        }
        Err(error) => trials.push(load_failed(format!("pg load {}", path.display()), error)),
    }
}

fn add_provider(trials: &mut Vec<Trial>) {
    let path = fixture("provider/A-text");
    match testkit::provider::load_scenario(&path) {
        Ok(scenario) => {
            let trial = format!("provider {}", scenario.name);
            trials.push(Trial::test(trial, move || {
                must_pass(testkit::provider::replay(&scenario, |canned| {
                    let status = canned.first().map(|c| c.status);
                    similar_asserts::assert_eq!(status, Some(200));
                    ProviderTurn {
                        requests: vec![testkit::CapturedRequest {
                            method: "POST".to_string(),
                            target: "/v1/chat/completions".to_string(),
                            headers: Vec::new(),
                            body: r#"{"stream":true,"messages":[{"content":"Say hi.","role":"user"}],"model":"test-model"}"#
                                .to_string(),
                        }],
                        chunks: vec![
                            json!({"text": "Hi", "type": "text"}),
                            json!({"type": "done"}),
                        ],
                    }
                }))
            }));
        }
        Err(error) => trials.push(load_failed(
            format!("provider load {}", path.display()),
            error,
        )),
    }
}

fn classify(function: &str, input: &Json) -> Result<Json, Thrown> {
    match function {
        "shared_dir" => {
            let dir_env = input
                .get("env")
                .and_then(|env| env.get("RIVETOS_SHARED_DIR"))
                .and_then(Json::as_str)
                .unwrap_or("");
            let trimmed = dir_env.trim().trim_end_matches('\n');
            Ok(Json::String(if trimmed.is_empty() {
                "/rivet-shared".to_string()
            } else {
                trimmed.to_string()
            }))
        }
        "resolve_env_vars" => Err(Thrown("rejected".to_string())),
        "install_root" => Ok(json!({"$t": "undefined"})),
        _ => Err(Thrown("unknown function".to_string())),
    }
}

fn add_cases(trials: &mut Vec<Trial>, path: &Path) {
    match testkit::cases::cases(path) {
        Ok(list) => {
            for case in list {
                let trial = format!("case {}:{}", case.function, case.line);
                trials.push(Trial::test(trial, move || {
                    must_pass(testkit::cases::replay(&case, classify))
                }));
            }
        }
        Err(error) => trials.push(load_failed(format!("cases load {}", path.display()), error)),
    }
}

fn drift_http_status_and_headers(trials: &mut Vec<Trial>) {
    trials.push(Trial::test("drift http reports mismatching exchanges", {
        let path = fixture("http.json");
        move || {
            let cases = must_load(testkit::http::cases(&path), "fixture loads")?;
            let healthz = must_find(
                cases.iter().find(|case| case.id == "main/healthz-get"),
                "healthz",
            )?
            .clone();
            let mut wrong_body = http_healthz();
            wrong_body.body = br#"{"ok":true,"sessions":1,"name":"gold-host"}"#.to_vec();
            let outcome = testkit::http::replay(&healthz, &masks(), |_| wrong_body.clone());
            let mismatch = must_fail(outcome, "body drift")?;
            assert_detail(&mismatch, "body mismatch", "body drift")?;

            let mut missing_header = http_healthz();
            missing_header.headers.clear();
            let outcome = testkit::http::replay(&healthz, &masks(), |_| missing_header.clone());
            let mismatch = must_fail(outcome, "missing header")?;
            assert_detail(&mismatch, "Content-Type", "missing header")?;

            let wrong_status = Response {
                status: 404,
                headers: Vec::new(),
                body: Vec::new(),
            };
            let outcome = testkit::http::replay(&healthz, &masks(), |_| wrong_status.clone());
            let mismatch = must_fail(outcome, "status drift")?;
            assert_detail(&mismatch, "status 404", "status drift")?;
            Ok(())
        }
    }));
    trials.push(Trial::test(
        "drift http fails on an unparseable request head",
        || {
            let case = testkit::http::HttpCase {
                id: "main/malformed-request-line".to_string(),
                request: testkit::http::RequestRecord {
                    head: "GARBAGE\r\n\r\n".to_string(),
                    ..testkit::http::RequestRecord::default()
                },
                response: testkit::http::ResponseRecord {
                    status_line: "HTTP/1.1 400 Bad Request".to_string(),
                    ..testkit::http::ResponseRecord::default()
                },
            };
            let outcome = testkit::http::replay(&case, &masks(), |_| http_healthz());
            let mismatch = must_fail(outcome, "unreadable request")?;
            assert_detail(
                &mismatch,
                "recording request unreadable",
                "unreadable request",
            )
        },
    ));
}

fn ws_upgrade_reply(accept: &str, close_only: bool) -> WsReply {
    let headers = if close_only {
        vec![("Connection".to_string(), "close".to_string())]
    } else {
        vec![
            ("Upgrade".to_string(), "websocket".to_string()),
            ("Connection".to_string(), "Upgrade".to_string()),
            ("Sec-WebSocket-Accept".to_string(), accept.to_string()),
        ]
    };
    WsReply::Handshake(Handshake {
        status: Some(if close_only { 403 } else { 101 }),
        headers,
        body: Vec::new(),
    })
}

fn ws_drift_harness(takes: &mut usize) -> impl FnMut(WsCall) -> WsReply + '_ {
    move |call| match call {
        WsCall::Upgrade(connect) => {
            let denied = connect
                .request
                .headers
                .iter()
                .any(|(name, value)| name == "Host" && value == "evil.example");
            if denied {
                ws_upgrade_reply("unused", true)
            } else {
                ws_upgrade_reply("wrong-accept", false)
            }
        }
        WsCall::Http(_) => WsReply::Http(Response {
            status: 200,
            headers: Vec::new(),
            body: br#"{"ok":false}"#.to_vec(),
        }),
        WsCall::Take { .. } => {
            *takes += 1;
            if *takes == 1 {
                WsReply::Frames(vec![Frame::Text(
                    "{\"type\":\"snapshot\",\"v\":1}".to_string(),
                )])
            } else {
                WsReply::Frames(Vec::new())
            }
        }
        WsCall::Sent(_) | WsCall::ClientClose { .. } => WsReply::Done,
    }
}

fn add_ws_drift(trials: &mut Vec<Trial>) {
    trials.push(Trial::test(
        "drift ws reports wrong handshakes and frames",
        {
            let path = fixture("ws.json");
            move || {
                let steps = must_load(testkit::ws::steps(&path), "fixture loads")?;
                let mut takes = 0;
                let mut harness = ws_drift_harness(&mut takes);
                for step in &steps {
                    let outcome = testkit::ws::replay(step, &masks(), &mut harness);
                    let mismatch = must_fail(outcome, "ws drift")?;
                    let expected = match step.id.as_str() {
                        "main/post-event" => "body mismatch",
                        "main/upgrade-denied" => "Content-Length",
                        "main/connect-a" | "main/term-echo" => "Sec-WebSocket-Accept",
                        other => return Err(Failed::from(format!("unexpected step {other}"))),
                    };
                    assert_detail(&mismatch, expected, &step.id)?;
                }
                Ok(())
            }
        },
    ));
    trials.push(Trial::test("drift ws reports missing frames", {
        let path = fixture("ws.json");
        move || {
            let steps = must_load(testkit::ws::steps(&path), "fixture loads")?;
            let step = must_find(
                steps.iter().find(|step| step.id == "main/connect-a"),
                "connect-a",
            )?;
            let mut harness = move |call| match call {
                WsCall::Upgrade(_) => ws_upgrade_reply("s3pPLMBiTxaQ9kYGzzhZRbK+xO=", false),
                WsCall::Take { .. } => WsReply::Frames(Vec::new()),
                WsCall::Http(_) | WsCall::Sent(_) | WsCall::ClientClose { .. } => WsReply::Done,
            };
            let outcome = testkit::ws::replay(step, &masks(), &mut harness);
            let mismatch = must_fail(outcome, "missing frames")?;
            assert_detail(&mismatch, "0 frame(s), recorded 1", "missing frames")
        }
    }));
}

fn add_capture_drift(trials: &mut Vec<Trial>) {
    trials.push(Trial::test("drift capture fails on wrong batches", {
        let path = fixture("hook.json");
        move || {
            let entries = must_load(testkit::capture::entries(&path), "fixture loads")?;
            let entry = must_find(entries.first(), "hook entry")?.clone();
            let outcome = testkit::capture::replay(&entry, |_event, _stdin| {
                hook_outcome(
                    "claude-code:44444444-4444-4444-4444-444444444444",
                    "claude-code:33333333-3333-3333-3333-333333333333",
                )
            });
            let mismatch = must_fail(outcome, "wrong batch body")?;
            assert_detail(&mismatch, "batch body mismatch", "wrong batch body")?;

            let outcome = testkit::capture::replay(&entry, |_event, _stdin| {
                hook_outcome(
                    "claude-code:33333333-3333-3333-3333-333333333333",
                    "claude-code:44444444-4444-4444-4444-444444444444",
                )
            });
            let mismatch = must_fail(outcome, "wrong file content")?;
            assert_detail(&mismatch, "content mismatch", "wrong file content")
        }
    }));
}

fn add_pg_drift(trials: &mut Vec<Trial>) {
    trials.push(Trial::test("drift pg fails when rows differ", {
        let path = fixture("pg.json");
        move || {
            let scenarios = must_load(testkit::capture_pg::scenarios(&path), "fixture loads")?;
            let scenario = must_find(scenarios.first(), "scenario")?.clone();
            let outcome = testkit::capture_pg::replay_scenario(&scenario, None, |call| {
                let mut actual = pg_outcome(call);
                if let Some(messages) = actual
                    .rows
                    .as_mut()
                    .and_then(|rows| rows.get_mut("ros_messages"))
                    .and_then(Json::as_array_mut)
                {
                    messages.push(json!({
                        "id": "11111111-2222-4333-8444-555555555555",
                        "conversation_id": "99999999-8888-4333-8444-555555555555",
                        "created_at": "2026-10-12T10:15:31.654321+00:00"
                    }));
                }
                actual
            });
            let mismatch = must_fail(outcome, "extra row")?;
            assert_detail(&mismatch, "rows mismatch", "extra row")
        }
    }));
}

fn add_provider_drift(trials: &mut Vec<Trial>) {
    trials.push(Trial::test("drift provider fails on drifted bodies", {
        let path = fixture("provider/A-text");
        move || {
            let scenario = must_load(testkit::provider::load_scenario(&path), "scenario loads")?;
            let outcome = testkit::provider::replay(&scenario, |_canned| ProviderTurn {
                requests: vec![testkit::CapturedRequest {
                    method: "POST".to_string(),
                    target: "/v1/chat/completions".to_string(),
                    headers: Vec::new(),
                    body: r#"{"model":"other-model","messages":[{"role":"user","content":"Say hi."}],"stream":true}"#
                        .to_string(),
                }],
                chunks: vec![json!({"type": "text", "text": "Hi"})],
            });
            let mismatch = must_fail(outcome, "drifted body")?;
            assert_detail(&mismatch, "request body mismatch", "drifted body")?;
            assert_detail(&mismatch, "other-model", "drifted body")?;

            let outcome = testkit::provider::replay(&scenario, |_canned| ProviderTurn {
                requests: vec![testkit::CapturedRequest {
                    method: "POST".to_string(),
                    target: "/v1/chat/completions".to_string(),
                    headers: Vec::new(),
                    body: r#"{"stream":true,"messages":[{"content":"Say hi.","role":"user"}],"model":"test-model"}"#
                        .to_string(),
                }],
                chunks: vec![json!({"type": "text", "text": "Hi"})],
            });
            let mismatch = must_fail(outcome, "chunk count drift")?;
            assert_detail(&mismatch, "1 chunk(s), recorded 2", "chunk count drift")
        }
    }));
}

fn add_provider_shared(trials: &mut Vec<Trial>) {
    trials.push(Trial::test("drift provider fails on drifted auth header", {
        let path = fixture("provider/A-text");
        move || {
            let mut scenario =
                must_load(testkit::provider::load_scenario(&path), "scenario loads")?;
            let Some(recorded_request) = scenario.requests.first_mut() else {
                return Err(Failed::from("the synthetic request: not found in the fixture"));
            };
            recorded_request.headers = vec![(
                "authorization".to_string(),
                "Bearer <redacted>".to_string(),
            )];
            let outcome = testkit::provider::replay(&scenario, |_canned| ProviderTurn {
                requests: vec![testkit::CapturedRequest {
                    method: "POST".to_string(),
                    target: "/v1/chat/completions".to_string(),
                    headers: vec![(
                        "Authorization".to_string(),
                        "Bearer something-else".to_string(),
                    )],
                    body: r#"{"stream":true,"messages":[{"content":"Say hi.","role":"user"}],"model":"test-model"}"#
                        .to_string(),
                }],
                chunks: Vec::new(),
            });
            let mismatch = must_fail(outcome, "drifted auth")?;
            assert_detail(&mismatch, "authorization", "drifted auth")
        }
    }));
    trials.push(Trial::test("provider strip_b8_deltas removes exactly the B8 fields", {
        move || {
            let stripped = testkit::provider::strip_b8_deltas(
                r#"{"model":"m","max_tokens":4096,"temperature":0.7,"top_p":0.95,"messages":[],"tool_choice":"auto","stream":true}"#,
            )
            .map_err(|error| Failed::from(error.to_string()))?;
            let parsed: Json = serde_json::from_str(&stripped)
                .map_err(|error| Failed::from(error.to_string()))?;
            if testkit::json_eq(
                &parsed,
                &json!({"model":"m","messages":[],"stream":true}),
            ) {
                Ok(())
            } else {
                Err(Failed::from(format!(
                    "the strip left the wrong body: {parsed}"
                )))
            }
        }
    }));
}

fn add_cases_drift(trials: &mut Vec<Trial>) {
    trials.push(Trial::test("drift cases report failing classifiers", {
        let path = fixture("cases.jsonl");
        move || {
            let cases = must_load(testkit::cases::cases(&path), "fixture loads")?;
            let case = must_find(
                cases.iter().find(|case| case.function == "install_root"),
                "install_root",
            )?;
            let outcome = testkit::cases::replay(case, |_function, _input| {
                Ok(Json::String("/opt/rivethub".to_string()))
            });
            let mismatch = must_fail(outcome, "wrong classifier")?;
            if !mismatch.id.contains("install_root") {
                return Err(Failed::from(format!(
                    "wrong classifier: expected install_root in the id, got: {}",
                    mismatch.id
                )));
            }
            assert_detail(&mismatch, "expected", "wrong classifier")
        }
    }));
}

fn main() {
    let args = Arguments::from_args();
    let mut trials = Vec::new();
    add_http(&mut trials, &fixture("http.json"));
    add_recipe(&mut trials, &fixture("recipe.json"));
    add_ws(&mut trials, &fixture("ws.json"));
    add_capture(&mut trials, &fixture("hook.json"));
    add_capture_pg(&mut trials, &fixture("pg.json"));
    add_provider(&mut trials);
    add_cases(&mut trials, &fixture("cases.jsonl"));
    drift_http_status_and_headers(&mut trials);
    add_ws_drift(&mut trials);
    add_capture_drift(&mut trials);
    add_pg_drift(&mut trials);
    add_provider_drift(&mut trials);
    add_provider_shared(&mut trials);
    add_cases_drift(&mut trials);
    run(&args, trials).exit();
}
