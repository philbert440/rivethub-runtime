use std::net::SocketAddr;
use std::time::Duration;

use super::TestResult;
use wire::gateway::v1::{ChatRequest, ChunkChoice, Completion, CompletionChunk, CompletionDelta};
use wire::message::Role;
use wire::stream::openai::{ChatCompletionRequest, FinishReason, Message};

#[derive(Debug)]
pub struct Provider {
    pub address: SocketAddr,
    cancel: tokio_util::sync::CancellationToken,
    task: tokio::task::JoinHandle<std::io::Result<()>>,
}

impl Provider {
    pub async fn start() -> TestResult<Self> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let chunk = CompletionChunk {
            id: "test-completion".into(),
            created: 0,
            model: "test".into(),
            choices: vec![ChunkChoice {
                index: 0,
                delta: CompletionDelta {
                    content: Some("hello back".into()),
                    ..CompletionDelta::default()
                },
                finish_reason: Some(FinishReason::Stop),
            }],
            usage: None,
        };
        let body = format!(
            "data: {}\n\ndata: [DONE]\n\n",
            serde_json::to_string(&chunk)?
        );
        let router = axum::Router::new().route(
            "/v1/chat/completions",
            axum::routing::post(
                move |axum::Json(request): axum::Json<ChatCompletionRequest>| {
                    let body = body.clone();
                    async move {
                        assert_eq!(request.model, "test");
                        (
                            [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
                            body,
                        )
                    }
                },
            ),
        );
        let cancel = tokio_util::sync::CancellationToken::new();
        let stopped = cancel.clone();
        let task = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(stopped.cancelled_owned())
                .await
        });
        Ok(Self {
            address,
            cancel,
            task,
        })
    }

    pub async fn close(self) -> TestResult {
        self.cancel.cancel();
        tokio::time::timeout(Duration::from_secs(5), self.task).await???;
        Ok(())
    }
}

pub async fn turns(client: &reqwest::Client, address: SocketAddr, channel_port: u16) -> TestResult {
    let request = ChatRequest {
        model: "test".into(),
        messages: vec![Message {
            role: Role::User,
            content: Some("hello".into()),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
        }],
        stream: None,
        max_tokens: None,
        stream_options: None,
    };
    let response: Completion = client
        .post(format!("https://{address}/v1/chat/completions"))
        .json(&request)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(
        response
            .choices
            .first()
            .ok_or("missing choice")?
            .message
            .content
            .as_deref(),
        Some("hello back")
    );
    let request = wire::mesh_channel::AgentMessage {
        agent_id: "main".into(),
        content: "hello".into(),
        session_key: None,
    };
    let response: wire::mesh_channel::AgentReply = client
        .post(format!("https://127.0.0.1:{channel_port}/message"))
        .json(&request)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(
        response,
        wire::mesh_channel::AgentReply {
            content: "hello back".into(),
            session_key: None
        }
    );
    Ok(())
}
