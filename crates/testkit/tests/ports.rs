use std::collections::BTreeMap;
use std::str::FromStr;
use std::sync::Arc;

use chrono::DateTime;
use chrono::Utc;
use futures::executor::block_on;
use kernel::ToolContext;
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
use kernel::harness::TurnInput;
use kernel::memory::BrowseFilter;
use kernel::memory::MemoryStore;
use kernel::memory::SearchQuery;
use kernel::memory::TagAction;
use kernel::model::ModelChunk;
use kernel::model::ModelProvider;
use kernel::model::ModelRequest;
use kernel::queue::Job;
use kernel::queue::JobKeyMode;
use kernel::queue::JobName;
use kernel::queue::JobQueue;
use kernel::queue::WorkerRole;
use kernel::terminal::TermSize;
use kernel::terminal::TerminalMux;
use kernel::tool::Tool;
use kernel::transcript::TranscriptReader;
use kernel::transcript::Turn;
use testkit::FakeHarness;
use testkit::FakeJobQueue;
use testkit::FakeMemoryStore;
use testkit::FakeMux;
use testkit::FakeProvider;
use testkit::FakeReader;
use testkit::FakeTool;
use testkit::Script;
use wire::harness::ApprovalDecision as Decision;
use wire::harness::HarnessId;
use wire::memory::MemoryEntry;
use wire::message::Role;
use wire::session_id::SessionId;

fn memory_entry(session: &str, content: &str, at: DateTime<Utc>) -> MemoryEntry {
    MemoryEntry {
        id: Some(format!("m-{}", content.len())),
        session_id: session.to_string(),
        agent: "agent-a".to_string(),
        channel: "test".to_string(),
        role: Role::User,
        content: content.to_string(),
        tool_name: None,
        tool_args: None,
        tool_result: None,
        metadata: None,
        created_at: Some(at),
    }
}

#[test]
fn memory_store_roundtrip() {
    block_on(async {
        let store: Arc<dyn MemoryStore> = Arc::new(FakeMemoryStore::default());
        let now = Utc::now();
        let inserted = store
            .append(vec![memory_entry("claude-code:abc", "hello world", now)])
            .await
            .unwrap();
        assert_eq!(inserted, 1);
        let hits = store
            .search(SearchQuery {
                text: "hello".to_string(),
                agent: None,
                limit: 10,
            })
            .await
            .unwrap();
        assert_eq!(hits.len(), 1);
        let stats = store.stats().await.unwrap();
        assert_eq!(stats.messages, 1);
        let browsed = store
            .browse(BrowseFilter {
                session_key: Some("claude-code:abc".to_string()),
                agent: None,
                limit: 10,
            })
            .await
            .unwrap();
        assert_eq!(browsed.len(), 1);
        let full = store.get_full("m-11").await.unwrap();
        assert!(full.is_some());
        let context = store.context("claude-code:abc", now).await.unwrap();
        assert_eq!(context, "hello world");
        let tags = store.tags(TagAction::Accept).await.unwrap();
        assert_eq!((tags.moved, tags.dropped), (0, 0));
    });
}

#[test]
fn job_queue_claims_in_order() {
    block_on(async {
        let queue: Arc<dyn JobQueue> = Arc::new(FakeJobQueue::default());
        queue
            .enqueue(Job {
                name: JobName::EmbedTarget,
                payload: wire::JsonObject::default(),
                key: Some("task:1".to_string()),
                key_mode: JobKeyMode::Replace,
                priority: 0,
                run_at: None,
            })
            .await
            .unwrap();
        let claimed = queue
            .claim(WorkerRole::Embedder)
            .await
            .unwrap()
            .expect("expected one queued job");
        assert_eq!(claimed.name, JobName::EmbedTarget);
        queue.complete(claimed.id).await.unwrap();
        assert!(queue.claim(WorkerRole::Embedder).await.unwrap().is_none());
    });
}

#[test]
fn provider_streams_scripted_then_default() {
    block_on(async {
        let provider: Arc<dyn ModelProvider> =
            Arc::new(FakeProvider::new(vec![Script::Stream(vec![
                ModelChunk::Text {
                    delta: "scripted".to_string(),
                },
                ModelChunk::Done,
            ])]));
        let request = ModelRequest {
            model: "fake-model".to_string(),
            messages: vec![wire::message::Message {
                role: Role::User,
                content: wire::message::MessageContent::Text("hi".to_string()),
                tool_calls: None,
                tool_call_id: None,
            }],
            tools: Vec::new(),
            thinking: Some(wire::stream::ThinkingLevel::Medium),
            max_tokens: None,
            temperature: None,
        };
        let mut rx = provider.stream(request.clone()).await.unwrap();
        let first = rx.recv().await.expect("expected a chunk");
        assert_eq!(
            first,
            ModelChunk::Text {
                delta: "scripted".to_string()
            }
        );
        assert_eq!(rx.recv().await, Some(ModelChunk::Done));
        let mut fallback = provider.stream(request.clone()).await.unwrap();
        let echoed = fallback.recv().await.expect("expected the default chunk");
        assert_eq!(
            echoed,
            ModelChunk::Text {
                delta: "hi".to_string()
            }
        );
        let chat = provider.chat(request).await.unwrap();
        assert_eq!(chat.content.as_deref(), Some("ok"));
        let embeddings = provider.embed(vec!["abc".to_string()]).await.unwrap();
        assert_eq!(
            embeddings.first().map(|e| e.vector.clone()),
            Some(vec![3.0_f32])
        );
        let models = provider.list_models().await.unwrap();
        assert_eq!(models, vec!["fake-model".to_string()]);
    });
}

#[test]
fn harness_delivers_session_events() {
    block_on(async {
        let session = SessionId::from_str("grok-build:native-1").unwrap();
        let events = vec![
            SessionEvent::ToolCall(ToolCallInfo {
                tool_call_id: "t1".to_string(),
                name: "shell".to_string(),
                input: wire::JsonObject::default(),
            }),
            SessionEvent::ToolCallUpdate(ToolCallUpdate {
                tool_call_id: "t1".to_string(),
                status: ToolCallStatus::Completed,
                output: Some("done".to_string()),
            }),
            SessionEvent::Plan(vec![PlanEntry {
                content: "step".to_string(),
                priority: PlanPriority::High,
            }]),
            SessionEvent::AvailableCommands(vec![Command {
                name: "compact".to_string(),
                description: "compact context".to_string(),
            }]),
            SessionEvent::TurnEnded,
        ];
        let adapter: Arc<dyn HarnessAdapter> = Arc::new(FakeHarness::new(events));
        let created = adapter
            .new_session(SessionSpec {
                session: session.clone(),
                harness: HarnessId::GrokBuild,
                model: None,
                effort: None,
                working_dir: None,
            })
            .await
            .unwrap();
        assert_eq!(created, session);
        let mut rx = adapter.events(&session).await.unwrap();
        assert!(adapter.events(&session).await.is_err());
        adapter
            .prompt(&session, TurnInput::Text("go".to_string()))
            .await
            .unwrap();
        let mut seen = Vec::new();
        for _ in 0..5 {
            let event = rx.recv().await.expect("expected an event");
            seen.push(event);
        }
        assert_eq!(seen.len(), 5);
        adapter
            .answer_permission(&session, "r1", Decision::Allow)
            .await
            .unwrap();
        adapter
            .set_option(&session, SessionOption::Model("m".to_string()))
            .await
            .unwrap();
    });
}

#[test]
fn mux_spawn_kill_list() {
    block_on(async {
        let mux: Arc<dyn TerminalMux> = Arc::new(FakeMux::default());
        let session =
            SessionId::from_str("claude-code:11111111-2222-4333-8444-555555555555").unwrap();
        let id = mux
            .spawn(
                &session,
                vec!["bash".to_string()],
                std::path::Path::new("/work/acme"),
                TermSize { cols: 80, rows: 24 },
            )
            .await
            .unwrap();
        let rx = mux.attach(&id).await.unwrap();
        mux.input(&id, b"ls\n".to_vec()).await.unwrap();
        mux.resize(
            &id,
            TermSize {
                cols: 120,
                rows: 40,
            },
        )
        .await
        .unwrap();
        let listed = mux.list().await.unwrap();
        assert!(listed.first().is_some_and(|pty| pty.running));
        mux.kill(&id).await.unwrap();
        let listed = mux.list().await.unwrap();
        assert!(listed.first().is_some_and(|pty| !pty.running));
        drop(rx);
    });
}

#[test]
fn tool_echoes_text_argument() {
    block_on(async {
        let tool: Arc<dyn Tool> = Arc::new(FakeTool::new("echo"));
        let definition = tool.definition();
        assert_eq!(definition.name.as_ref(), "echo");
        let args = serde_json::json!({"text": "ping"});
        let result = tool
            .call(
                args.as_object().cloned(),
                ToolContext {
                    agent_id: Some("agent-a".to_string()),
                    working_dir: None,
                    session: None,
                },
            )
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false));
        let missing = tool
            .call(
                None,
                ToolContext {
                    agent_id: None,
                    working_dir: None,
                    session: None,
                },
            )
            .await;
        assert!(missing.is_err());
    });
}

#[test]
fn reader_reads_and_tails() {
    block_on(async {
        let session = SessionId::from_str("codex:native-9").unwrap();
        let early = Turn {
            role: Role::User,
            content: "early".to_string(),
            at: DateTime::parse_from_rfc3339("2026-10-12T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        };
        let late = Turn {
            role: Role::Assistant,
            content: "late".to_string(),
            at: DateTime::parse_from_rfc3339("2026-10-12T01:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        };
        let mut turns = BTreeMap::new();
        turns.insert(session.to_string(), vec![early.clone(), late.clone()]);
        let reader: Arc<dyn TranscriptReader> = Arc::new(FakeReader::new(turns));
        let listed = reader
            .sessions(std::path::Path::new("/work/acme/.claude"))
            .await
            .unwrap();
        assert_eq!(listed, vec![session.clone()]);
        let read = reader.read(&session).await.unwrap();
        assert_eq!(read.len(), 2);
        let since = DateTime::parse_from_rfc3339("2026-10-12T00:30:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let tail = reader.tail(&session, since).await.unwrap();
        assert_eq!(tail, vec![late]);
    });
}
