#[expect(
    dead_code,
    reason = "the harness module is included whole by every test binary and this one uses a subset"
)]
mod common;

use std::net::SocketAddr;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::extract::Request;
use axum::extract::State;
use axum::http::StatusCode;
use axum::http::header;
use axum::response::IntoResponse;
use axum::response::Response;
use axum::routing::any;
use futures::SinkExt;
use futures::StreamExt;
use kernel::memory::MemoryStore;
use serde::Deserialize;
use serde::Serialize;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;
use wire::events::ProtocolVersion;
use wire::gateway::GatewayError;
use wire::gateway::devices::AudioBackend;
use wire::gateway::devices::AudioFormat;
use wire::gateway::devices::AudioHealth;
use wire::gateway::devices::AudioMicClientFrame;
use wire::gateway::devices::AudioMicErrorCode;
use wire::gateway::devices::AudioMicServerFrame;
use wire::gateway::devices::AudioStatus;
use wire::gateway::devices::VoiceSpeakRequest;
use wire::gateway::devices::VoiceTranscribeResponse;

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

const WAIT: Duration = Duration::from_secs(2);

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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn voice_rejects_bad_requests() -> Result<(), String> {
    let gateway = Arc::new(base());
    let app = gateway.router(false);
    assert_error(
        &call(&app, "POST", "/api/voice/transcribe", b"abc".to_vec(), &[]).await?,
        501,
        "voice transcription is not configured on this node",
    )?;
    assert_error(
        &call(
            &app,
            "POST",
            "/api/voice/speak",
            b"{}".as_slice().to_vec(),
            &[],
        )
        .await?,
        501,
        "voice synthesis is not configured on this node",
    )?;
    assert_error(
        &call(&app, "GET", "/api/voice/transcribe", Vec::new(), &[]).await?,
        404,
        "not found",
    )?;
    let blank = Arc::new(
        base()
            .with_voice(gateway::VoiceSettings::new(
                Some("  ".to_owned()),
                Some(String::new()),
                None,
            ))
            .expect("blank urls are unconfigured"),
    );
    assert_error(
        &call(
            &blank.router(false),
            "POST",
            "/api/voice/speak",
            b"{}".to_vec(),
            &[("content-type", "application/json")],
        )
        .await?,
        501,
        "voice synthesis is not configured on this node",
    )?;
    let Err(error) = base().with_voice(gateway::VoiceSettings::new(
        Some("ftp://files.example/a".to_owned()),
        None,
        None,
    )) else {
        return Err("ftp transcription url was accepted".to_owned());
    };
    assert!(error.to_string().contains("http or https"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn voice_rejects_invalid_bodies() -> Result<(), String> {
    let upstream = Upstream::start().await?;
    let voiced = voiced(Some(upstream.url("/stt")), Some(upstream.url("/tts")), None)?;
    let app = voiced.router(false);
    assert_error(
        &call(
            &app,
            "POST",
            "/api/voice/transcribe",
            Vec::new(),
            &[("content-type", "audio/wav")],
        )
        .await?,
        400,
        "empty audio body",
    )?;
    assert_error(
        &call(&app, "POST", "/api/voice/speak", b"not-json".to_vec(), &[]).await?,
        400,
        "speak body must be JSON",
    )?;
    assert_error(
        &call(
            &app,
            "POST",
            "/api/voice/speak",
            b"{}".to_vec(),
            &[("content-type", "application/json")],
        )
        .await?,
        400,
        "input required",
    )?;
    let long = VoiceSpeakRequest {
        input: "a".repeat(4097),
        instructions: None,
        voice: None,
    };
    assert_error(
        &post_json(&app, "/api/voice/speak", &long).await?,
        400,
        "input exceeds 4096 UTF-8 bytes",
    )?;
    assert_error(
        &call(
            &app,
            "POST",
            "/api/voice/speak",
            vec![b' '; 64 * 1024 + 1],
            &[],
        )
        .await?,
        400,
        "speak body too large",
    )?;
    assert_error(
        &call(
            &app,
            "POST",
            "/api/voice/transcribe",
            vec![0; 15 * 1024 * 1024 + 1],
            &[("content-type", "audio/wav")],
        )
        .await?,
        413,
        "audio body too large",
    )?;
    assert_error(
        &call(&app, "PUT", "/api/voice/speak", Vec::new(), &[]).await?,
        404,
        "not found",
    )?;
    upstream.stop().await?;
    close(voiced).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn voice_proxies_upstream_audio() -> Result<(), String> {
    let upstream = Upstream::start().await?;
    let gateway = voiced(
        Some(upstream.url("/stt")),
        Some(upstream.url("/tts")),
        Some("steady".to_owned()),
    )?;
    let app = gateway.router(false);
    let webm = call(
        &app,
        "POST",
        "/api/voice/transcribe",
        b"pcm".to_vec(),
        &[("content-type", "audio/webm; codecs=opus")],
    )
    .await?;
    assert_transcript(&webm, "heard")?;
    let html = call(
        &app,
        "POST",
        "/api/voice/transcribe",
        b"pcm".to_vec(),
        &[("content-type", "text/html")],
    )
    .await?;
    assert_transcript(&html, "heard")?;
    let hits = upstream.seen.take().await;
    assert!(has_bytes(
        &find_hit(&hits, "/stt")?.body,
        b"filename=\"speech.audio\""
    ));
    assert!(has_bytes(
        &find_hit(&hits, "/stt")?.body,
        b"Content-Type: audio/webm"
    ));
    assert!(has_bytes(
        &find_hit_nth(&hits, "/stt", 1)?.body,
        b"Content-Type: audio/wav"
    ));
    let plain = VoiceSpeakRequest {
        input: "hi".to_owned(),
        instructions: None,
        voice: None,
    };
    assert_spoken(&upstream, &app, &plain, Some("steady"), None).await?;
    let override_voice = VoiceSpeakRequest {
        input: "hi".to_owned(),
        instructions: Some("louder".to_owned()),
        voice: Some("cedar".to_owned()),
    };
    assert_spoken(
        &upstream,
        &app,
        &override_voice,
        Some("louder"),
        Some("cedar"),
    )
    .await?;
    upstream.stop().await?;
    close(gateway).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn voice_follows_redirects_and_media_types() -> Result<(), String> {
    let upstream = Upstream::start().await?;
    let plain = VoiceSpeakRequest {
        input: "hi".to_owned(),
        instructions: None,
        voice: None,
    };
    let mpeg = voiced(None, Some(upstream.url("/mpeg")), None)?;
    let audio = call(
        &mpeg.router(false),
        "POST",
        "/api/voice/speak",
        serde_json::to_vec(&plain).expect("speak"),
        &[("content-type", "application/json")],
    )
    .await?;
    assert_eq!(audio.status, 200);
    assert_eq!(header(&audio, "content-type")?, "audio/mpeg");
    assert_eq!(audio.body, b"ID3");
    let redirect = voiced(Some(upstream.url("/hop")), None, None)?;
    assert_transcript(
        &call(
            &redirect.router(false),
            "POST",
            "/api/voice/transcribe",
            b"pcm".to_vec(),
            &[("content-type", "audio/wav")],
        )
        .await?,
        "followed",
    )?;
    let ftp = voiced(Some(upstream.url("/ftp")), None, None)?;
    let refused = call(
        &ftp.router(false),
        "POST",
        "/api/voice/transcribe",
        b"pcm".to_vec(),
        &[("content-type", "audio/wav")],
    )
    .await?;
    assert_eq!(refused.status, 502);
    assert!(
        gateway_error(&refused)?
            .error
            .contains("redirect target is not http(s)")
    );
    upstream.stop().await?;
    close(mpeg).await?;
    close(redirect).await?;
    close(ftp).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn voice_reports_upstream_failures() -> Result<(), String> {
    let upstream = Upstream::start().await?;
    let gateway = voiced(
        Some(upstream.url("/fail")),
        Some(upstream.url("/tts-fail")),
        None,
    )?;
    let app = gateway.router(false);
    let failed = call(
        &app,
        "POST",
        "/api/voice/transcribe",
        b"pcm".to_vec(),
        &[("content-type", "audio/wav")],
    )
    .await?;
    assert_eq!(failed.status, 502);
    let error = gateway_error(&failed)?;
    assert!(error.error.starts_with("voice upstream failed:"));
    assert!(error.error.contains("upstream said no"));
    let synth = post_json(
        &app,
        "/api/voice/speak",
        &VoiceSpeakRequest {
            input: "hi".to_owned(),
            instructions: None,
            voice: None,
        },
    )
    .await?;
    assert_eq!(synth.status, 502);
    assert!(gateway_error(&synth)?.error.contains("synth failed"));
    let stalled_voice = Arc::new(
        base()
            .with_voice(
                gateway::VoiceSettings::new(Some(upstream.url("/hang")), None, None)
                    .with_timeouts(Duration::from_millis(200), Duration::from_millis(200)),
            )
            .expect("short voice timeout"),
    );
    let stalled = tokio::time::timeout(
        WAIT,
        common::answer(
            stalled_voice.router(false),
            recorded(
                "POST",
                "/api/voice/transcribe",
                b"pcm".to_vec(),
                &[("content-type", "audio/wav")],
            ),
        ),
    )
    .await
    .expect("hang backstop")
    .expect("hang response");
    assert_eq!(stalled.status, 502);
    assert!(
        gateway_error(&stalled)?
            .error
            .starts_with("voice upstream failed:")
    );
    upstream.stop().await?;
    close(gateway).await?;
    close(stalled_voice).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn voice_rejects_a_huge_upstream_body() -> Result<(), String> {
    let huge = Huge::start().await?;
    let gateway = voiced(None, Some(huge.url.clone()), None)?;
    let over = tokio::time::timeout(
        WAIT,
        common::answer(
            gateway.router(false),
            recorded(
                "POST",
                "/api/voice/speak",
                serde_json::to_vec(&VoiceSpeakRequest {
                    input: "hi".to_owned(),
                    instructions: None,
                    voice: None,
                })
                .expect("speak"),
                &[("content-type", "application/json")],
            ),
        ),
    )
    .await
    .expect("huge backstop")
    .expect("huge response");
    assert_eq!(over.status, 502);
    assert!(
        gateway_error(&over)?
            .error
            .contains("upstream audio exceeds 30 MiB")
    );
    huge.stop().await?;
    close(gateway).await?;
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

fn assert_transcript(response: &testkit::Response, text: &str) -> Result<(), String> {
    assert_eq!(response.status, 200, "{}", body_text(&response.body));
    let body = body_text(&response.body);
    assert!(!body.contains("ignored"));
    let parsed: VoiceTranscribeResponse =
        serde_json::from_str(&body).map_err(|error| error.to_string())?;
    assert_eq!(parsed.text, text);
    Ok(())
}

async fn assert_spoken(
    upstream: &Upstream,
    app: &axum::Router,
    body: &VoiceSpeakRequest,
    instructions: Option<&str>,
    voice: Option<&str>,
) -> Result<(), String> {
    upstream.seen.take().await;
    let response = post_json(app, "/api/voice/speak", body).await?;
    assert_eq!(response.status, 200, "{}", body_text(&response.body));
    assert_eq!(header(&response, "content-type")?, "audio/wav");
    assert_eq!(response.body, b"RIFF");
    let hits = upstream.seen.take().await;
    let spoken: Spoken = serde_json::from_slice(&find_hit(&hits, "/tts")?.body)
        .map_err(|error| error.to_string())?;
    assert_eq!(spoken.input, body.input);
    assert_eq!(spoken.response_format, "wav");
    assert_eq!(spoken.instructions.as_deref(), instructions);
    assert_eq!(spoken.voice.as_deref(), voice);
    Ok(())
}

async fn round_trip(live: &AudioLive) -> Result<(), String> {
    let mut socket = connect(&live.running.addr, "/audio/mic").await?;
    ready(&mut socket).await?;
    let reader = open_reader(&live.dir.join("mic.pcm"))?;
    send_text(&mut socket, r#"{"type":"start"}"#).await?;
    let armed = audio_status(
        &call(
            &live.running.gateway.router(false),
            "GET",
            "/audio/status",
            Vec::new(),
            &[],
        )
        .await?,
    )?;
    assert!(armed.armed);
    assert!(armed.publisher_id.is_some());
    assert_eq!(armed.bytes_written, 0);
    send_binary(&mut socket, &[1, 2, 3, 4]).await?;
    send_text(&mut socket, r#"{"type":"stop"}"#).await?;
    assert_eq!(
        recv_server(&mut socket, "stopped").await?,
        AudioMicServerFrame::Stopped
    );
    assert_eq!(read_pcm(reader)?, vec![1, 2, 3, 4]);
    let done = audio_status(
        &call(
            &live.running.gateway.router(false),
            "GET",
            "/audio",
            Vec::new(),
            &[],
        )
        .await?,
    )?;
    assert!(!done.armed);
    assert!(done.publisher_id.is_none());
    assert_eq!(done.bytes_written, 4);
    Ok(())
}

async fn reject_before_arm(live: &AudioLive) -> Result<(), String> {
    let mut socket = connect(&live.running.addr, "/audio/mic").await?;
    send_text(&mut socket, r#"{"type":"start"}"#).await?;
    expect_fault(&mut socket, AudioMicErrorCode::BadHello, "expected hello").await?;
    let mut socket = connect(&live.running.addr, "/audio/mic").await?;
    let slow = hello(Some(8_000), Some(1))?;
    send_text(&mut socket, &slow).await?;
    expect_fault(
        &mut socket,
        AudioMicErrorCode::BadHello,
        "sample rate must be 16000",
    )
    .await?;
    let mut socket = connect(&live.running.addr, "/audio/mic").await?;
    let wide = hello(Some(16_000), Some(2))?;
    send_text(&mut socket, &wide).await?;
    expect_fault(
        &mut socket,
        AudioMicErrorCode::BadHello,
        "channels must be 1",
    )
    .await?;
    let mut socket = connect(&live.running.addr, "/audio/mic").await?;
    ready(&mut socket).await?;
    send_binary(&mut socket, &[9, 8]).await?;
    expect_fault(
        &mut socket,
        AudioMicErrorCode::BadHello,
        "binary audio requires start",
    )
    .await?;
    Ok(())
}

async fn reject_odd_pcm(live: &AudioLive) -> Result<(), String> {
    let mut socket = connect(&live.running.addr, "/audio/mic").await?;
    ready(&mut socket).await?;
    let _reader = open_reader(&live.dir.join("mic.pcm"))?;
    send_text(&mut socket, r#"{"type":"start"}"#).await?;
    send_binary(&mut socket, &[1]).await?;
    expect_fault(
        &mut socket,
        AudioMicErrorCode::BadHello,
        "pcm length must be even",
    )
    .await?;
    Ok(())
}

async fn second_publisher_is_busy(live: &AudioLive) -> Result<(), String> {
    let mut first = connect(&live.running.addr, "/audio/mic").await?;
    let mut second = connect(&live.running.addr, "/api/audio/mic").await?;
    ready(&mut first).await?;
    ready(&mut second).await?;
    let _reader = open_reader(&live.dir.join("mic.pcm"))?;
    let before = publisher_id(live).await?;
    send_text(&mut first, r#"{"type":"start"}"#).await?;
    wait_for_publisher_change(live, before.as_ref()).await?;
    send_text(&mut second, r#"{"type":"start"}"#).await?;
    expect_fault(
        &mut second,
        AudioMicErrorCode::Busy,
        "another publisher is armed",
    )
    .await?;
    send_text(&mut first, r#"{"type":"start"}"#).await?;
    expect_fault(
        &mut first,
        AudioMicErrorCode::Busy,
        "another publisher is armed",
    )
    .await?;
    Ok(())
}

async fn shutdown_sends_close(live: &AudioLive) -> Result<(), String> {
    let mut socket = connect(&live.running.addr, "/audio/mic").await?;
    ready(&mut socket).await?;
    live.running.token.cancel();
    match next_message(&mut socket, "shutdown close").await? {
        Message::Close(_) => Ok(()),
        Message::Text(text) => Err(format!("text during shutdown: {text}")),
        Message::Binary(_) => Err("binary during shutdown".to_owned()),
        Message::Ping(_) => Err("ping during shutdown".to_owned()),
        Message::Pong(_) => Err("pong during shutdown".to_owned()),
        Message::Frame(_) => Err("frame during shutdown".to_owned()),
    }
}

async fn ready(socket: &mut MicSocket) -> Result<(), String> {
    let frame = hello(Some(16_000), Some(1))?;
    send_text(socket, &frame).await?;
    match recv_server(socket, "ready").await? {
        AudioMicServerFrame::Ready {
            device,
            sample_rate,
            format,
            backend,
        } => {
            assert_eq!(device, "RivetHub Mic");
            assert_eq!(sample_rate, 16_000);
            assert_eq!(format, AudioFormat::S16Le);
            assert_eq!(backend, AudioBackend::FifoShim);
            Ok(())
        }
        AudioMicServerFrame::Error { code, message } => {
            Err(format!("hello failed: {code:?} {message}"))
        }
        AudioMicServerFrame::Stopped => Err("stopped before ready".to_owned()),
    }
}

async fn publisher_id(live: &AudioLive) -> Result<Option<String>, String> {
    let status = audio_status(
        &call(
            &live.running.gateway.router(false),
            "GET",
            "/audio/status",
            Vec::new(),
            &[],
        )
        .await?,
    )?;
    Ok(status.publisher_id)
}

async fn wait_for_publisher_change(
    live: &AudioLive,
    before: Option<&String>,
) -> Result<(), String> {
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        let id = publisher_id(live).await?;
        if id.is_some() && id.as_ref() != before {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("publisher did not arm".to_owned());
        }
        tokio::task::yield_now().await;
    }
}

async fn expect_fault(
    socket: &mut MicSocket,
    code: AudioMicErrorCode,
    message: &str,
) -> Result<(), String> {
    match recv_server(socket, &format!("fault:{message}")).await? {
        AudioMicServerFrame::Error {
            code: got,
            message: text,
        } => {
            assert_eq!(got, code);
            assert_eq!(text, message);
        }
        AudioMicServerFrame::Ready { .. } => return Err("ready".to_owned()),
        AudioMicServerFrame::Stopped => return Err("stopped".to_owned()),
    }
    match next_message(socket, "fault close").await? {
        Message::Close(_) => Ok(()),
        Message::Text(text) => Err(format!("text after error: {text}")),
        Message::Binary(_) => Err("binary after error".to_owned()),
        Message::Ping(_) => Err("ping after error".to_owned()),
        Message::Pong(_) => Err("pong after error".to_owned()),
        Message::Frame(_) => Err("frame after error".to_owned()),
    }
}

fn hello(sample_rate: Option<u32>, channels: Option<u32>) -> Result<String, String> {
    serde_json::to_string(&AudioMicClientFrame::Hello {
        v: ProtocolVersion::V1,
        sample_rate,
        channels,
        format: Some(AudioFormat::S16Le),
    })
    .map_err(|error| error.to_string())
}

async fn send_text(socket: &mut MicSocket, text: &str) -> Result<(), String> {
    socket
        .send(Message::text(text))
        .await
        .map_err(|error| error.to_string())
}

async fn send_binary(socket: &mut MicSocket, bytes: &[u8]) -> Result<(), String> {
    socket
        .send(Message::binary(bytes.to_vec()))
        .await
        .map_err(|error| error.to_string())
}

async fn recv_server(socket: &mut MicSocket, step: &str) -> Result<AudioMicServerFrame, String> {
    match next_message(socket, step).await? {
        Message::Text(text) => serde_json::from_str(&text).map_err(|error| error.to_string()),
        Message::Binary(_) => Err("binary from the mic bridge".to_owned()),
        Message::Ping(_) => Err("ping from the mic bridge".to_owned()),
        Message::Pong(_) => Err("pong from the mic bridge".to_owned()),
        Message::Close(_) => Err("closed before a frame".to_owned()),
        Message::Frame(_) => Err("raw frame from the mic bridge".to_owned()),
    }
}

async fn next_message(socket: &mut MicSocket, step: &str) -> Result<Message, String> {
    match tokio::time::timeout(WAIT, socket.next()).await {
        Ok(Some(Ok(message))) => Ok(message),
        Ok(Some(Err(error))) => Err(format!("{step}: {error}")),
        Ok(None) => Err(format!("{step}: socket closed")),
        Err(error) => Err(format!("{step}: {error}")),
    }
}

async fn connect(addr: &SocketAddr, path: &str) -> Result<MicSocket, String> {
    let (socket, _) = tokio::time::timeout(
        WAIT,
        tokio_tungstenite::connect_async(format!("ws://{addr}{path}")),
    )
    .await
    .map_err(|error| format!("mic connect: {error}"))?
    .map_err(|error| format!("mic connect: {error}"))?;
    Ok(socket)
}

async fn destroyed(addr: &SocketAddr, path: &str) -> Result<Vec<u8>, String> {
    let mut stream = TcpStream::connect(*addr)
        .await
        .map_err(|error| error.to_string())?;
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {addr}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|error| error.to_string())?;
    let mut buf = Vec::new();
    tokio::time::timeout(WAIT, stream.read_to_end(&mut buf))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
    Ok(buf)
}

fn base() -> gateway::Gateway {
    let store: Arc<dyn MemoryStore> = Arc::new(testkit::FakeMemoryStore::default());
    gateway::Gateway::new(store, common::settings())
}

fn voiced(
    stt: Option<String>,
    tts: Option<String>,
    instructions: Option<String>,
) -> Result<Arc<gateway::Gateway>, String> {
    base()
        .with_voice(gateway::VoiceSettings::new(stt, tts, instructions).with_timeouts(WAIT, WAIT))
        .map(Arc::new)
        .map_err(|error| error.to_string())
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

fn open_reader(path: &Path) -> Result<std::fs::File, String> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
        .map_err(|error| error.to_string())
}

fn read_pcm(mut file: std::fs::File) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut out = Vec::new();
    let mut buf = [0u8; 64];
    loop {
        match file.read(&mut buf) {
            Ok(0) => break,
            Ok(count) => {
                let Some(part) = buf.get(..count) else {
                    return Err("fifo read was longer than the buffer".to_owned());
                };
                out.extend_from_slice(part);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(out)
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

fn header<'a>(response: &'a testkit::Response, name: &str) -> Result<&'a str, String> {
    response
        .headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
        .ok_or_else(|| format!("missing header {name}"))
}

fn has_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn find_hit<'a>(hits: &'a [Hit], path: &str) -> Result<&'a Hit, String> {
    find_hit_nth(hits, path, 0)
}

fn find_hit_nth<'a>(hits: &'a [Hit], path: &str, nth: usize) -> Result<&'a Hit, String> {
    hits.iter()
        .filter(|hit| hit.path == path)
        .nth(nth)
        .ok_or_else(|| format!("missing hit {path} #{nth}"))
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

impl Seen {
    fn new(stop: CancellationToken) -> Self {
        Self {
            hits: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            stop,
        }
    }

    async fn push(&self, hit: Hit) {
        self.hits.lock().await.push(hit);
    }

    async fn take(&self) -> Vec<Hit> {
        std::mem::take(&mut *self.hits.lock().await)
    }
}

impl Upstream {
    async fn start() -> Result<Self, String> {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .map_err(|error| error.to_string())?;
        let addr = listener.local_addr().map_err(|error| error.to_string())?;
        let token = CancellationToken::new();
        let seen = Seen::new(token.clone());
        let app = Router::new()
            .route("/stt", any(take))
            .route("/land", any(take))
            .route("/fail", any(take))
            .route("/hang", any(take))
            .route("/hop", any(take))
            .route("/ftp", any(take))
            .route("/tts", any(take))
            .route("/mpeg", any(take))
            .route("/tts-fail", any(take))
            .with_state(seen.clone());
        let stop = token.clone();
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move { stop.cancelled().await })
                .await
        });
        Ok(Self {
            addr,
            seen,
            token,
            server,
        })
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    async fn stop(self) -> Result<(), String> {
        self.token.cancel();
        tokio::time::timeout(Duration::from_secs(5), self.server)
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())?;
        Ok(())
    }
}

async fn take(State(seen): State<Seen>, request: Request) -> Response {
    let path = request.uri().path().to_owned();
    if path == "/hang" {
        seen.stop.cancelled().await;
        return StatusCode::NO_CONTENT.into_response();
    }
    let Ok(body) = axum::body::to_bytes(request.into_body(), 2 * 1024 * 1024).await else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let body = body.to_vec();
    seen.push(Hit {
        path: path.clone(),
        body,
    })
    .await;
    answer_path(&path)
}

fn answer_path(path: &str) -> Response {
    match path {
        "/stt" => Json(Heard {
            text: "heard".to_owned(),
            ignored: true,
        })
        .into_response(),
        "/land" => Json(Heard {
            text: "followed".to_owned(),
            ignored: true,
        })
        .into_response(),
        "/fail" => (StatusCode::INTERNAL_SERVER_ERROR, "upstream said no").into_response(),
        "/tts-fail" => (StatusCode::INTERNAL_SERVER_ERROR, "synth failed").into_response(),
        "/hop" => redirect("/land"),
        "/ftp" => redirect("ftp://example.test/a"),
        "/tts" => bytes_response("audio/wav", b"RIFF"),
        "/mpeg" => bytes_response("audio/mpeg", b"ID3"),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

fn redirect(location: &str) -> Response {
    if let Ok(response) = Response::builder()
        .status(StatusCode::FOUND)
        .header(header::LOCATION, location)
        .body(Body::empty())
    {
        response
    } else {
        StatusCode::INTERNAL_SERVER_ERROR.into_response()
    }
}

fn bytes_response(content_type: &'static str, body: &'static [u8]) -> Response {
    if let Ok(response) = Response::builder()
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(body))
    {
        response
    } else {
        StatusCode::INTERNAL_SERVER_ERROR.into_response()
    }
}

struct Huge {
    url: String,
    token: CancellationToken,
    server: tokio::task::JoinHandle<()>,
}

impl Huge {
    async fn start() -> Result<Self, String> {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .map_err(|error| error.to_string())?;
        let addr = listener.local_addr().map_err(|error| error.to_string())?;
        let token = CancellationToken::new();
        let stop = token.clone();
        let server = tokio::spawn(async move {
            loop {
                let accepted = tokio::select! {
                    () = stop.cancelled() => break,
                    result = listener.accept() => result,
                };
                let Ok((stream, _)) = accepted else {
                    break;
                };
                let child = stop.clone();
                tokio::spawn(async move { hold_huge(stream, child).await });
            }
        });
        Ok(Self {
            url: format!("http://{addr}/tts"),
            token,
            server,
        })
    }

    async fn stop(self) -> Result<(), String> {
        self.token.cancel();
        tokio::time::timeout(Duration::from_secs(5), self.server)
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())?;
        Ok(())
    }
}

async fn hold_huge(mut stream: TcpStream, stop: CancellationToken) {
    let mut buf = [0u8; 512];
    let mut seen = Vec::new();
    loop {
        if seen.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
        if seen.len() > 8192 {
            return;
        }
        let count = match stream.read(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(count) => count,
        };
        let Some(part) = buf.get(..count) else {
            return;
        };
        seen.extend_from_slice(part);
    }
    let head = b"HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nContent-Length: 31457281\r\n\r\n";
    if stream.write_all(head).await.is_err() {
        return;
    }
    stop.cancelled().await;
}
