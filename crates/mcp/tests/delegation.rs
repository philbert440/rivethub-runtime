use std::sync::Arc;
use std::time::Duration;

use kernel::tasks::AgentDirectory;
use kernel::tasks::DelegateOutcome;
use kernel::tasks::DelegateRequest;
use kernel::tasks::Delegation;
use kernel::tasks::DelegationError;
use kernel::tasks::PresetAgent;
use kernel::tasks::TaskStatus;
use mcp::Server;
use mcp::Surface;
use rmcp::ServiceExt;
use rmcp::model::CallToolRequest;
use rmcp::model::CallToolRequestParams;
use rmcp::model::ClientConfig;
use rmcp::model::ClientRequest;
use rmcp::model::ProtocolVersion;
use rmcp::service::PeerRequestOptions;
use testkit::FakeMemoryStore;
use tokio::sync::Mutex;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Debug)]
struct Fake {
    directory: AgentDirectory,
    requests: Mutex<Vec<DelegateRequest>>,
    entered: Notify,
    finished: Notify,
    hold: bool,
    outcome: DelegateOutcome,
}

#[async_trait::async_trait]
impl Delegation for Fake {
    async fn agents(&self) -> Result<AgentDirectory, DelegationError> {
        Ok(self.directory.clone())
    }

    async fn delegate(
        &self,
        request: DelegateRequest,
        cancel: CancellationToken,
    ) -> Result<DelegateOutcome, DelegationError> {
        self.requests.lock().await.push(request);
        self.entered.notify_one();
        if self.hold {
            cancel.cancelled().await;
            self.finished.notify_one();
            return Err(DelegationError::Cancelled);
        }
        self.finished.notify_one();
        Ok(self.outcome.clone())
    }
}

fn directory() -> AgentDirectory {
    AgentDirectory {
        presets: vec![PresetAgent {
            id: "preset-id".to_owned(),
            name: "worker".to_owned(),
            node: None,
            harness: Some(kernel::tasks::Harness::Codex),
            directory: None,
            local: true,
        }],
        runtime: Vec::new(),
    }
}

fn outcome() -> DelegateOutcome {
    DelegateOutcome {
        id: Uuid::nil(),
        status: TaskStatus::Completed,
        summary: None,
        error: Some("kept".to_owned()),
        child: Some("codex:child".to_owned()),
        node: "a".to_owned(),
        tokens: None,
        timed_out: false,
    }
}

fn fake(hold: bool) -> Arc<Fake> {
    Arc::new(Fake {
        directory: directory(),
        requests: Mutex::new(Vec::new()),
        entered: Notify::new(),
        finished: Notify::new(),
        hold,
        outcome: outcome(),
    })
}

#[expect(clippy::expect_used, reason = "a builder failure is a broken fixture")]
fn server(port: Arc<Fake>, surface: Surface) -> Server {
    let port: Arc<dyn Delegation> = port;
    Server::builder(
        Arc::new(FakeMemoryStore::default()),
        false,
        CancellationToken::new(),
    )
    .delegation(port, surface)
    .build()
    .expect("server")
}

#[expect(
    clippy::expect_used,
    reason = "argument JSON and the protocol result are fixtures; a failure is a broken test"
)]
async fn call(server: &Server, name: &str, arguments: &str) -> rmcp::model::CallToolResult {
    let mut request = CallToolRequestParams::new(name.to_owned());
    request.arguments = Some(serde_json::from_str(arguments).expect("arguments"));
    server.call(request).await.expect("protocol")
}

#[expect(
    clippy::expect_used,
    reason = "a tool result without text is a broken fixture"
)]
fn text(result: &rmcp::model::CallToolResult) -> &str {
    &result
        .content
        .first()
        .expect("content")
        .as_text()
        .expect("text")
        .text
}

#[tokio::test]
async fn list_agents_returns_the_port_directory() {
    let port = fake(false);
    let server = server(Arc::clone(&port), Surface::Stdio);
    let result = call(&server, "list_agents", "{}").await;
    assert_eq!(result.is_error, None);
    assert_eq!(
        text(&result),
        "- worker (agent: codex)\n\nto_agent accepts a preset name or id, or a runtime agent id (agent@node pins the node).\n"
    );
}

#[tokio::test]
async fn delegate_task_joins_context_and_applies_the_default_timeout() {
    let port = fake(false);
    let server = server(Arc::clone(&port), Surface::Stdio);
    let result = call(
        &server,
        "delegate_task",
        r#"{"to_agent":"worker","task":"ship it","context":["one",""],"criteria":["tested"]}"#,
    )
    .await;
    assert_eq!(result.is_error, None);
    let rendered = text(&result);
    assert!(
        rendered.starts_with("[no response from the delegate]\n\n---\n_Delegation [completed]: "),
        "{rendered}"
    );
    assert!(
        rendered.ends_with(&format!(
            "task {} · session codex:child · node a",
            Uuid::nil()
        )),
        "{rendered}"
    );
    let request = port.requests.lock().await.first().expect("request").clone();
    assert_eq!(request.target, "worker");
    assert_eq!(request.prompt, "ship it\none\n");
    assert_eq!(request.criteria, vec!["tested".to_owned()]);
    assert_eq!(request.model, None);
    assert_eq!(request.timeout, Duration::from_mins(20));
}

#[tokio::test]
async fn delegate_task_rejects_bad_arguments() {
    let server = server(fake(false), Surface::Stdio);
    for arguments in [
        r#"{"to_agent":"","task":"ship"}"#,
        r#"{"to_agent":"worker","task":""}"#,
        r#"{"to_agent":"worker","task":"ship","timeout_ms":0}"#,
        r#"{"to_agent":"worker","task":"ship","timeout_ms":-1}"#,
        r#"{"to_agent":"worker","task":"ship","timeout_ms":1800001}"#,
        r#"{"to_agent":"worker","task":"ship","extra":1}"#,
        r#"{"to_agent":7,"task":"ship"}"#,
        r#"{"task":"ship"}"#,
    ] {
        let result = call(&server, "delegate_task", arguments).await;
        assert_eq!(result.is_error, Some(true), "{arguments}");
    }
    let result = call(&server, "list_agents", r#"{"extra":1}"#).await;
    assert_eq!(result.is_error, Some(true));
    let result = call(
        &server,
        "delegate_task",
        r#"{"to_agent":"worker","task":"ship","timeout_ms":1800000,"model":"demo","context":[]}"#,
    )
    .await;
    assert_eq!(result.is_error, None);
}

#[tokio::test]
async fn surfaces_register_delegate_task_only_on_stdio() {
    let remote = server(fake(false), Surface::Remote);
    let names: Vec<_> = remote
        .tools()
        .iter()
        .map(|tool| tool.name.to_string())
        .collect();
    assert!(names.contains(&"list_agents".to_owned()));
    assert!(!names.contains(&"delegate_task".to_owned()));
    assert_eq!(names.first().map(String::as_str), Some("list_agents"));
    let error = remote
        .call(CallToolRequestParams::new("delegate_task".to_owned()))
        .await
        .expect_err("absent");
    assert_eq!(error.code.0, -32602);
    assert_eq!(error.message, "Tool delegate_task not found");
    let plain = Server::new(
        Arc::new(FakeMemoryStore::default()),
        false,
        CancellationToken::new(),
    )
    .expect("server");
    assert!(
        plain
            .tools()
            .iter()
            .all(|tool| tool.name != "delegate_task" && tool.name != "list_agents")
    );
    let stdio = server(fake(false), Surface::Stdio);
    assert_eq!(
        stdio.tools().first().expect("first").name.as_ref(),
        "delegate_task"
    );
    assert_eq!(
        stdio.tools().get(1).expect("second").name.as_ref(),
        "list_agents"
    );
}

#[tokio::test]
async fn shutdown_during_delegate_task_lets_the_port_observe_cancel() {
    let port = fake(true);
    let shutdown = CancellationToken::new();
    let server = Server::builder(
        Arc::new(FakeMemoryStore::default()),
        false,
        shutdown.clone(),
    )
    .delegation(Arc::clone(&port) as Arc<dyn Delegation>, Surface::Stdio)
    .build()
    .expect("server");
    let running = tokio::spawn(async move {
        call(
            &server,
            "delegate_task",
            r#"{"to_agent":"worker","task":"ship"}"#,
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(2), port.entered.notified())
        .await
        .expect("entered");
    shutdown.cancel();
    let result = tokio::time::timeout(Duration::from_secs(2), running)
        .await
        .expect("joined")
        .expect("spawn");
    assert_eq!(result.is_error, Some(true));
    assert_eq!(text(&result), "operation cancelled");
    tokio::time::timeout(Duration::from_secs(2), port.finished.notified())
        .await
        .expect("port finished");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_client_cancel_reaches_the_delegation_port() {
    let port = fake(true);
    let server = server(Arc::clone(&port), Surface::Stdio);
    let (client_io, server_io) = tokio::io::duplex(65536);
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(mcp::transport::serve_io(
        server,
        server_io,
        shutdown.clone(),
    ));
    let mut config = ClientConfig::default();
    config.protocol_version = ProtocolVersion::V_2025_11_25;
    let client = config.serve(client_io).await.expect("initialize");
    let mut params = CallToolRequestParams::new("delegate_task".to_owned());
    params.arguments =
        Some(serde_json::from_str(r#"{"to_agent":"worker","task":"ship"}"#).expect("arguments"));
    let handle = client
        .send_cancellable_request(
            ClientRequest::CallToolRequest(CallToolRequest::new(params)),
            PeerRequestOptions::no_options(),
        )
        .await
        .expect("request");
    tokio::time::timeout(Duration::from_secs(2), port.entered.notified())
        .await
        .expect("entered");
    handle.cancel(Some("abort".into())).await.expect("cancel");
    tokio::time::timeout(Duration::from_secs(2), port.finished.notified())
        .await
        .expect("port finished");
    client.cancel().await.expect("client closed");
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .expect("serve joined")
        .expect("spawn")
        .expect("server");
}
