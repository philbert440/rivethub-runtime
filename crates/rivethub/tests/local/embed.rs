use wire::gateway::memory::MemorySearchResponse;
use wire::stream::openai::{EmbeddingRequest, EmbeddingResponse, EmbeddingRow};

use super::{TestResult, free_port, provider_base, start, stop, write_config};

const QUERY: &str = "when does acmekit deploy";

#[tokio::test]
async fn local_vector_search_embeds_the_query_with_the_sqlite_endpoint() -> TestResult {
    let (calls_tx, mut calls_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<String>>();
    let router = axum::Router::new().route(
        "/v1/embeddings",
        axum::routing::post(move |axum::Json(request): axum::Json<EmbeddingRequest>| {
            let calls = calls_tx.clone();
            async move {
                let rows = request
                    .input
                    .iter()
                    .enumerate()
                    .map(|(index, _)| EmbeddingRow {
                        index,
                        embedding: vec![0.6, 0.8],
                    })
                    .collect();
                drop(calls.send(request.input));
                axum::Json(EmbeddingResponse { data: rows })
            }
        }),
    );
    let server = testkit::stub_server(router).await?;
    let home = tempfile::tempdir()?;
    let port = free_port()?;
    let database = home.path().join(".rivethub").join("memory.sqlite");
    if let Some(parent) = database.parent() {
        std::fs::create_dir_all(parent)?;
    }
    migrate::sqlite(&database)?;
    write_config(
        home.path(),
        &provider_base(
            home.path(),
            "http://127.0.0.1:1",
            &format!(
                "memory:\n  backend: sqlite\n  sqlite:\n    path: {}\n    embed_endpoint: http://{}\n    embed_model: embedding-model\nden:\n  host: 127.0.0.1\n  port: {port}\n",
                database.display(),
                server.addr
            ),
        ),
    )?;
    let started = start(
        home.path(),
        &["local", "--no-lan"],
        &["gateway listening on "],
    )
    .await?;
    let searched = reqwest::Client::builder()
        .no_proxy()
        .build()?
        .get(format!(
            "http://127.0.0.1:{port}/api/memory/search?q={}&mode=vector",
            QUERY.replace(' ', "%20")
        ))
        .send()
        .await;
    stop(started).await?;
    let response = searched?
        .error_for_status()?
        .json::<MemorySearchResponse>()
        .await?;
    assert_eq!(response.degraded, None, "{response:?}");
    let mut calls = Vec::new();
    while let Ok(input) = calls_rx.try_recv() {
        calls.push(input);
    }
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert!(
        calls.iter().flatten().any(|input| input.contains(QUERY)),
        "{calls:?}"
    );
    Ok(())
}
