use std::sync::Arc;

use agent_loop::{Error, StopReason};
use kernel::agent::TurnUsage;
use kernel::model::ModelChunk;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use wire::hooks::HookContextKind;
use wire::message::{Message, MessageContent, Role};
use wire::stream::LlmUsage;

use crate::support::provider::{Plan, Provider, chunks, failure};
use crate::support::{Harness, agent, call, names, text, turn};

#[tokio::test]
async fn reports_registered_tool_names() {
    let mut harness = Harness::new().await;
    let provider = Provider::new(Vec::new());
    let tool = Arc::new(testkit::FakeTool::new("echo").expect("echo tool schema"));
    let agent = agent(provider, vec![tool], &harness);
    assert_eq!(agent.tool_names(), vec!["echo".to_owned()]);
    harness.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn streams_reasoning_tools_results_and_usage_through_ports() {
    let mut h = Harness::new().await;
    let usage = LlmUsage {
        prompt_tokens: 7,
        completion_tokens: 3,
        reasoning_tokens: Some(1),
        cached_tokens: None,
        cache_creation_tokens: None,
        cache_read_tokens: None,
    };
    let mut first = vec![
        text("checking"),
        ModelChunk::Reasoning {
            delta: "think 🦀".to_owned(),
        },
    ];
    first.extend(call(0, "echo", r#"{"text":"héllo 🦀"}"#));
    first.push(ModelChunk::Usage {
        usage: usage.clone(),
    });
    first.push(ModelChunk::Done);
    let second = vec![
        text("finished"),
        ModelChunk::Usage { usage },
        ModelChunk::Done,
    ];
    let expected_events = first.iter().chain(&second).cloned().collect::<Vec<_>>();
    let provider = Provider::new(vec![chunks(first), chunks(second)]);
    let tool = Arc::new(testkit::FakeTool::new("echo").expect("echo tool schema"));
    let agent = agent(provider.clone(), vec![tool], &h);
    let mut input = turn();
    input.authorized_tools = names(&["echo"]);
    let user = input.request.messages.clone();
    let (sender, mut receiver) = mpsc::channel(32);
    let result = agent
        .run(input, sender, CancellationToken::new())
        .await
        .expect("turn completes");
    assert_eq!(result.response, "finished");
    assert_eq!(result.stop, StopReason::Completed);
    assert_eq!(result.iterations, 2);
    assert_eq!(result.tools_used, ["echo"]);
    assert_eq!(
        result.usage,
        Some(TurnUsage {
            prompt_tokens: 14,
            completion_tokens: 6
        })
    );
    let mut events = Vec::new();
    while let Some(event) = tokio::time::timeout(std::time::Duration::from_secs(1), receiver.recv())
        .await
        .expect("stream event")
    {
        events.push(event);
    }
    assert_eq!(events, expected_events);
    let requests = provider.requests.lock().clone();
    assert_feedback(&requests, &user, &result.messages);
    h.close().await;
    let records = h.records().await;
    assert_eq!(records.len(), 8);
    assert!(matches!(
        records.first().expect("turn before").context.kind,
        HookContextKind::TurnBefore { .. }
    ));
    assert!(matches!(
        records.get(3).expect("tool before").context.kind,
        HookContextKind::ToolBefore {
            blocked: Some(false),
            ..
        }
    ));
    assert!(matches!(
        records.last().expect("turn after").context.kind,
        HookContextKind::TurnAfter {
            aborted: false,
            iterations: 2,
            ..
        }
    ));
}

fn assert_feedback(
    requests: &[kernel::model::ModelRequest],
    user: &[Message],
    messages: &[Message],
) {
    let [first, second] = requests else {
        panic!("two requests")
    };
    assert_eq!(first.messages, user);
    assert_eq!(first.tools.len(), 1);
    assert_eq!(
        second.messages,
        messages.get(..3).expect("feedback messages")
    );
    let [_, assistant, tool] = second.messages.as_slice() else {
        panic!("user assistant tool")
    };
    assert_eq!(assistant.reasoning.as_deref(), Some("think 🦀"));
    assert_eq!(tool.role, Role::Tool);
    assert_eq!(tool.tool_call_id.as_deref(), Some("call-0"));
    let MessageContent::Text(body) = &tool.content else {
        panic!("tool body")
    };
    let tool_result: rmcp::model::CallToolResult = serde_json::from_str(body).expect("MCP result");
    assert_eq!(
        tool_result,
        rmcp::model::CallToolResult::success(vec![rmcp::model::ContentBlock::text("héllo 🦀")])
    );
}

#[tokio::test]
async fn step_limit_bounds_provider_requests_and_zero_runs_none() {
    for steps in [0, 1] {
        let mut h = Harness::new().await;
        let mut response = call(0, "echo", r#"{"text":"x"}"#);
        response.push(ModelChunk::Done);
        let provider = Provider::new(vec![chunks(response)]);
        let agent = agent(
            provider.clone(),
            vec![Arc::new(
                testkit::FakeTool::new("echo").expect("echo tool schema"),
            )],
            &h,
        );
        let mut input = turn();
        input.limits.steps = steps;
        input.authorized_tools = names(&["echo"]);
        let (sender, _receiver) = mpsc::channel(32);
        let result = agent
            .run(input, sender, CancellationToken::new())
            .await
            .expect("bounded turn");
        assert_eq!(result.stop, StopReason::StepLimit);
        assert_eq!(result.iterations, steps);
        assert_eq!(provider.requests.lock().len(), steps as usize);
        h.close().await;
    }
}

#[tokio::test]
async fn invalid_streams_are_typed_errors_and_never_run_tools() {
    let cases = vec![
        chunks(vec![text("missing done")]),
        chunks(vec![
            ModelChunk::ToolCallDelta {
                index: 8,
                arguments_delta: "{}".to_owned(),
            },
            ModelChunk::Done,
        ]),
        chunks(vec![
            ModelChunk::ToolCallStart {
                index: 0,
                id: "a".to_owned(),
                name: "echo".to_owned(),
            },
            ModelChunk::ToolCallStart {
                index: 0,
                id: "b".to_owned(),
                name: "echo".to_owned(),
            },
            ModelChunk::Done,
        ]),
        chunks([call(0, "echo", "[]"), vec![ModelChunk::Done]].concat()),
        chunks([call(0, "echo", "{"), vec![ModelChunk::Done]].concat()),
        Plan::Chunks(vec![Err(failure())]),
        Plan::Fail,
    ];
    for plan in cases {
        let mut h = Harness::new().await;
        let provider = Provider::new(vec![plan]);
        let agent = agent(
            provider,
            vec![Arc::new(
                testkit::FakeTool::new("echo").expect("echo tool schema"),
            )],
            &h,
        );
        let (sender, _receiver) = mpsc::channel(32);
        let error = agent
            .run(turn(), sender, CancellationToken::new())
            .await
            .expect_err("invalid stream");
        assert!(matches!(
            error,
            Error::Stream(_) | Error::Json(_) | Error::Provider(_)
        ));
        h.close().await;
        let records = h.records().await;
        assert!(
            records
                .iter()
                .any(|record| matches!(record.context.kind, HookContextKind::ProviderError { .. }))
        );
        assert!(
            !records
                .iter()
                .any(|record| matches!(record.context.kind, HookContextKind::ToolBefore { .. }))
        );
    }
}

#[tokio::test]
async fn authorization_defaults_to_refusal_and_tool_failures_feed_back() {
    for (authorized, args, expected_used) in [(false, r#"{"text":"x"}"#, 0), (true, "{}", 1)] {
        let mut h = Harness::new().await;
        let provider = Provider::new(vec![
            chunks([call(0, "echo", args), vec![ModelChunk::Done]].concat()),
            chunks(vec![text("recovered"), ModelChunk::Done]),
        ]);
        let agent = agent(
            provider.clone(),
            vec![Arc::new(
                testkit::FakeTool::new("echo").expect("echo tool schema"),
            )],
            &h,
        );
        let mut input = turn();
        if authorized {
            input.authorized_tools = names(&["echo"]);
        }
        let (sender, _receiver) = mpsc::channel(32);
        let result = agent
            .run(input, sender, CancellationToken::new())
            .await
            .expect("error is feedback");
        assert_eq!(result.tools_used.len(), expected_used);
        let requests = provider.requests.lock().clone();
        let [first, second] = requests.as_slice() else {
            panic!("two requests")
        };
        assert_eq!(first.tools.len(), usize::from(authorized));
        let Message {
            content: MessageContent::Text(body),
            ..
        } = second.messages.get(2).expect("tool feedback")
        else {
            panic!("tool feedback")
        };
        let result: rmcp::model::CallToolResult = serde_json::from_str(body).expect("typed result");
        assert_eq!(result.is_error, Some(true));
        h.close().await;
    }
}

#[tokio::test]
async fn safety_hooks_refuse_authorized_tools_before_execution() {
    for (tool, args) in [
        (
            "shell",
            r#"{"command":"rm -rf /","text":"must not execute"}"#,
        ),
        (
            "file_read",
            r#"{"path":"/outside/workspace","text":"must not execute"}"#,
        ),
        ("file_read", r#"{"path":5,"text":"must not execute"}"#),
    ] {
        let mut h = Harness::new().await;
        let provider = Provider::new(vec![
            chunks([call(0, tool, args), vec![ModelChunk::Done]].concat()),
            chunks(vec![text("refusal received"), ModelChunk::Done]),
        ]);
        let runner = agent(
            provider,
            vec![Arc::new(
                testkit::FakeTool::new(tool).expect("echo tool schema"),
            )],
            &h,
        );
        let mut input = turn();
        input.authorized_tools = names(&[tool]);
        let (sender, _receiver) = mpsc::channel(16);
        let result = runner
            .run(input, sender, CancellationToken::new())
            .await
            .expect("refusal feedback");
        assert_eq!(result.tools_used, Vec::<String>::new());
        assert_eq!(result.response, "refusal received");
        h.close().await;
        let records = h.records().await;
        let blocked = records
            .iter()
            .find(|record| matches!(record.context.kind, HookContextKind::ToolBefore { .. }))
            .expect("before event");
        assert!(blocked.aborted);
        assert_eq!(blocked.errors.len(), 1);
    }
}
