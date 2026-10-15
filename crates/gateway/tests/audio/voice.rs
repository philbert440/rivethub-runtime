use std::sync::Arc;
use std::time::Duration;

use wire::gateway::devices::{VoiceSpeakRequest, VoiceTranscribeResponse};

use super::{
    Hit, Huge, Spoken, Upstream, assert_error, base, body_text, call, close, common, gateway_error,
    post_json, recorded,
};

const WAIT: Duration = Duration::from_secs(2);

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
