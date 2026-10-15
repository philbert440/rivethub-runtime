use super::{
    Failed, Frame, Handshake, Json, ProviderTurn, Response, Trial, WsCall, WsReply, assert_detail,
    fixture, hook_entries, hook_outcome, http_healthz, json, masks, must_fail, must_find,
    must_load, pg_outcome,
};

pub(super) fn drift_http_status_and_headers(trials: &mut Vec<Trial>) {
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
            headers: vec![("Content-Type".to_owned(), "application/json".to_owned())],
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

pub(super) fn add_ws_drift(trials: &mut Vec<Trial>) {
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

pub(super) fn add_capture_drift(trials: &mut Vec<Trial>) {
    trials.push(Trial::test("drift capture fails on wrong batches", {
        let path = fixture("hook.json");
        move || {
            let entries = hook_entries(&path)?;
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

pub(super) fn add_pg_drift(trials: &mut Vec<Trial>) {
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

pub(super) fn add_provider_drift(trials: &mut Vec<Trial>) {
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

pub(super) fn add_provider_shared(trials: &mut Vec<Trial>) {
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

pub(super) fn add_cases_drift(trials: &mut Vec<Trial>) {
    trials.push(Trial::test(
        "case comparison reports changed output",
        || {
            let case = testkit::cases::DiffCase {
                line: 1,
                function: "boundary".to_owned(),
                input: Json::Null,
                expected: testkit::cases::Expected::Out(Json::Bool(true)),
            };
            let mismatch = must_fail(
                testkit::cases::replay(&case, |_, _| Ok(Json::Bool(false))),
                "changed output",
            )?;
            assert_detail(&mismatch, "expected true", "changed output")
        },
    ));
}
