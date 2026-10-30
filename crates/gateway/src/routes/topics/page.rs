use std::sync::Arc;

use axum::Json;
use axum::extract::Path;
use axum::extract::State;
use chrono::Utc;
use kernel::topics::PageEdit;
use kernel::topics::Topic;
use kernel::topics::TopicPageError;
use kernel::topics::bound_page;
use kernel::topics::edit_topic_page;
use kernel::wiki::builder::WikiStoreError;
use wire::gateway::topics::TopicPage;
use wire::gateway::wiki::WikiPageEdit;

use super::map;
use super::resolve;
use crate::Gateway;
use crate::error::ApiError;
use crate::extract::Body;

pub(super) async fn page(
    State(gateway): State<Arc<Gateway>>,
    Path(key): Path<String>,
) -> Result<Json<TopicPage>, ApiError> {
    let topic = resolve(gateway.topics()?.as_ref(), &key).await?;
    answer(&gateway, topic).await
}

pub(super) async fn edit(
    State(gateway): State<Arc<Gateway>>,
    Path(key): Path<String>,
    Body(request): Body<WikiPageEdit>,
) -> Result<Json<TopicPage>, ApiError> {
    let store = gateway.topics()?;
    let topic = resolve(store.as_ref(), &key).await?;
    let edit = PageEdit {
        current_state: request.current_state,
        article: request.article,
    };
    let edited = edit_topic_page(store.as_ref(), gateway.pages()?, &topic, &edit, Utc::now())
        .await
        .map_err(refused)?;
    if edited.is_none() {
        return Err(ApiError::Conflict("A type has no page".to_owned()));
    }
    answer(&gateway, topic).await
}

async fn answer(gateway: &Gateway, topic: Topic) -> Result<Json<TopicPage>, ApiError> {
    let page = bound_page(gateway.topics()?.as_ref(), gateway.store().as_ref(), &topic)
        .await
        .map_err(map::page_error)?;
    Ok(Json(TopicPage {
        topic: map::record(topic),
        page,
    }))
}

fn refused(error: TopicPageError) -> ApiError {
    match error {
        TopicPageError::Wiki(WikiStoreError::DirectoryMissing) => {
            ApiError::Unavailable("wiki directory is not configured".into())
        }
        other => map::page_error(other),
    }
}
