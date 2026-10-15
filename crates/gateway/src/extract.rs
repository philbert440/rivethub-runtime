use std::error::Error;
use std::future::Future;
use std::str::FromStr;

use axum::body::Bytes;
use axum::extract::FromRequest;
use axum::extract::FromRequestParts;
use axum::extract::Request;
use axum::extract::rejection::BytesRejection;
use axum::extract::rejection::FailedToBufferBody;
use axum::http::request::Parts;
use chrono::DateTime;
use chrono::Utc;
use kernel::memory::TagLiteral;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde::de::Deserializer;
use serde::de::SeqAccess;
use serde::de::Visitor;
use serde_json::error::Category;
use wire::JsonObject;
use wire::events::AgentEvent;
use wire::gateway::memory::BrowseWindow;
use wire::gateway::memory::BrowseWindowError;
use wire::gateway::memory::MemoryScope;
use wire::gateway::memory::TagEntityType;
use wire::gateway::memory::TagState;
use wire::message::Role;

use crate::error::ApiError;
use crate::error::body_must_be_object;
use crate::error::invalid_json;
use crate::server::RequestDeadlineExceeded;

#[derive(Debug)]
pub struct Body<T>(pub T);

pub async fn limited_bytes<S: Send + Sync>(request: Request, state: &S) -> Result<Bytes, ApiError> {
    Bytes::from_request(request, state)
        .await
        .map_err(|rejection| {
            if carries_deadline_error(&rejection) {
                ApiError::RequestTimeout
            } else if let BytesRejection::FailedToBufferBody(
                FailedToBufferBody::LengthLimitError(_),
            ) = rejection
            {
                ApiError::BodyTooLarge
            } else {
                invalid_json()
            }
        })
}

fn carries_deadline_error(rejection: &BytesRejection) -> bool {
    let mut cause: Option<&(dyn Error + 'static)> = Some(rejection);
    while let Some(error) = cause {
        if error.is::<RequestDeadlineExceeded>() {
            return true;
        }
        cause = error.source();
    }
    false
}

impl<S, T> FromRequest<S> for Body<T>
where
    S: Send + Sync,
    T: DeserializeOwned,
{
    type Rejection = ApiError;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        let bytes = limited_bytes(request, state).await?;
        Ok(Self(decode_json_object(&bytes)?))
    }
}

fn decode_json_object<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, ApiError> {
    if let Err(error) = serde_json::from_slice::<JsonObject>(bytes) {
        return Err(object_shape_error(&error));
    }
    match serde_json::from_slice::<T>(bytes) {
        Ok(value) => Ok(value),
        Err(error) => Err(typed_value_error(&error)),
    }
}

fn object_shape_error(error: &serde_json::Error) -> ApiError {
    match error.classify() {
        Category::Data => body_must_be_object(),
        Category::Io | Category::Syntax | Category::Eof => invalid_json(),
    }
}

fn typed_value_error(error: &serde_json::Error) -> ApiError {
    match error.classify() {
        Category::Data => ApiError::BadRequest(error.to_string()),
        Category::Io | Category::Syntax | Category::Eof => invalid_json(),
    }
}

#[derive(Debug)]
pub struct Query<T>(pub T);

impl<S, T> FromRequestParts<S> for Query<T>
where
    S: Send + Sync,
    T: DeserializeOwned + Send,
{
    type Rejection = ApiError;

    fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        let raw = parts.uri.query().unwrap_or_default();
        std::future::ready(
            serde_urlencoded::from_str(raw)
                .map(Self)
                .map_err(|error| ApiError::BadRequest(error.to_string())),
        )
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct ParamError(pub &'static str);

fn enum_from_str<T: DeserializeOwned>(raw: &str) -> Option<T> {
    T::deserialize(serde::de::value::StrDeserializer::<serde::de::value::Error>::new(raw)).ok()
}

pub fn parse_enum<T: DeserializeOwned>(raw: &str) -> Option<T> {
    enum_from_str(raw)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct ScopeParam(pub MemoryScope);

impl TryFrom<String> for ScopeParam {
    type Error = ParamError;

    fn try_from(raw: String) -> Result<Self, Self::Error> {
        enum_from_str::<MemoryScope>(&raw)
            .map(Self)
            .ok_or(ParamError("invalid scope"))
    }
}

impl ScopeParam {
    #[must_use]
    pub const fn get(self) -> MemoryScope {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct WindowParam(pub BrowseWindow);

impl TryFrom<String> for WindowParam {
    type Error = BrowseWindowError;

    fn try_from(raw: String) -> Result<Self, Self::Error> {
        BrowseWindow::parse(&raw).map(Self)
    }
}

impl WindowParam {
    #[must_use]
    pub const fn get(self) -> BrowseWindow {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct TagFilter(pub TagLiteral);

impl TryFrom<String> for TagFilter {
    type Error = ParamError;

    fn try_from(raw: String) -> Result<Self, Self::Error> {
        TagLiteral::from_str(&raw)
            .map(Self)
            .map_err(|_| ParamError("tag must be key:value"))
    }
}

impl TagFilter {
    #[must_use]
    pub fn get(self) -> TagLiteral {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct RawLimit(pub u32);

impl TryFrom<String> for RawLimit {
    type Error = ParamError;

    fn try_from(raw: String) -> Result<Self, Self::Error> {
        raw.parse()
            .map(Self)
            .map_err(|_| ParamError("invalid limit"))
    }
}

impl RawLimit {
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct RoleParam(pub Role);

impl TryFrom<String> for RoleParam {
    type Error = ParamError;

    fn try_from(raw: String) -> Result<Self, Self::Error> {
        enum_from_str::<Role>(&raw)
            .map(Self)
            .ok_or(ParamError("invalid role"))
    }
}

impl RoleParam {
    #[must_use]
    pub const fn get(self) -> Role {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct SinceParam(pub DateTime<Utc>);

impl TryFrom<String> for SinceParam {
    type Error = ParamError;

    fn try_from(raw: String) -> Result<Self, Self::Error> {
        DateTime::parse_from_rfc3339(&raw)
            .map(|parsed| Self(parsed.with_timezone(&Utc)))
            .map_err(|_| ParamError("invalid since"))
    }
}

impl SinceParam {
    #[must_use]
    pub const fn get(self) -> DateTime<Utc> {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct BeforeParam(pub DateTime<Utc>);

impl TryFrom<String> for BeforeParam {
    type Error = ParamError;

    fn try_from(raw: String) -> Result<Self, Self::Error> {
        DateTime::parse_from_rfc3339(&raw)
            .map(|parsed| Self(parsed.with_timezone(&Utc)))
            .map_err(|_| ParamError("invalid before"))
    }
}

impl BeforeParam {
    #[must_use]
    pub const fn get(self) -> DateTime<Utc> {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct EntityTypeParam(pub TagEntityType);

impl TryFrom<String> for EntityTypeParam {
    type Error = ParamError;

    fn try_from(raw: String) -> Result<Self, Self::Error> {
        enum_from_str::<TagEntityType>(&raw)
            .map(Self)
            .ok_or(ParamError("bad entity_type"))
    }
}

impl EntityTypeParam {
    #[must_use]
    pub const fn get(self) -> TagEntityType {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct EntityIdParam(pub uuid::Uuid);

impl TryFrom<String> for EntityIdParam {
    type Error = ParamError;

    fn try_from(raw: String) -> Result<Self, Self::Error> {
        raw.parse()
            .map(Self)
            .map_err(|_| ParamError("bad entity_id"))
    }
}

impl EntityIdParam {
    #[must_use]
    pub const fn get(self) -> uuid::Uuid {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct StateParam(pub TagState);

impl TryFrom<String> for StateParam {
    type Error = ParamError;

    fn try_from(raw: String) -> Result<Self, Self::Error> {
        enum_from_str::<TagState>(&raw)
            .map(Self)
            .ok_or(ParamError("bad state"))
    }
}

impl StateParam {
    #[must_use]
    pub const fn get(self) -> TagState {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct StateFilter(pub Vec<TagState>);

impl TryFrom<String> for StateFilter {
    type Error = ParamError;

    fn try_from(raw: String) -> Result<Self, Self::Error> {
        raw.split(',')
            .map(|state| enum_from_str::<TagState>(state.trim()).ok_or(ParamError("bad state")))
            .collect::<Result<Vec<_>, _>>()
            .map(Self)
    }
}

impl StateFilter {
    #[must_use]
    pub fn into_vec(self) -> Vec<TagState> {
        self.0
    }
}

const BATCH_LIMIT: usize = 100;
const BATCH_SHAPE: &str = "expected an array of 1-100 events";

struct EventBatch(Vec<AgentEvent>);

struct BatchVisitor;

impl<'de> Visitor<'de> for BatchVisitor {
    type Value = EventBatch;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(BATCH_SHAPE)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut events = Vec::new();
        loop {
            let index = events.len();
            if index == BATCH_LIMIT {
                return match seq.next_element::<AgentEvent>() {
                    Ok(None) => Ok(EventBatch(events)),
                    Ok(Some(_)) => Err(serde::de::Error::custom(BATCH_SHAPE)),
                    Err(error) => element_error(index, &error),
                };
            }
            match seq.next_element::<AgentEvent>() {
                Ok(Some(event)) => events.push(event),
                Ok(None) => {
                    return if events.is_empty() {
                        Err(serde::de::Error::custom(BATCH_SHAPE))
                    } else {
                        Ok(EventBatch(events))
                    };
                }
                Err(error) => return element_error(index, &error),
            }
        }
    }
}

fn element_error<E: serde::de::Error>(
    index: usize,
    error: &impl std::fmt::Display,
) -> Result<EventBatch, E> {
    tracing::debug!("event batch element {index}: {error}");
    Err(serde::de::Error::custom(format!(
        "event[{index}] is not a valid v1 AgentEvent"
    )))
}

pub(crate) fn decode_event_batch(bytes: &[u8]) -> Result<Vec<AgentEvent>, ApiError> {
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let events = match Deserializer::deserialize_seq(&mut deserializer, BatchVisitor) {
        Ok(EventBatch(events)) => events,
        Err(error) => return Err(batch_error(&error)),
    };
    match deserializer.end() {
        Ok(()) => Ok(events),
        Err(error) => Err(batch_error(&error)),
    }
}

fn batch_error(error: &serde_json::Error) -> ApiError {
    let sentence = strip_location(&error.to_string());
    if sentence.starts_with("event[") {
        return ApiError::Unprocessable(sentence);
    }
    match error.classify() {
        Category::Data => ApiError::BadRequest(BATCH_SHAPE.to_owned()),
        Category::Io | Category::Syntax | Category::Eof => invalid_json(),
    }
}

fn strip_location(text: &str) -> String {
    match text.rfind(" at line ") {
        Some(index) if text.is_char_boundary(index) => {
            let (head, _) = text.split_at(index);
            head.to_owned()
        }
        Some(_) | None => text.to_owned(),
    }
}
