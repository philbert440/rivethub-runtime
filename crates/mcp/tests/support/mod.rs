#![cfg(test)]

use mcp::Server;
use rmcp::model::{CallToolRequestParams, CallToolResult};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub fn server() -> Server {
    Server::new(
        Arc::new(testkit::FakeMemoryStore::default()),
        false,
        CancellationToken::new(),
    )
    .expect("server")
}

pub async fn call(server: &Server, name: &str, arguments: &str) -> CallToolResult {
    let mut request = CallToolRequestParams::new(name.to_owned());
    request.arguments = Some(serde_json::from_str(arguments).expect("arguments"));
    server.call(request).await.expect("tool protocol result")
}
