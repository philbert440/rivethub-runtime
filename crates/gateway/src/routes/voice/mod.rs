use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::extract::DefaultBodyLimit;
use axum::extract::Request;
use axum::extract::State;
use axum::http::HeaderValue;
use axum::http::StatusCode;
use axum::http::header;
use axum::response::IntoResponse;
use axum::response::Response;
use axum::routing::post;
use futures::StreamExt;
use serde::Deserialize;
use serde::Serialize;
use thiserror::Error;

mod media;

use media::forwarded_media;
use wire::gateway::GatewayError;
use wire::gateway::devices::VoiceSpeakRequest;
use wire::gateway::devices::VoiceTranscribeResponse;

use crate::Gateway;

const AUDIO_LIMIT: usize = 15 * 1024 * 1024;
const SPEAK_LIMIT: usize = 64 * 1024;
const INPUT_LIMIT: usize = 4096;
const STT_BODY: usize = 1024 * 1024;
const SPEAK_STREAM: usize = 30 * 1024 * 1024;
const SPEAK_STREAM_U64: u64 = 30 * 1024 * 1024;
const SNIPPET_READ: usize = 8 * 1024;
const SNIPPET_CHARS: usize = 240;
const STT_TIMEOUT: Duration = Duration::from_secs(60);
const TTS_TIMEOUT: Duration = Duration::from_secs(120);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REDIRECT_LIMIT: usize = 5;
const UPLOAD_NAME: &str = "speech.audio";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceSettings {
    stt_url: Option<String>,
    tts_url: Option<String>,
    instructions: Option<String>,
    stt_timeout: Duration,
    tts_timeout: Duration,
}

impl VoiceSettings {
    #[must_use]
    pub fn new(
        stt_url: Option<String>,
        tts_url: Option<String>,
        instructions: Option<String>,
    ) -> Self {
        Self {
            stt_url,
            tts_url,
            instructions,
            stt_timeout: STT_TIMEOUT,
            tts_timeout: TTS_TIMEOUT,
        }
    }

    #[must_use]
    pub fn with_timeouts(mut self, stt: Duration, tts: Duration) -> Self {
        self.stt_timeout = stt;
        self.tts_timeout = tts;
        self
    }
}

#[derive(Debug, Error)]
pub enum VoiceConfigError {
    #[error("voice transcription URL is invalid: {0}")]
    Transcription(String),
    #[error("voice synthesis URL is invalid: {0}")]
    Synthesis(String),
    #[error("the voice client could not be built: {0}")]
    Client(String),
}

#[derive(Clone, Copy)]
enum EndpointKind {
    Transcription,
    Synthesis,
}

#[derive(Debug, Clone)]
pub(crate) struct VoiceProxy {
    client: Option<reqwest::Client>,
    stt: Option<url::Url>,
    tts: Option<url::Url>,
    instructions: Option<String>,
    stt_timeout: Duration,
    tts_timeout: Duration,
}

impl VoiceProxy {
    pub(crate) fn unconfigured() -> Self {
        Self {
            client: None,
            stt: None,
            tts: None,
            instructions: None,
            stt_timeout: STT_TIMEOUT,
            tts_timeout: TTS_TIMEOUT,
        }
    }

    pub(crate) fn from_settings(settings: VoiceSettings) -> Result<Self, VoiceConfigError> {
        let stt = parse_endpoint(settings.stt_url, EndpointKind::Transcription)?;
        let tts = parse_endpoint(settings.tts_url, EndpointKind::Synthesis)?;
        let client = if stt.is_some() || tts.is_some() {
            Some(build_client()?)
        } else {
            None
        };
        Ok(Self {
            client,
            stt,
            tts,
            instructions: settings.instructions,
            stt_timeout: settings.stt_timeout,
            tts_timeout: settings.tts_timeout,
        })
    }

    async fn transcribe(&self, request: Request) -> Result<Response, VoiceError> {
        let Some(url) = self.stt.clone() else {
            return Err(VoiceError::TranscriptionUnconfigured);
        };
        let client = self.client()?;
        let media = forwarded_media(request.headers().get(header::CONTENT_TYPE));
        let bytes = read_request(request.into_body(), AUDIO_LIMIT, false).await?;
        if bytes.is_empty() {
            return Err(VoiceError::EmptyAudio);
        }
        let part = reqwest::multipart::Part::bytes(bytes)
            .file_name(UPLOAD_NAME)
            .mime_str(&media)
            .map_err(|error| VoiceError::Media(error.to_string()))?;
        let form = reqwest::multipart::Form::new().part("file", part);
        let response = match client
            .post(url)
            .timeout(self.stt_timeout)
            .multipart(form)
            .send()
            .await
        {
            Ok(response) => response,
            Err(error) => return Err(transport_failure(VoiceRoute::Transcribe, &error)),
        };
        self.finish_transcribe(response).await
    }

    async fn finish_transcribe(&self, response: reqwest::Response) -> Result<Response, VoiceError> {
        if !response.status().is_success() {
            return Err(self
                .upstream_failure(VoiceRoute::Transcribe, response)
                .await);
        }
        let bytes = read_upstream(response, STT_BODY, false, VoiceRoute::Transcribe).await?;
        match serde_json::from_slice::<VoiceTranscribeResponse>(&bytes) {
            Ok(parsed) => Ok(Json(parsed).into_response()),
            Err(error) => {
                tracing::debug!("voice transcription JSON rejected: {error}");
                Err(logged(VoiceRoute::Transcribe, snippet(&bytes)))
            }
        }
    }

    async fn speak(&self, request: Request) -> Result<Response, VoiceError> {
        let Some(url) = self.tts.clone() else {
            return Err(VoiceError::SynthesisUnconfigured);
        };
        let client = self.client()?;
        let bytes = read_request(request.into_body(), SPEAK_LIMIT, false).await?;
        let body = parse_speak(&bytes)?;
        let instructions = body
            .instructions
            .as_deref()
            .or(self.instructions.as_deref());
        let payload = UpstreamSpeak {
            input: &body.input,
            response_format: WavFormat::Wav,
            instructions,
            voice: body.voice.as_deref(),
        };
        let response = match client
            .post(url)
            .timeout(self.tts_timeout)
            .json(&payload)
            .send()
            .await
        {
            Ok(response) => response,
            Err(error) => return Err(transport_failure(VoiceRoute::Speak, &error)),
        };
        self.finish_speak(response).await
    }

    async fn finish_speak(&self, response: reqwest::Response) -> Result<Response, VoiceError> {
        if !response.status().is_success() {
            return Err(self.upstream_failure(VoiceRoute::Speak, response).await);
        }
        if response
            .content_length()
            .is_some_and(|len| len > SPEAK_STREAM_U64)
        {
            return Err(logged(
                VoiceRoute::Speak,
                "upstream audio exceeds 30 MiB".to_owned(),
            ));
        }
        let content_type = speak_type(&response);
        let stream = capped_audio(response);
        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, content_type)
            .body(Body::from_stream(stream))
            .map_err(|error| VoiceError::Response(error.to_string()))
    }

    async fn upstream_failure(&self, route: VoiceRoute, response: reqwest::Response) -> VoiceError {
        let status = response.status();
        let bytes = match read_upstream(response, SNIPPET_READ, true, route).await {
            Ok(bytes) => bytes,
            Err(error) => return error,
        };
        logged(route, format!("status {status}: {}", snippet(&bytes)))
    }

    fn client(&self) -> Result<&reqwest::Client, VoiceError> {
        self.client.as_ref().ok_or(VoiceError::ClientMissing)
    }
}

#[derive(Debug, Serialize)]
struct UpstreamSpeak<'a> {
    input: &'a str,
    response_format: WavFormat,
    #[serde(skip_serializing_if = "Option::is_none")]
    instructions: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    voice: Option<&'a str>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum WavFormat {
    Wav,
}

#[derive(Debug, Deserialize)]
struct SpeakDocument {
    #[serde(default)]
    input: Option<String>,
    #[serde(default)]
    instructions: Option<String>,
    #[serde(default)]
    voice: Option<String>,
}

fn parse_speak(bytes: &[u8]) -> Result<VoiceSpeakRequest, VoiceError> {
    let document = match serde_json::from_slice::<SpeakDocument>(bytes) {
        Ok(document) => document,
        Err(error) if error.is_syntax() || error.is_eof() || error.is_io() => {
            return Err(VoiceError::SpeakJson);
        }
        Err(error) => return Err(VoiceError::SpeakInvalid(error.to_string())),
    };
    let Some(input) = document.input.filter(|input| !input.is_empty()) else {
        return Err(VoiceError::InputRequired);
    };
    if input.len() > INPUT_LIMIT {
        return Err(VoiceError::InputTooLong);
    }
    Ok(VoiceSpeakRequest {
        input,
        instructions: document.instructions,
        voice: document.voice,
    })
}

fn speak_type(response: &reqwest::Response) -> HeaderValue {
    match response.headers().get(header::CONTENT_TYPE) {
        Some(value) if value.to_str().is_ok() && !value.is_empty() => value.clone(),
        Some(_) | None => HeaderValue::from_static("audio/wav"),
    }
}

fn parse_endpoint(
    raw: Option<String>,
    kind: EndpointKind,
) -> Result<Option<url::Url>, VoiceConfigError> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let url = url::Url::parse(trimmed).map_err(|error| endpoint_error(kind, error.to_string()))?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err(endpoint_error(
            kind,
            format!("{trimmed} must use http or https"),
        ));
    }
    if url.host_str().is_none() {
        return Err(endpoint_error(
            kind,
            format!("{trimmed} must include a host"),
        ));
    }
    Ok(Some(url))
}

fn endpoint_error(kind: EndpointKind, message: String) -> VoiceConfigError {
    match kind {
        EndpointKind::Transcription => VoiceConfigError::Transcription(message),
        EndpointKind::Synthesis => VoiceConfigError::Synthesis(message),
    }
}

fn build_client() -> Result<reqwest::Client, VoiceConfigError> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::custom(redirect_policy))
        .connect_timeout(CONNECT_TIMEOUT)
        .build()
        .map_err(|error| VoiceConfigError::Client(error.to_string()))
}

fn redirect_policy(attempt: reqwest::redirect::Attempt<'_>) -> reqwest::redirect::Action {
    if attempt.previous().len() >= REDIRECT_LIMIT {
        return attempt.error(RedirectFault::Limit);
    }
    match attempt.url().scheme() {
        "http" | "https" => attempt.follow(),
        _ => attempt.error(RedirectFault::Scheme),
    }
}

#[derive(Debug, Error)]
enum RedirectFault {
    #[error("redirect target is not http(s)")]
    Scheme,
    #[error("too many voice redirects")]
    Limit,
}

#[derive(Clone, Copy)]
enum VoiceRoute {
    Transcribe,
    Speak,
}

impl std::fmt::Display for VoiceRoute {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Transcribe => "transcribe",
            Self::Speak => "speak",
        })
    }
}

#[derive(Debug, Error)]
enum VoiceError {
    #[error("voice transcription is not configured on this node")]
    TranscriptionUnconfigured,
    #[error("voice synthesis is not configured on this node")]
    SynthesisUnconfigured,
    #[error("empty audio body")]
    EmptyAudio,
    #[error("audio body too large")]
    AudioTooLarge,
    #[error("speak body too large")]
    SpeakTooLarge,
    #[error("speak body must be JSON")]
    SpeakJson,
    #[error("input required")]
    InputRequired,
    #[error("input exceeds 4096 UTF-8 bytes")]
    InputTooLong,
    #[error("{0}")]
    SpeakInvalid(String),
    #[error("voice upstream failed: {0}")]
    Upstream(String),
    #[error("the voice request body failed: {0}")]
    Body(String),
    #[error("voice media type is invalid: {0}")]
    Media(String),
    #[error("the voice client is missing")]
    ClientMissing,
    #[error("the voice response could not be built: {0}")]
    Response(String),
}

impl IntoResponse for VoiceError {
    fn into_response(self) -> Response {
        let status = match &self {
            Self::TranscriptionUnconfigured | Self::SynthesisUnconfigured => {
                StatusCode::NOT_IMPLEMENTED
            }
            Self::AudioTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Self::Upstream(_) => StatusCode::BAD_GATEWAY,
            Self::Media(_) | Self::ClientMissing | Self::Response(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
            Self::EmptyAudio
            | Self::SpeakTooLarge
            | Self::SpeakJson
            | Self::InputRequired
            | Self::InputTooLong
            | Self::SpeakInvalid(_)
            | Self::Body(_) => StatusCode::BAD_REQUEST,
        };
        (status, Json(GatewayError::new(self.to_string()))).into_response()
    }
}

fn logged(route: VoiceRoute, snippet: String) -> VoiceError {
    tracing::warn!("[voice] {route} upstream failed: {snippet}");
    VoiceError::Upstream(snippet)
}

fn transport_failure(route: VoiceRoute, error: &reqwest::Error) -> VoiceError {
    logged(route, take_chars(&error_chain(error), SNIPPET_CHARS))
}

fn error_chain(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    for _ in 0..8 {
        let Some(next) = source else {
            break;
        };
        text.push_str(": ");
        text.push_str(&next.to_string());
        source = next.source();
    }
    text
}

async fn read_request(body: Body, limit: usize, prefix: bool) -> Result<Vec<u8>, VoiceError> {
    let too_large = if limit == SPEAK_LIMIT {
        VoiceError::SpeakTooLarge
    } else {
        VoiceError::AudioTooLarge
    };
    let stream = Box::pin(body.into_data_stream());
    match collect(stream, limit, prefix).await {
        Ok(bytes) => Ok(bytes),
        Err(ReadFault::TooLarge) => Err(too_large),
        Err(ReadFault::Body(error)) => Err(VoiceError::Body(error)),
    }
}

async fn read_upstream(
    response: reqwest::Response,
    limit: usize,
    prefix: bool,
    route: VoiceRoute,
) -> Result<Vec<u8>, VoiceError> {
    let stream = Box::pin(response.bytes_stream());
    match collect(stream, limit, prefix).await {
        Ok(bytes) => Ok(bytes),
        Err(ReadFault::TooLarge) => Err(logged(
            route,
            "upstream transcription exceeds 1 MiB".to_owned(),
        )),
        Err(ReadFault::Body(error)) => Err(logged(route, take_chars(&error, SNIPPET_CHARS))),
    }
}

enum ReadFault {
    TooLarge,
    Body(String),
}

async fn collect<S, E>(mut stream: S, limit: usize, prefix: bool) -> Result<Vec<u8>, ReadFault>
where
    S: futures::Stream<Item = Result<axum::body::Bytes, E>> + Unpin,
    E: std::fmt::Display,
{
    let mut buf = Vec::new();
    while let Some(item) = stream.next().await {
        let chunk = item.map_err(|error| ReadFault::Body(error.to_string()))?;
        let Some(next) = buf.len().checked_add(chunk.len()) else {
            return Err(ReadFault::TooLarge);
        };
        if next > limit {
            if prefix {
                let remaining = limit.saturating_sub(buf.len());
                if let Some(part) = chunk.get(..remaining) {
                    buf.extend_from_slice(part);
                }
                return Ok(buf);
            }
            return Err(ReadFault::TooLarge);
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

fn snippet(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return "upstream body was empty".to_owned();
    }
    match std::str::from_utf8(bytes) {
        Ok(text) => take_chars(text, SNIPPET_CHARS),
        Err(error) => {
            let Some(valid) = bytes.get(..error.valid_up_to()) else {
                return "upstream body is not utf-8".to_owned();
            };
            if valid.is_empty() {
                "upstream body is not utf-8".to_owned()
            } else if let Ok(text) = std::str::from_utf8(valid) {
                take_chars(text, SNIPPET_CHARS)
            } else {
                "upstream body is not utf-8".to_owned()
            }
        }
    }
}

fn take_chars(text: &str, max: usize) -> String {
    let Some((end, _)) = text.char_indices().nth(max) else {
        return text.to_owned();
    };
    match text.get(..end) {
        Some(prefix) => prefix.to_owned(),
        None => text.to_owned(),
    }
}

fn capped_audio(
    response: reqwest::Response,
) -> impl futures::Stream<Item = Result<axum::body::Bytes, axum::Error>> + Send {
    let stream = Box::pin(response.bytes_stream());
    futures::stream::unfold(
        (stream, 0usize, false),
        |(mut stream, seen, done)| async move {
            if done {
                return None;
            }
            match stream.next().await {
                None => None,
                Some(Ok(chunk)) => match seen.checked_add(chunk.len()) {
                    Some(total) if total <= SPEAK_STREAM => {
                        Some((Ok(chunk), (stream, total, false)))
                    }
                    Some(_) | None => {
                        Some((Err(axum::Error::new(StreamLimit)), (stream, seen, true)))
                    }
                },
                Some(Err(error)) => Some((Err(axum::Error::new(error)), (stream, seen, true))),
            }
        },
    )
}

#[derive(Debug, Error)]
#[error("upstream audio exceeds 30 MiB")]
struct StreamLimit;

pub(crate) fn router() -> Router<Arc<Gateway>> {
    Router::new()
        .route(
            "/api/voice/transcribe",
            post(transcribe)
                .fallback(missing)
                .layer(DefaultBodyLimit::disable()),
        )
        .route(
            "/api/voice/speak",
            post(speak)
                .fallback(missing)
                .layer(DefaultBodyLimit::disable()),
        )
}

async fn transcribe(State(gateway): State<Arc<Gateway>>, request: Request) -> Response {
    answered(gateway.voice().transcribe(request).await)
}

async fn speak(State(gateway): State<Arc<Gateway>>, request: Request) -> Response {
    answered(gateway.voice().speak(request).await)
}

fn answered(result: Result<Response, VoiceError>) -> Response {
    match result {
        Ok(response) => response,
        Err(error) => error.into_response(),
    }
}

async fn missing() -> Response {
    (StatusCode::NOT_FOUND, Json(GatewayError::new("not found"))).into_response()
}
