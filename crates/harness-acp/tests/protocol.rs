use std::sync::Arc;

use harness_acp::AcpHarnessAdapter;
use kernel::harness::Command;
use kernel::harness::HarnessAdapter;
use kernel::harness::PlanEntry;
use kernel::harness::PlanPriority;
use kernel::harness::SessionEvent;
use kernel::harness::SessionOption;
use kernel::harness::SessionSpec;
use kernel::harness::ToolCallInfo;
use kernel::harness::ToolCallStatus;
use kernel::harness::ToolCallUpdate;
use kernel::harness::TurnAttachment;
use kernel::harness::TurnInput;
use serde::Deserialize;
use serde::Serialize;
use tempfile::tempdir;
use wire::harness::ApprovalDecision;
use wire::harness::HarnessId;
use wire::json::JsonObject;
use wire::session_id::SessionId;
use wire::stream::LlmUsage;
use wire::stream::ThinkingLevel;

#[cfg(test)]
mod protocol_support;
use protocol_support::{SESSION, collect_until_turn_end, launch, launch_in, run, spec};

#[derive(Debug, Deserialize, PartialEq, Serialize)]
struct CommandInput {
    command: String,
}

fn expected_events(input: JsonObject) -> Vec<SessionEvent> {
    vec![
        SessionEvent::AgentMessageChunk {
            text: "hello".to_owned(),
        },
        SessionEvent::AgentThoughtChunk {
            text: "pondering".to_owned(),
        },
        SessionEvent::UserMessageChunk {
            text: "echo".to_owned(),
        },
        SessionEvent::ToolCall(ToolCallInfo {
            tool_call_id: "fake-tool-1".to_owned(),
            name: "run build".to_owned(),
            input,
        }),
        SessionEvent::ToolCallUpdate(ToolCallUpdate {
            tool_call_id: "fake-tool-1".to_owned(),
            status: ToolCallStatus::Completed,
            output: Some("build ok".to_owned()),
        }),
        SessionEvent::Plan(vec![PlanEntry {
            content: "measure the thing".to_owned(),
            priority: PlanPriority::High,
        }]),
        SessionEvent::AvailableCommands(vec![Command {
            name: "review".to_owned(),
            description: "review the change".to_owned(),
        }]),
        SessionEvent::CurrentMode {
            mode: "fast".to_owned(),
        },
        SessionEvent::TurnEnded,
    ]
}

#[test]
fn a_turn_streams_the_mapped_events_in_order() {
    let adapter = AcpHarnessAdapter::with_launch_table(launch(
        &[
            "text:hello",
            "thought:pondering",
            "user:echo",
            "tool:run build",
            "tool-update:fake-tool-1:completed",
            "plan",
            "commands",
            "mode:fast",
        ],
        &[],
    ));
    run(async {
        let cwd = tempdir().expect("cwd");
        let session = adapter
            .new_session(spec(cwd.path()))
            .await
            .expect("new session");
        assert_eq!(session.as_str(), "opencode:ses_fake0123456789abcdefgh");
        let mut events = adapter.events(&session).await.expect("events");
        adapter
            .prompt(&session, TurnInput::Text("do the thing".to_owned()))
            .await
            .expect("prompt");
        let collected = collect_until_turn_end(&mut events)
            .await
            .expect("events before turn end");
        let input: JsonObject = serde_json::from_value(
            serde_json::to_value(CommandInput {
                command: "ls -1".to_owned(),
            })
            .expect("input serializes"),
        )
        .expect("object input");
        assert_eq!(collected, expected_events(input));
    });
}

fn turn_usage_events(mode: &str) -> Result<Vec<SessionEvent>, String> {
    let adapter = AcpHarnessAdapter::with_launch_table(launch(&["text:hello"], &["--usage", mode]));
    run(async {
        let cwd = tempdir().map_err(|error| error.to_string())?;
        let session = adapter
            .new_session(spec(cwd.path()))
            .await
            .map_err(|error| error.to_string())?;
        let mut events = adapter
            .events(&session)
            .await
            .map_err(|error| error.to_string())?;
        adapter
            .prompt(&session, TurnInput::Text("do the thing".to_owned()))
            .await
            .map_err(|error| error.to_string())?;
        collect_until_turn_end(&mut events).await
    })
}

#[test]
fn the_prompt_result_usage_arrives_before_the_turn_end() {
    assert_eq!(
        turn_usage_events("reported").expect("reported usage turn"),
        vec![
            SessionEvent::AgentMessageChunk {
                text: "hello".to_owned(),
            },
            SessionEvent::Usage {
                usage: LlmUsage {
                    prompt_tokens: 23_248,
                    completion_tokens: 4,
                    reasoning_tokens: None,
                    cached_tokens: None,
                    cache_creation_tokens: Some(12_776),
                    cache_read_tokens: Some(10_470),
                },
            },
            SessionEvent::TurnEnded,
        ]
    );
}

#[test]
fn usage_under_the_prompt_result_meta_arrives_before_the_turn_end() {
    assert_eq!(
        turn_usage_events("meta").expect("meta usage turn"),
        vec![
            SessionEvent::AgentMessageChunk {
                text: "hello".to_owned(),
            },
            SessionEvent::Usage {
                usage: LlmUsage {
                    prompt_tokens: 20_185,
                    completion_tokens: 44,
                    reasoning_tokens: Some(43),
                    cached_tokens: None,
                    cache_creation_tokens: Some(0),
                    cache_read_tokens: Some(2_176),
                },
            },
            SessionEvent::TurnEnded,
        ]
    );
}

#[test]
fn a_permission_request_is_answered_through_the_port() {
    let adapter = Arc::new(AcpHarnessAdapter::with_launch_table(launch(
        &["tool:run build", "permission"],
        &[],
    )));
    run(async {
        let cwd = tempdir().expect("cwd");
        let session = adapter
            .new_session(spec(cwd.path()))
            .await
            .expect("new session");
        let mut events = adapter.events(&session).await.expect("events");
        let prompt_task = tokio::spawn({
            let adapter = adapter.clone();
            let session = session.clone();
            async move {
                adapter
                    .prompt(&session, TurnInput::Text("go".to_owned()))
                    .await
            }
        });
        let mut seen = Vec::new();
        let request = loop {
            let event = events.recv().await.expect("event");
            let SessionEvent::Permission(request) = event else {
                seen.push(event);
                continue;
            };
            break request;
        };
        assert_eq!(seen.len(), 1);
        assert!(matches!(
            seen.first(),
            Some(SessionEvent::ToolCall(ToolCallInfo { name, .. })) if name == "run build"
        ));
        assert_eq!(request.name, "run the build");
        assert_eq!(request.tool_call_id.as_deref(), Some("fake-tool-1"));
        assert_eq!(request.reason, None);
        let input: CommandInput =
            serde_json::from_value(serde_json::to_value(&request.input).expect("input serializes"))
                .expect("input decodes");
        assert_eq!(input.command, "cargo build");
        adapter
            .answer_permission(&session, &request.request_id, ApprovalDecision::Allow)
            .await
            .expect("allow");
        prompt_task.await.expect("prompt task").expect("prompt ok");
        let rest = collect_until_turn_end(&mut events)
            .await
            .expect("events before turn end");
        assert_eq!(rest, vec![SessionEvent::TurnEnded]);
        let wrong = adapter
            .answer_permission(&session, "acp-permission-999", ApprovalDecision::Allow)
            .await
            .expect_err("unknown request");
        assert!(wrong.to_string().contains("acp-permission-999"));
    });
}

#[test]
fn a_denied_permission_ends_the_turn_normally() {
    let adapter = Arc::new(AcpHarnessAdapter::with_launch_table(launch(
        &["permission"],
        &[],
    )));
    run(async {
        let cwd = tempdir().expect("cwd");
        let session = adapter
            .new_session(spec(cwd.path()))
            .await
            .expect("new session");
        let mut events = adapter.events(&session).await.expect("events");
        let prompt_task = tokio::spawn({
            let adapter = adapter.clone();
            let session = session.clone();
            async move {
                adapter
                    .prompt(&session, TurnInput::Text("go".to_owned()))
                    .await
            }
        });
        let request = loop {
            let event = events.recv().await.expect("event");
            let SessionEvent::Permission(request) = event else {
                continue;
            };
            break request;
        };
        adapter
            .answer_permission(&session, &request.request_id, ApprovalDecision::Deny)
            .await
            .expect("deny");
        prompt_task.await.expect("prompt task").expect("prompt ok");
        let rest = collect_until_turn_end(&mut events)
            .await
            .expect("events before turn end");
        assert_eq!(rest, vec![SessionEvent::TurnEnded]);
    });
}

#[test]
fn an_unanswerable_decision_is_a_typed_error() {
    let adapter = Arc::new(AcpHarnessAdapter::with_launch_table(launch(
        &["permission"],
        &[],
    )));
    run(async {
        let cwd = tempdir().expect("cwd");
        let session = adapter
            .new_session(spec(cwd.path()))
            .await
            .expect("new session");
        let mut events = adapter.events(&session).await.expect("events");
        let prompt_task = tokio::spawn({
            let adapter = adapter.clone();
            let session = session.clone();
            async move {
                adapter
                    .prompt(&session, TurnInput::Text("go".to_owned()))
                    .await
            }
        });
        let request = loop {
            let event = events.recv().await.expect("event");
            let SessionEvent::Permission(request) = event else {
                continue;
            };
            break request;
        };
        let wrong = adapter
            .answer_permission(
                &session,
                &request.request_id,
                ApprovalDecision::AllowSession,
            )
            .await
            .expect_err("no allow-always option offered");
        assert!(wrong.to_string().contains("AllowSession"));
        adapter.cancel(&session).await.expect("cancel");
        prompt_task.await.expect("prompt task").expect("prompt ok");
    });
}

#[test]
fn cancel_ends_a_hanging_turn() {
    let adapter = Arc::new(AcpHarnessAdapter::with_launch_table(launch(
        &["text:partial"],
        &["--hang-until-cancel"],
    )));
    run(async {
        let cwd = tempdir().expect("cwd");
        let session = adapter
            .new_session(spec(cwd.path()))
            .await
            .expect("new session");
        let mut events = adapter.events(&session).await.expect("events");
        let prompt_task = tokio::spawn({
            let adapter = adapter.clone();
            let session = session.clone();
            async move {
                adapter
                    .prompt(&session, TurnInput::Text("go".to_owned()))
                    .await
            }
        });
        loop {
            let event = events.recv().await.expect("event");
            let SessionEvent::AgentMessageChunk { text } = event else {
                continue;
            };
            if text == "partial" {
                break;
            }
        }
        adapter.cancel(&session).await.expect("cancel");
        prompt_task.await.expect("prompt task").expect("prompt ok");
        let rest = collect_until_turn_end(&mut events)
            .await
            .expect("events before turn end");
        assert_eq!(rest, vec![SessionEvent::TurnEnded]);
    });
}

#[test]
fn resume_discards_the_load_replay_on_a_fresh_agent() {
    run(async {
        let cwd = tempdir().expect("cwd");
        let session = {
            let adapter =
                AcpHarnessAdapter::with_launch_table(launch(&[], &["--on-load", "replay"]));
            adapter
                .new_session(spec(cwd.path()))
                .await
                .expect("new session")
        };
        let adapter = AcpHarnessAdapter::with_launch_table(launch_in(
            &["text:after"],
            &["--on-load", "replay"],
            cwd.path(),
        ));
        adapter.resume(&session).await.expect("resume");
        let mut events = adapter.events(&session).await.expect("events");
        adapter
            .prompt(&session, TurnInput::Text("continue".to_owned()))
            .await
            .expect("prompt");
        let collected = collect_until_turn_end(&mut events)
            .await
            .expect("events before turn end");
        let expected = vec![
            SessionEvent::AgentMessageChunk {
                text: "after".to_owned(),
            },
            SessionEvent::TurnEnded,
        ];
        assert_eq!(collected, expected);
    });
}

#[test]
fn resume_of_an_unknown_session_is_a_typed_protocol_error() {
    run(async {
        let cwd = tempdir().expect("cwd");
        let adapter = AcpHarnessAdapter::with_launch_table(launch_in(
            &[],
            &["--on-load", "reject"],
            cwd.path(),
        ));
        let unknown = SessionId::new(HarnessId::Opencode, "ses_other000000000000000019");
        let error = adapter.resume(&unknown).await.expect_err("load rejected");
        assert!(error.to_string().contains("session/load failed"));
        assert!(error.to_string().contains("session not found"));
    });
}

#[test]
fn a_crashed_agent_ends_the_stream() {
    let adapter = AcpHarnessAdapter::with_launch_table(launch(&[], &["--crash-on", "prompt"]));
    run(async {
        let cwd = tempdir().expect("cwd");
        let session = adapter
            .new_session(spec(cwd.path()))
            .await
            .expect("new session");
        let mut events = adapter.events(&session).await.expect("events");
        let error = adapter
            .prompt(&session, TurnInput::Text("go".to_owned()))
            .await
            .expect_err("agent crashed");
        assert!(error.to_string().contains("session/prompt"));
        let mut drained = Vec::new();
        while let Some(event) = events.recv().await {
            drained.push(event);
        }
        assert!(
            drained
                .iter()
                .any(|event| matches!(event, SessionEvent::Exited { .. })),
            "the exit is reported: {drained:?}"
        );
    });
}

#[test]
fn an_agent_that_dies_at_spawn_is_a_typed_error() {
    let adapter = AcpHarnessAdapter::with_launch_table(launch(&[], &["--crash-on", "new"]));
    run(async {
        let cwd = tempdir().expect("cwd");
        let error = adapter
            .new_session(spec(cwd.path()))
            .await
            .expect_err("agent crashed at session/new");
        assert!(error.to_string().contains("session/new failed"));
    });
}

#[test]
fn set_option_round_trips_model_and_effort() {
    let adapter = AcpHarnessAdapter::with_launch_table(launch(&[], &[]));
    run(async {
        let cwd = tempdir().expect("cwd");
        let session = adapter
            .new_session(spec(cwd.path()))
            .await
            .expect("new session");
        let mut events = adapter.events(&session).await.expect("events");
        adapter
            .set_option(&session, SessionOption::Model("grok-5".to_owned()))
            .await
            .expect("model option");
        adapter
            .set_option(&session, SessionOption::Effort(ThinkingLevel::High))
            .await
            .expect("effort option");
        let mut collected = Vec::new();
        for _ in 0..2 {
            collected.push(events.recv().await.expect("config event"));
        }
        assert_eq!(
            collected,
            vec![
                SessionEvent::AgentMessageChunk {
                    text: "model=grok-5".to_owned(),
                },
                SessionEvent::AgentMessageChunk {
                    text: "thought_level=high".to_owned(),
                },
            ]
        );
    });
}

#[test]
fn a_rejected_config_value_fails_closed() {
    let adapter = AcpHarnessAdapter::with_launch_table(launch(&[], &[]));
    run(async {
        let cwd = tempdir().expect("cwd");
        let session = adapter
            .new_session(spec(cwd.path()))
            .await
            .expect("new session");
        let error = adapter
            .set_option(&session, SessionOption::Model("missing".to_owned()))
            .await
            .expect_err("value not offered");
        let message = error.to_string();
        assert!(message.contains("model"), "{message}");
        assert!(message.contains("missing"), "{message}");
    });
}

#[test]
fn the_spec_model_and_effort_are_applied_at_new_session() {
    let adapter = AcpHarnessAdapter::with_launch_table(launch(&[], &[]));
    run(async {
        let cwd = tempdir().expect("cwd");
        let full = SessionSpec {
            session: SessionId::new(HarnessId::Opencode, SESSION),
            harness: HarnessId::Opencode,
            model: Some("grok-5".to_owned()),
            effort: Some(ThinkingLevel::Low),
            working_dir: Some(cwd.path().to_path_buf()),
        };
        let session = adapter.new_session(full).await.expect("new session");
        let mut events = adapter.events(&session).await.expect("events");
        let mut collected = Vec::new();
        for _ in 0..2 {
            collected.push(events.recv().await.expect("config event"));
        }
        assert_eq!(
            collected,
            vec![
                SessionEvent::AgentMessageChunk {
                    text: "model=grok-5".to_owned(),
                },
                SessionEvent::AgentMessageChunk {
                    text: "thought_level=low".to_owned(),
                },
            ]
        );
    });
}

#[test]
fn port_refusals_are_typed() {
    let adapter = AcpHarnessAdapter::with_launch_table(launch(&[], &[]));
    run(async {
        let cwd = tempdir().expect("cwd");
        let unknown = SessionId::new(HarnessId::Opencode, "ses_absent000000000000000001");
        let error = adapter
            .prompt(&unknown, TurnInput::Text("go".to_owned()))
            .await
            .expect_err("unknown session");
        assert!(matches!(
            error,
            kernel::harness::HarnessAdapterError::UnknownSession(_)
        ));
        let error = adapter.events(&unknown).await.expect_err("unknown session");
        assert!(matches!(
            error,
            kernel::harness::HarnessAdapterError::UnknownSession(_)
        ));
        let session = adapter
            .new_session(spec(cwd.path()))
            .await
            .expect("new session");
        let first = adapter.events(&session).await.expect("events");
        let error = adapter.events(&session).await.expect_err("stream taken");
        assert!(matches!(
            error,
            kernel::harness::HarnessAdapterError::EventStreamTaken(_)
        ));
        drop(first);
        let mismatched = SessionSpec {
            session: SessionId::new(
                HarnessId::KimiCode,
                "session_00000000-0000-4000-8000-000000000000",
            ),
            harness: HarnessId::Opencode,
            model: None,
            effort: None,
            working_dir: Some(cwd.path().to_path_buf()),
        };
        let error = adapter
            .new_session(mismatched)
            .await
            .expect_err("harness mismatch");
        assert!(matches!(
            error,
            kernel::harness::HarnessAdapterError::InvalidSession(_)
        ));
    });
}

#[test]
fn raw_array_tool_input_has_no_event_form() {
    let adapter = AcpHarnessAdapter::with_launch_table(launch(&["tool-raw-array"], &[]));
    run(async {
        let cwd = tempdir().expect("cwd");
        let session = adapter
            .new_session(spec(cwd.path()))
            .await
            .expect("new session");
        let mut events = adapter.events(&session).await.expect("events");
        adapter
            .prompt(&session, TurnInput::Text("go".to_owned()))
            .await
            .expect("prompt");
        let collected = collect_until_turn_end(&mut events)
            .await
            .expect("events before turn end");
        assert_eq!(collected, vec![SessionEvent::TurnEnded]);
    });
}

#[test]
fn resume_refuses_an_active_session() {
    run(async {
        let cwd = tempdir().expect("cwd");
        let adapter = AcpHarnessAdapter::with_launch_table(launch(&[], &["--on-load", "replay"]));
        let session = adapter
            .new_session(spec(cwd.path()))
            .await
            .expect("new session");
        let error = adapter.resume(&session).await.expect_err("still active");
        assert!(error.to_string().contains("already active"));
    });
}

#[test]
fn attachments_reach_the_agent_as_image_and_link_blocks() {
    run(async {
        let cwd = tempdir().expect("cwd");
        let image = cwd.path().join("shot.png");
        std::fs::write(&image, b"png").expect("image");
        let notes = cwd.path().join("notes.md");
        std::fs::write(&notes, b"# notes").expect("notes");
        let inline = format!("image:image/png:4:file://{}", image.display());
        let linked = format!("link:shot.png:image/png:file://{}", image.display());
        for (extra, image_block) in [
            (["--prompt", "echo-images"], inline),
            (["--prompt", "echo"], linked),
        ] {
            let adapter = AcpHarnessAdapter::with_launch_table(launch(&[], &extra));
            let session = adapter
                .new_session(spec(cwd.path()))
                .await
                .expect("new session");
            let mut events = adapter.events(&session).await.expect("events");
            let attachments = vec![
                TurnAttachment {
                    path: image.clone(),
                    media_type: "image/png".to_owned(),
                    name: "shot.png".to_owned(),
                },
                TurnAttachment {
                    path: notes.clone(),
                    media_type: "text/markdown".to_owned(),
                    name: "notes.md".to_owned(),
                },
            ];
            adapter
                .prompt(
                    &session,
                    TurnInput::Attached {
                        text: "look".to_owned(),
                        attachments,
                    },
                )
                .await
                .expect("prompt");
            let collected = collect_until_turn_end(&mut events)
                .await
                .expect("events before turn end");
            let expected: Vec<SessionEvent> = [
                "text:look".to_owned(),
                image_block,
                format!("link:notes.md:text/markdown:file://{}", notes.display()),
            ]
            .into_iter()
            .map(|text| SessionEvent::AgentMessageChunk { text })
            .chain([SessionEvent::TurnEnded])
            .collect();
            assert_eq!(collected, expected);
        }
    });
}
