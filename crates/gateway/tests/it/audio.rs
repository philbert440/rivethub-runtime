use crate::common;

#[path = "audio/mic.rs"]
mod mic;
#[path = "audio/upstream.rs"]
mod upstream;
#[path = "audio/voice.rs"]
mod voice;

use mic::{
    connect, destroyed, expect_fault, hello, reject_before_arm, reject_odd_pcm, round_trip,
    second_publisher_is_busy, send_text, shutdown_sends_close,
};
use upstream::Huge;

use std::net::SocketAddr;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use kernel::memory::MemoryStore;
use serde::Deserialize;
use serde::Serialize;
use tokio_util::sync::CancellationToken;
use wire::gateway::GatewayError;
use wire::gateway::devices::AudioBackend;
use wire::gateway::devices::AudioFormat;
use wire::gateway::devices::AudioHealth;
use wire::gateway::devices::AudioMicErrorCode;
use wire::gateway::devices::AudioStatus;

const CLAIMS: &[&str] = &[
    "plain-main/static-audio-nested",
    "plain-main/alias-api-audio",
    "plain-main/alias-api-audio-status",
    "plain-main/alias-api-audio-health",
    "plain-main/alias-api-audio-mic-http",
    "plain-main/audio-disabled",
    "ws-main/upgrade-audio-mic-disabled",
    "ws-main/upgrade-alias-audio-mic",
];

const SKIPS: &[(&str, &str)] = &[];

type MicSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

#[derive(Deserialize)]
struct WsDocument {
    runs: std::collections::BTreeMap<String, WsRun>,
}

#[derive(Deserialize)]
struct WsRun {
    #[serde(default)]
    steps: Vec<WsStep>,
}

#[derive(Deserialize)]
struct WsStep {
    id: String,
}

#[derive(Clone)]
struct Seen {
    hits: Arc<tokio::sync::Mutex<Vec<Hit>>>,
    stop: CancellationToken,
}

struct Hit {
    path: String,
    body: Vec<u8>,
}

#[derive(Serialize)]
struct Heard {
    text: String,
    ignored: bool,
}

#[derive(Deserialize)]
struct Spoken {
    input: String,
    response_format: String,
    #[serde(default)]
    instructions: Option<String>,
    #[serde(default)]
    voice: Option<String>,
}

struct Upstream {
    addr: SocketAddr,
    seen: Seen,
    token: CancellationToken,
    server: tokio::task::JoinHandle<Result<(), std::io::Error>>,
}

struct Running {
    gateway: Arc<gateway::Gateway>,
    addr: SocketAddr,
    token: CancellationToken,
    server: tokio::task::JoinHandle<Result<(), gateway::server::ServeError>>,
}

struct AudioLive {
    running: Running,
    dir: PathBuf,
    _keep: tempfile::TempDir,
}

#[test]
fn recorded_audio_http_matches() {
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let handle = runtime.handle().clone();
    let cases = testkit::http::cases(&testkit::recording("gateway/http-plain.json")).expect("http");
    let (gateway, _store) = common::gateway();
    let app = gateway.router(false);
    let masks = testkit::Masks {
        port: "5174".to_owned(),
        hostname: Some("gold-host".to_owned()),
        tmp: Some("/tmp/golden".to_owned()),
    };
    for id in CLAIMS.iter().copied().filter(|id| id.starts_with("plain-")) {
        let Some(case) = cases.iter().find(|case| case.id == id) else {
            panic!("missing recording {id}");
        };
        let app = app.clone();
        let outcome = testkit::http::replay(case, &masks, |request| {
            match handle.block_on(common::answer(app.clone(), request.clone())) {
                Ok(response) => response,
                Err(error) => testkit::Response::json(599, &error).expect("error body serializes"),
            }
        });
        if let Err(mismatch) = outcome {
            panic!("{}: {}", mismatch.id, mismatch.detail);
        }
    }
}

#[test]
fn audio_named_scenarios_are_claimed_or_skipped() -> Result<(), String> {
    let recorded = audio_ids()?;
    reject_duplicates(CLAIMS, "claim");
    let mut skip_ids = Vec::new();
    for (id, reason) in SKIPS {
        assert!(!reason.trim().is_empty(), "empty skip reason for {id}");
        assert!(names_audio(id), "skip is outside this slice: {id}");
        skip_ids.push(*id);
    }
    reject_duplicates(&skip_ids, "skip");
    for id in &skip_ids {
        assert!(
            recorded.iter().any(|candidate| candidate == id),
            "skip is not in the recordings: {id}"
        );
    }
    for id in CLAIMS {
        assert!(
            recorded.iter().any(|candidate| candidate == id),
            "claim is not an audio recording: {id}"
        );
        assert!(!skip_ids.contains(id), "{id} is both claimed and skipped");
    }
    for id in &recorded {
        let claimed = CLAIMS.contains(&id.as_str());
        let skipped = skip_ids.contains(&id.as_str());
        assert!(claimed != skipped, "{id} must be claimed or skipped once");
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disabled_routes_share_the_refusal() -> Result<(), String> {
    let gateway = Arc::new(base());
    let app = gateway.router(false);
    for (method, target) in [
        ("POST", "/audio"),
        ("DELETE", "/api/audio/health"),
        ("PUT", "/audio/status"),
        ("GET", "/audio/mic"),
    ] {
        let response = call(&app, method, target, Vec::new(), &[]).await?;
        assert_eq!(response.status, 503);
        let error = gateway_error(&response)?;
        assert_eq!(error.error, "audio mic bridge disabled on this node");
        assert_eq!(error.hint.as_deref(), Some("set RIVETOS_DEN_AUDIO=1"));
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enabled_audio_reports_status_health_and_modes() -> Result<(), String> {
    let keep = tempfile::tempdir().expect("dir");
    let dir = keep.path().join("audio");
    let gateway = Arc::new(base().with_audio(gateway::AudioConfig::new(dir.clone())));
    let app = gateway.router(false);
    let status = audio_status(&call(&app, "GET", "/audio", Vec::new(), &[]).await?)?;
    assert_idle(&status, &dir)?;
    assert_eq!(perm(&dir)?, 0o700);
    assert_eq!(perm(&dir.join("mic.pcm"))?, 0o600);
    assert!(is_fifo(&dir.join("mic.pcm"))?);
    let health = call(&app, "GET", "/audio/health", Vec::new(), &[]).await?;
    assert_eq!(health.status, 200);
    let body: AudioHealth = serde_json::from_slice(&health.body).expect("health");
    assert!(body.ok);
    assert!(body.runtime_ready);
    assert!(body.message.is_none());
    assert_eq!(body.fifo_path, status.fifo_path);
    assert_eq!(
        call(&app, "HEAD", "/api/audio", Vec::new(), &[])
            .await?
            .status,
        200
    );
    let alias = audio_status(&call(&app, "GET", "/api/audio/status", Vec::new(), &[]).await?)?;
    assert_eq!(alias.fifo_path, status.fifo_path);
    assert_error(
        &call(&app, "GET", "/audio/missing", Vec::new(), &[]).await?,
        404,
        "not found",
    )?;
    assert_error(
        &call(&app, "POST", "/audio", Vec::from(b"{}" as &[u8]), &[]).await?,
        405,
        "method not allowed",
    )?;
    assert_error(
        &call(&app, "GET", "/audio/mic", Vec::new(), &[]).await?,
        404,
        "not found",
    )?;
    close(gateway).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn symlink_directory_and_fifo_stay_unfollowed() -> Result<(), String> {
    let keep = tempfile::tempdir().expect("dir");
    let real = keep.path().join("real");
    std::fs::create_dir(&real).expect("real");
    std::fs::write(real.join("marker"), b"stay").expect("marker");
    let link = keep.path().join("link");
    std::os::unix::fs::symlink(&real, &link).expect("symlink");
    let gateway = Arc::new(base().with_audio(gateway::AudioConfig::new(link)));
    let app = gateway.router(false);
    let status = audio_status(&call(&app, "GET", "/audio", Vec::new(), &[]).await?)?;
    assert!(!status.runtime_ready);
    assert_eq!(status.backend, AudioBackend::None);
    assert!(!real.join("mic.pcm").exists());
    assert_eq!(std::fs::read(real.join("marker")).expect("marker"), b"stay");
    assert_unready(&call(&app, "GET", "/audio/health", Vec::new(), &[]).await?)?;
    let running = serve_running(Arc::clone(&gateway)).await?;
    let mut socket = connect(&running.addr, "/audio/mic").await?;
    let greeting = hello(None, None)?;
    send_text(&mut socket, &greeting).await?;
    expect_fault(
        &mut socket,
        AudioMicErrorCode::NoRuntime,
        "audio runtime is not ready",
    )
    .await?;
    drop(socket);
    finish(running).await?;

    let dir = keep.path().join("audio");
    std::fs::create_dir(&dir).expect("audio");
    let outside = keep.path().join("outside");
    std::fs::write(&outside, b"keep").expect("outside");
    std::os::unix::fs::symlink(&outside, dir.join("mic.pcm")).expect("fifo link");
    let gateway = Arc::new(base().with_audio(gateway::AudioConfig::new(dir.clone())));
    let app = gateway.router(false);
    let status = audio_status(&call(&app, "GET", "/audio", Vec::new(), &[]).await?)?;
    assert!(!status.runtime_ready);
    assert!(is_symlink(&dir.join("mic.pcm"))?);
    assert_eq!(std::fs::read(&outside).expect("outside"), b"keep");
    assert_unready(&call(&app, "GET", "/api/audio/health", Vec::new(), &[]).await?)?;
    close(gateway).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disabled_mic_upgrade_returns_no_bytes() -> Result<(), String> {
    let running = serve_running(Arc::new(base())).await?;
    for path in ["/audio/mic", "/api/audio/mic"] {
        assert_eq!(destroyed(&running.addr, path).await?, Vec::<u8>::new());
    }
    finish(running).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mic_round_trip_writes_pcm_and_rejects_bad_frames() -> Result<(), String> {
    let live = serve_audio().await?;
    round_trip(&live).await?;
    reject_before_arm(&live).await?;
    reject_odd_pcm(&live).await?;
    second_publisher_is_busy(&live).await?;
    shutdown_sends_close(&live).await?;
    finish(live.running).await?;
    Ok(())
}

fn assert_idle(status: &AudioStatus, dir: &Path) -> Result<(), String> {
    assert!(status.enabled);
    assert!(status.runtime_ready);
    assert_eq!(status.backend, AudioBackend::FifoShim);
    assert_eq!(status.device, "RivetHub Mic");
    assert_eq!(status.sample_rate, 16_000);
    assert_eq!(status.channels, 1);
    assert_eq!(status.format, AudioFormat::S16Le);
    assert_eq!(status.fifo_path, fifo_text(dir)?);
    assert!(!status.armed);
    assert!(status.publisher_id.is_none());
    assert!(status.armed_at.is_none());
    assert_eq!(status.bytes_written, 0);
    Ok(())
}

fn assert_unready(response: &testkit::Response) -> Result<(), String> {
    assert_eq!(response.status, 503);
    let body: AudioHealth =
        serde_json::from_slice(&response.body).map_err(|error| error.to_string())?;
    assert!(!body.ok);
    assert!(!body.runtime_ready);
    assert_eq!(
        body.message.as_deref(),
        Some("audio path escapes the audio directory")
    );
    Ok(())
}

fn assert_error(response: &testkit::Response, status: u16, message: &str) -> Result<(), String> {
    assert_eq!(response.status, status, "{}", body_text(&response.body));
    assert_eq!(gateway_error(response)?.error, message);
    Ok(())
}

fn base() -> gateway::Gateway {
    let store: Arc<dyn MemoryStore> = Arc::new(testkit::FakeMemoryStore::default());
    gateway::Gateway::new(store, common::settings())
}

async fn serve_audio() -> Result<AudioLive, String> {
    let keep = tempfile::tempdir().map_err(|error| error.to_string())?;
    let dir = keep.path().join("audio");
    let gateway = Arc::new(base().with_audio(gateway::AudioConfig::new(dir.clone())));
    Ok(AudioLive {
        running: serve_running(gateway).await?,
        dir,
        _keep: keep,
    })
}

async fn serve_running(gateway: Arc<gateway::Gateway>) -> Result<Running, String> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .map_err(|error| error.to_string())?;
    let addr = listener.local_addr().map_err(|error| error.to_string())?;
    let token = CancellationToken::new();
    let server = tokio::spawn(gateway::server::serve(
        listener,
        Arc::clone(&gateway),
        None,
        token.clone(),
    ));
    Ok(Running {
        gateway,
        addr,
        token,
        server,
    })
}

async fn finish(running: Running) -> Result<(), String> {
    running.token.cancel();
    tokio::time::timeout(Duration::from_secs(5), running.server)
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
    Ok(())
}

async fn close(gateway: Arc<gateway::Gateway>) -> Result<(), String> {
    tokio::time::timeout(Duration::from_secs(5), gateway.close())
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn audio_ids() -> Result<Vec<String>, String> {
    let mut ids = Vec::new();
    let http = testkit::http::cases(&testkit::recording("gateway/http-plain.json"))
        .map_err(|error| error.to_string())?;
    for case in http {
        if names_audio(&case.id) {
            ids.push(case.id);
        }
    }
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../wire/tests/golden/gateway/ws.json");
    let bytes = std::fs::read(&path).map_err(|error| error.to_string())?;
    let document: WsDocument = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    for (run, entry) in document.runs {
        for step in entry.steps {
            let id = format!("{run}/{}", step.id);
            if names_audio(&id) {
                ids.push(id);
            }
        }
    }
    Ok(ids)
}

fn names_audio(id: &str) -> bool {
    id.split(['/', '-'])
        .any(|part| part == "audio" || part == "voice")
}

fn reject_duplicates(ids: &[&str], label: &str) {
    let mut seen = Vec::new();
    for id in ids {
        assert!(!seen.contains(id), "duplicate {label} {id}");
        seen.push(*id);
    }
}

fn fifo_text(dir: &Path) -> Result<String, String> {
    let path = config::normalize(&dir.join("mic.pcm"));
    path.to_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| "fifo path is not utf-8".to_owned())
}

fn perm(path: &Path) -> Result<u32, String> {
    use std::os::unix::fs::PermissionsExt;
    let meta = std::fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    Ok(meta.permissions().mode() & 0o777)
}

fn is_fifo(path: &Path) -> Result<bool, String> {
    use std::os::unix::fs::FileTypeExt;
    let meta = std::fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    Ok(meta.file_type().is_fifo())
}

fn is_symlink(path: &Path) -> Result<bool, String> {
    let meta = std::fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    Ok(meta.file_type().is_symlink())
}

fn gateway_error(response: &testkit::Response) -> Result<GatewayError, String> {
    serde_json::from_slice(&response.body).map_err(|error| error.to_string())
}

fn audio_status(response: &testkit::Response) -> Result<AudioStatus, String> {
    assert_eq!(response.status, 200, "{}", body_text(&response.body));
    serde_json::from_slice(&response.body).map_err(|error| error.to_string())
}

fn body_text(bytes: &[u8]) -> String {
    if let Ok(text) = std::str::from_utf8(bytes) {
        text.to_owned()
    } else {
        "body is not utf-8".to_owned()
    }
}

async fn call(
    app: &axum::Router,
    method: &str,
    target: &str,
    body: Vec<u8>,
    extra: &[(&str, &str)],
) -> Result<testkit::Response, String> {
    match tokio::time::timeout(
        Duration::from_secs(5),
        common::answer(app.clone(), recorded(method, target, body, extra)),
    )
    .await
    {
        Ok(response) => response,
        Err(error) => Err(error.to_string()),
    }
}

fn recorded(method: &str, target: &str, body: Vec<u8>, extra: &[(&str, &str)]) -> testkit::Request {
    let mut headers = vec![("host".to_owned(), "127.0.0.1:5174".to_owned())];
    for (name, value) in extra {
        headers.push(((*name).to_owned(), (*value).to_owned()));
    }
    testkit::Request {
        method: method.to_owned(),
        target: target.to_owned(),
        headers,
        body,
        recipe: None,
        via: Some("loopback".to_owned()),
    }
}

async fn post_json(
    app: &axum::Router,
    target: &str,
    body: &impl Serialize,
) -> Result<testkit::Response, String> {
    call(
        app,
        "POST",
        target,
        serde_json::to_vec(body).map_err(|error| error.to_string())?,
        &[("content-type", "application/json")],
    )
    .await
}
