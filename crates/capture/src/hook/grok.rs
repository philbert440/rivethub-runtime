use std::path::Path;
use std::path::PathBuf;

use chrono::DateTime;
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;
use wire::JsonObject;
use wire::capture::CaptureBatchRequest;
use wire::harness::HarnessId;
use wire::session_id::SessionId;

use super::HookRun;
use super::append_log;
use super::iso_at;
use crate::listing::StoreRoots;
use crate::listing::grok_title;
use crate::readers::grok::GrokTranscriptReader;
use crate::writer::CaptureTransport;
use crate::writer::CaptureWriter;
use crate::writer::Totals;

pub const AGENT: &str = "rivet-grok";
pub const CHANNEL: &str = "grok-build";
const SOURCE: &str = "grok-build-hook";
const LOG_FILE: &str = "grok-capture.log";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, strum::Display)]
#[serde(rename_all = "PascalCase")]
#[strum(serialize_all = "PascalCase")]
enum GrokEvent {
    Stop,
    PreCompact,
    SessionEnd,
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
struct GrokPayload {
    session_id: Option<String>,
    hook_event_name: Option<GrokEvent>,
    cwd: Option<PathBuf>,
}

#[derive(Debug, Serialize)]
struct GrokSettings<'a> {
    source: &'static str,
    session_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    cwd: Option<String>,
    last_event: GrokEvent,
    last_ingest_at: String,
}

#[derive(Debug)]
pub struct GrokHook<T: CaptureTransport> {
    writer: CaptureWriter<T>,
    reader: GrokTranscriptReader,
    store: PathBuf,
    log_file: PathBuf,
}

impl<T: CaptureTransport> GrokHook<T> {
    #[must_use]
    pub fn new(
        writer: CaptureWriter<T>,
        roots: &StoreRoots,
        paths: &config::Paths,
    ) -> Option<Self> {
        let store = roots.grok.clone()?;
        Some(Self {
            writer,
            reader: GrokTranscriptReader::new(store.clone()),
            store,
            log_file: paths.state_dir().join(LOG_FILE),
        })
    }

    #[must_use]
    pub fn log_file(&self) -> &Path {
        &self.log_file
    }

    pub async fn run(&self, stdin: &str, now: DateTime<Utc>) -> HookRun {
        let payload = match serde_json::from_str::<GrokPayload>(stdin) {
            Ok(payload) => payload,
            Err(error) => {
                tracing::warn!("grok hook payload was not valid: {error}");
                return HookRun::done();
            }
        };
        let Some(event) = payload
            .hook_event_name
            .filter(|event| *event != GrokEvent::Other)
        else {
            return HookRun::done();
        };
        let Some(native) = payload.session_id.as_deref().filter(|id| !id.is_empty()) else {
            return HookRun::done();
        };
        let session = SessionId::new(HarnessId::GrokBuild, native);
        let slice = match self.reader.capture(&session).await {
            Ok(slice) => slice,
            Err(error) => return HookRun::failed(error.to_string()),
        };
        if slice.messages.is_empty() {
            return HookRun::done();
        }
        let settings = match JsonObject::from_serialize(&GrokSettings {
            source: SOURCE,
            session_id: native,
            cwd: payload.cwd.as_ref().map(|path| path.display().to_string()),
            last_event: event,
            last_ingest_at: iso_at(now),
        }) {
            Ok(settings) => settings,
            Err(error) => return HookRun::failed(error.to_string()),
        };
        let batch = CaptureBatchRequest {
            session_key: session.to_string(),
            agent: AGENT.to_owned(),
            channel: Some(CHANNEL.to_owned()),
            title: self.title(native).await,
            settings: Some(settings),
            task_id: None,
            finalize: (event == GrokEvent::SessionEnd).then_some(true),
            created_at: None,
            updated_at: None,
            messages: slice.messages,
        };
        match self.writer.write(&batch, now).await {
            Ok(totals) => {
                let line = log_line(event, &batch.session_key, &totals, now);
                if let Err(error) = append_log(&self.log_file, &line).await {
                    tracing::warn!(
                        "hook log {} was not written: {error}",
                        self.log_file.display()
                    );
                }
                HookRun::done()
            }
            Err(error) => HookRun::failed(error.to_string()),
        }
    }

    async fn title(&self, native: &str) -> Option<String> {
        match grok_title(self.store.clone(), native.to_owned()).await {
            Ok(title) => title,
            Err(error) => {
                tracing::debug!("grok session {native} has no title: {error}");
                None
            }
        }
    }
}

fn log_line(event: GrokEvent, session_key: &str, totals: &Totals, now: DateTime<Utc>) -> String {
    format!(
        "{} {event} {session_key}: updated conv {} — +{} msg (had {})",
        iso_at(now),
        totals.conversation_id,
        totals.inserted,
        totals.skipped
    )
}
