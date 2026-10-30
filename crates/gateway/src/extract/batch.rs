use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use chrono::DateTime;
use serde::Deserialize;
use serde_json::error::Category;
use serde_json::value::RawValue;
use wire::events::{Activity, AgentEvent, AgentEventKind, ProtocolVersion};

use crate::error::{ApiError, invalid_json};

const BATCH_LIMIT: usize = 100;
const BATCH_SHAPE: &str = "expected an array of 1-100 events";
const REPORTED_NAMES: usize = 64;

static REPORTED: LazyLock<parking_lot::Mutex<BTreeSet<String>>> = LazyLock::new(Default::default);

pub(crate) fn decode_event_batch(bytes: &[u8]) -> Result<Vec<AgentEvent>, ApiError> {
    let elements: Vec<&RawValue> = match serde_json::from_slice(bytes) {
        Ok(elements) => elements,
        Err(error) if error.classify() == Category::Data => {
            return Err(ApiError::BadRequest(BATCH_SHAPE.to_owned()));
        }
        Err(_) => return Err(invalid_json()),
    };
    if elements.is_empty() || elements.len() > BATCH_LIMIT {
        return Err(ApiError::BadRequest(BATCH_SHAPE.to_owned()));
    }
    let mut kept = Vec::with_capacity(elements.len());
    let mut skipped: BTreeMap<String, usize> = BTreeMap::new();
    let mut generic: BTreeMap<String, usize> = BTreeMap::new();
    for (index, element) in elements.into_iter().enumerate() {
        let error = match serde_json::from_str::<AgentEvent>(element.get()) {
            Ok(event) => {
                kept.push(event);
                continue;
            }
            Err(error) => error,
        };
        match unread(element, &error) {
            Some(Unread::Kind(kind)) => *skipped.entry(kind).or_default() += 1,
            Some(Unread::Activity(event, name)) => {
                *generic.entry(name).or_default() += 1;
                kept.push(*event);
            }
            None => {
                tracing::debug!("event batch element {index}: {error}");
                return Err(ApiError::Unprocessable(format!(
                    "event[{index}] is not a valid v1 AgentEvent"
                )));
            }
        }
    }
    report(
        &skipped,
        "skipping events of a kind this node doesn't know; the rest of their batches are kept",
    );
    report(
        &generic,
        "keeping an activity this node doesn't know as thinking",
    );
    Ok(kept)
}

enum Unread {
    Kind(String),
    Activity(Box<AgentEvent>, String),
}

#[derive(Deserialize)]
struct Tagged {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default, rename = "activity")]
    activity_name: Option<String>,
}

#[derive(Deserialize)]
struct NamedActivity {
    v: ProtocolVersion,
    session: String,
    #[serde(rename = "type")]
    _kind: String,
    #[serde(rename = "activity")]
    activity_name: String,
    #[serde(default)]
    ts: Option<i64>,
    #[serde(flatten)]
    extra: wire::Extra,
}

fn unread(element: &RawValue, error: &serde_json::Error) -> Option<Unread> {
    let name = unknown_variant(&error.to_string())?;
    let tagged = match serde_json::from_str::<Tagged>(element.get()) {
        Ok(tagged) => tagged,
        Err(untagged) => {
            tracing::debug!("an event naming an unknown value has no readable type: {untagged}");
            return None;
        }
    };
    if tagged.kind == name {
        return Some(Unread::Kind(name));
    }
    if tagged.activity_name.as_deref() != Some(name.as_str()) {
        return None;
    }
    match serde_json::from_str::<NamedActivity>(element.get()) {
        Ok(named) => Some(Unread::Activity(
            Box::new(named.as_thinking()),
            named.activity_name,
        )),
        Err(unshaped) => {
            tracing::debug!(
                "an activity of an unknown value is not otherwise an activity event: {unshaped}"
            );
            None
        }
    }
}

impl NamedActivity {
    fn as_thinking(&self) -> AgentEvent {
        AgentEvent {
            v: self.v,
            session: self.session.clone(),
            kind: AgentEventKind::Activity {
                activity: Activity::Thinking,
                ts: self.ts.and_then(DateTime::from_timestamp_millis),
            },
            extra: self.extra.clone(),
        }
    }
}

fn unknown_variant(message: &str) -> Option<String> {
    let rest = message.strip_prefix("unknown variant `")?;
    rest.split_once('`').map(|(name, _)| name.to_owned())
}

fn report(counts: &BTreeMap<String, usize>, warning: &str) {
    if counts.is_empty() {
        return;
    }
    tracing::debug!(?counts, "{warning}");
    let mut reported = REPORTED.lock();
    for name in counts.keys() {
        if reported.len() < REPORTED_NAMES && reported.insert(name.clone()) {
            tracing::warn!(name = name.as_str(), "{warning}");
        }
    }
}
