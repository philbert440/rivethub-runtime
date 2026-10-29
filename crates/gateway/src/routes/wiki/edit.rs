use std::sync::Arc;

use axum::extract::Path;
use axum::extract::State;
use axum::response::Response;
use chrono::Utc;
use kernel::topics::PageEdit;
use kernel::wiki::builder::WikiStoreError;
use wire::gateway::wiki::WikiPageEdit;

use super::Format;
use super::WikiError;
use super::requested_slug;
use super::respond;
use crate::Gateway;
use crate::error::ApiError;
use crate::extract::Body;

pub(super) async fn edit(
    State(gateway): State<Arc<Gateway>>,
    Path(slug): Path<String>,
    Body(request): Body<WikiPageEdit>,
) -> Result<Response, WikiError> {
    let slug = requested_slug(slug)?;
    let edit = PageEdit {
        current_state: request.current_state,
        article: request.article,
    };
    let edited = gateway
        .pages()?
        .edit_page(&slug, &edit, Utc::now())
        .await
        .map_err(refused)?;
    if !edited {
        return Err(
            ApiError::Missing(format!("{} is not a topic or entity page", slug.as_ref())).into(),
        );
    }
    respond(&gateway, slug, Format::Json).await
}

fn refused(error: WikiStoreError) -> ApiError {
    match error {
        WikiStoreError::TopicMissing(slug) => {
            ApiError::Missing(format!("unknown wiki page {slug}"))
        }
        WikiStoreError::DirectoryMissing => {
            ApiError::Unavailable("wiki directory is not configured".into())
        }
        other @ (WikiStoreError::Page(_)
        | WikiStoreError::Query(_)
        | WikiStoreError::Missing(_)
        | WikiStoreError::ConversationMismatch(_)
        | WikiStoreError::SlugConflict(_)
        | WikiStoreError::InvalidPatch(_)) => {
            tracing::error!("wiki edit failed: {other}");
            ApiError::Internal
        }
    }
}
