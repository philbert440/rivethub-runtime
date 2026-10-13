use std::time::Duration;

use async_trait::async_trait;
use capture::writer::CaptureTransport as _;
use chrono::DateTime;
use chrono::Utc;
use parking_lot::Mutex;
use wire::capture::CaptureResult;

fn spool_ok(conversation: &str) -> CaptureResult {
    CaptureResult {
        ok: true,
        conversation_id: conversation.to_owned(),
        inserted: 1,
        skipped: 0,
    }
}

fn spool_error(status: u16) -> capture::writer::PostError {
    if (400..500).contains(&status) {
        capture::writer::PostError::Client { status }
    } else {
        capture::writer::PostError::Server { status }
    }
}

#[derive(Default)]
struct ChoiceTransport {
    results: Mutex<Vec<Result<CaptureResult, capture::writer::PostError>>>,
}

impl ChoiceTransport {
    fn failing(error: capture::writer::PostError) -> Self {
        Self {
            results: Mutex::new(vec![Err(error)]),
        }
    }
}

#[async_trait]
impl capture::writer::CaptureTransport for ChoiceTransport {
    async fn post(&self, _body: String) -> Result<CaptureResult, capture::writer::PostError> {
        self.results
            .lock()
            .pop()
            .unwrap_or_else(|| Ok(spool_ok("c-x")))
    }
}

fn now_at(text: &str) -> Result<DateTime<Utc>, chrono::ParseError> {
    text.parse()
}

fn reqwest_client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .build()
}

type Seen = std::sync::Arc<Mutex<Vec<(Option<String>, String)>>>;

async fn handle_capture(
    axum::extract::State(seen): axum::extract::State<Seen>,
    request: axum::extract::Request,
) -> Result<axum::Json<CaptureResult>, axum::http::StatusCode> {
    let content_type = request
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let body = axum::body::to_bytes(request.into_body(), 1024 * 1024)
        .await
        .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;
    seen.lock()
        .push((content_type, String::from_utf8_lossy(&body).to_string()));
    Ok(axum::Json(CaptureResult {
        ok: true,
        conversation_id: "c-7".to_owned(),
        inserted: 1,
        skipped: 0,
    }))
}

async fn http_transport_posts_and_parses() -> Result<(), Box<dyn std::error::Error>> {
    let seen: Seen = std::sync::Arc::new(Mutex::new(Vec::new()));
    let routes = axum::Router::new()
        .route("/api/capture", axum::routing::post(handle_capture))
        .with_state(seen.clone());
    let stub = testkit::stub_server(routes).await?;
    let transport = capture::writer::HttpCaptureTransport::new(
        format!("http://{}", stub.addr),
        reqwest_client()?,
    );
    let batch = capture::batch::parse_batch(
        r#"{"session_key":"s","agent":"a","messages":[{"event_id":"e1","role":"user","content":"hi"}]}"#,
    )?;
    let body = serde_json::to_string(&batch)?;
    let result = transport.post(body).await?;
    assert_eq!(result.conversation_id, "c-7");
    let recorded = seen.lock().clone();
    let [entry] = recorded.as_slice() else {
        return Err("expected exactly one capture post".into());
    };
    assert_eq!(entry.0.as_deref(), Some("application/json"));
    let posted: testkit::Json = serde_json::from_str(&entry.1)?;
    let event_id = posted
        .get("messages")
        .and_then(|messages| messages.get(0))
        .and_then(|message| message.get("event_id"))
        .and_then(testkit::Json::as_str);
    assert_eq!(event_id, Some("e1"));
    stub.close().await;
    Ok(())
}

async fn http_transport_maps_4xx_to_client_errors() -> Result<(), Box<dyn std::error::Error>> {
    let routes = axum::Router::new().route(
        "/api/capture",
        axum::routing::post(|| async { (axum::http::StatusCode::BAD_REQUEST, "bad") }),
    );
    let other = axum::Router::new().route("/api/capture", axum::routing::post(|| async { "nope" }));
    let stub = testkit::stub_server(routes).await?;
    let transport = capture::writer::HttpCaptureTransport::new(
        format!("http://{}", stub.addr),
        reqwest_client()?,
    );
    let Err(error) = transport.post(r#"{"bad":1}"#.to_owned()).await else {
        return Err("expected a client error for a 400 response".into());
    };
    assert!(matches!(
        error,
        capture::writer::PostError::Client { status: 400 }
    ));
    stub.close().await;

    let stub = testkit::stub_server(other).await?;
    let transport = capture::writer::HttpCaptureTransport::new(
        format!("http://{}", stub.addr),
        reqwest_client()?,
    );
    let Err(error) = transport.post("{}".to_owned()).await else {
        return Err("expected a parse failure for a text response".into());
    };
    assert!(matches!(error, capture::writer::PostError::Network { .. }));
    stub.close().await;
    Ok(())
}

async fn spool_round_trip_and_dead_letters() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let spool = capture::spool::CaptureSpool::new(dir.path().to_path_buf());
    let now = now_at("2026-10-10T12:00:00Z")?;
    let first = spool.spool(r#"{"session_key":"a"}"#, now)?;
    spool.spool(r#"{"session_key":"b"}"#, now + Duration::from_millis(5))?;
    let extension = first
        .extension()
        .ok_or("the spool file has no extension")?
        .to_string_lossy()
        .into_owned();
    assert_eq!(extension, "json");
    let permissions = std::fs::metadata(&first)?.permissions();
    assert_eq!(
        std::os::unix::fs::PermissionsExt::mode(&permissions) & 0o777,
        0o600
    );

    let posted = Mutex::new(Vec::new());
    spool
        .replay(50, |body| {
            posted.lock().push(body);
            async { Ok(spool_ok("c-1")) }
        })
        .await?;
    assert_eq!(posted.lock().len(), 2);
    assert_eq!(std::fs::read_dir(dir.path())?.count(), 0);

    spool.spool(r#"{"session_key":"c"}"#, now)?;
    spool
        .replay(50, |_body| async { Err(spool_error(400)) })
        .await?;
    let dead = dir.path().join("dead");
    assert!(dead.is_dir());
    assert_eq!(std::fs::read_dir(&dead)?.count(), 1);

    spool.spool(r#"{"session_key":"d"}"#, now)?;
    let stopped = spool
        .replay(50, |_body| async { Err(spool_error(503)) })
        .await;
    assert!(stopped.is_err());
    Ok(())
}

async fn write_spools_when_the_server_fails() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let spool = capture::spool::CaptureSpool::new(dir.path().join("spool"));
    let transport = ChoiceTransport::failing(spool_error(503));
    let writer = capture::writer::CaptureWriter::new(transport, spool, 768 * 1024, None);
    let batch = capture::batch::parse_batch(
        r#"{"session_key":"s","agent":"a","messages":[{"event_id":"e1","role":"user","content":"hi"}]}"#,
    )?;
    let Err(error) = writer.write(&batch, now_at("2026-10-10T12:00:00Z")?).await else {
        return Err("expected the 503 response to fail the write".into());
    };
    assert!(error.to_string().contains("capture HTTP 503"));
    let spooled = std::fs::read_dir(dir.path().join("spool"))?.count();
    assert_eq!(spooled, 1);
    Ok(())
}

async fn write_replays_the_spool_first() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let spool = capture::spool::CaptureSpool::new(dir.path().join("spool"));
    let now = now_at("2026-10-10T12:00:00Z")?;
    spool.spool(r#"{"session_key":"old"}"#, now)?;
    let transport = ChoiceTransport::default();
    let writer = capture::writer::CaptureWriter::new(transport, spool, 768 * 1024, None);
    let batch = capture::batch::parse_batch(
        r#"{"session_key":"s","agent":"a","messages":[{"event_id":"e1","role":"user","content":"hi"}]}"#,
    )?;
    let totals = writer.write(&batch, now).await?;
    assert_eq!(totals.inserted, 1);
    let remaining = std::fs::read_dir(dir.path().join("spool"))?.count();
    assert_eq!(remaining, 0);
    Ok(())
}

#[test]
fn spool_and_transport_behaviours() -> Result<(), Box<dyn std::error::Error>> {
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(http_transport_posts_and_parses())?;
    runtime.block_on(http_transport_maps_4xx_to_client_errors())?;
    runtime.block_on(spool_round_trip_and_dead_letters())?;
    runtime.block_on(write_spools_when_the_server_fails())?;
    runtime.block_on(write_replays_the_spool_first())
}
