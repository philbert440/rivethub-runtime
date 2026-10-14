use std::sync::Arc;

use axum::extract::Path;
use axum::extract::rejection::JsonRejection;
use axum::{Extension, Json};
use chrono::Utc;
use uuid::Uuid;
use wire::gateway::OkReply;
use wire::gateway::v1::{Conversation, ConversationTitle, Conversations};

use super::{Error, Service, service};

pub(super) fn identifier(raw: &str) -> Result<Uuid, Error> {
    Uuid::parse_str(raw).map_err(|_| Error::Invalid("invalid conversation id".to_owned()))
}

pub(super) fn title(raw: String) -> Result<String, Error> {
    if raw.trim().is_empty() || raw.chars().count() > 200 {
        return Err(Error::Invalid(
            "title must contain 1 to 200 characters".to_owned(),
        ));
    }
    Ok(raw)
}

pub(super) async fn list(
    extension: Option<Extension<Arc<Service>>>,
) -> Result<Json<Conversations>, Error> {
    let service = service(extension)?;
    Ok(Json(Conversations {
        conversations: service.conversations.list(),
    }))
}

pub(super) async fn create(
    extension: Option<Extension<Arc<Service>>>,
    body: Result<Json<ConversationTitle>, JsonRejection>,
) -> Result<Json<Conversation>, Error> {
    let service = service(extension)?;
    let body = super::body(body)?;
    let conversation = service
        .conversations
        .create(
            title(body.title)?,
            Utc::now().timestamp_millis(),
            &service.tasks,
            &service.cancel,
        )
        .await?;
    Ok(Json(conversation))
}

pub(super) async fn get(
    extension: Option<Extension<Arc<Service>>>,
    Path(id): Path<String>,
) -> Result<Json<Conversation>, Error> {
    let service = service(extension)?;
    Ok(Json(
        service.conversations.entry(identifier(&id)?)?.snapshot()?,
    ))
}

pub(super) async fn rename(
    extension: Option<Extension<Arc<Service>>>,
    Path(id): Path<String>,
    body: Result<Json<ConversationTitle>, JsonRejection>,
) -> Result<Json<Conversation>, Error> {
    let service = service(extension)?;
    let body = super::body(body)?;
    let record = service
        .conversations
        .entry(identifier(&id)?)?
        .rename(title(body.title)?, Utc::now().timestamp_millis())
        .await?;
    Ok(Json(record))
}

pub(super) async fn delete(
    extension: Option<Extension<Arc<Service>>>,
    Path(id): Path<String>,
) -> Result<Json<OkReply>, Error> {
    let service = service(extension)?;
    service
        .conversations
        .delete(identifier(&id)?, Utc::now().timestamp_millis())
        .await?;
    Ok(Json(OkReply { ok: true }))
}
