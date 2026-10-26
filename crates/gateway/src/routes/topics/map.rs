use kernel::topics::Alias;
use kernel::topics::PlacedSession;
use kernel::topics::Placement;
use kernel::topics::PlacementSource;
use kernel::topics::Root;
use kernel::topics::Topic;
use kernel::topics::TopicError;
use kernel::topics::TopicListing;
use wire::gateway::topics::PlacedBy;
use wire::gateway::topics::TopicDirectory;
use wire::gateway::topics::TopicPlacement;
use wire::gateway::topics::TopicRecord;
use wire::gateway::topics::TopicSession;
use wire::gateway::topics::TopicSummary;

use crate::error::ApiError;

pub(super) fn error(error: TopicError) -> ApiError {
    match error {
        TopicError::EmptyName
        | TopicError::EmptyAlias
        | TopicError::EmptySlug { .. }
        | TopicError::OwnParent
        | TopicError::SelfMerge => ApiError::BadRequest(error.to_string()),
        TopicError::SlugTaken { .. }
        | TopicError::AliasTaken { .. }
        | TopicError::ParentIsSubtopic { .. }
        | TopicError::HasSubtopics
        | TopicError::MergeWouldNest { .. } => ApiError::Conflict(error.to_string()),
        TopicError::NotFound { .. } => ApiError::Missing(error.to_string()),
        TopicError::Unavailable(source) => {
            tracing::error!("topic store failed: {source}");
            ApiError::Unavailable("topic store unavailable".into())
        }
    }
}

pub(super) fn aliases(texts: &[String]) -> Result<Vec<Alias>, ApiError> {
    texts
        .iter()
        .map(|text| Alias::new(text).map_err(error))
        .collect()
}

pub(super) fn root(directory: TopicDirectory) -> Root {
    match directory {
        TopicDirectory::Local { node, path } => Root::Local { node, path },
        TopicDirectory::Shared { path } => Root::Shared { path },
    }
}

fn directory(root: Root) -> TopicDirectory {
    match root {
        Root::Local { node, path } => TopicDirectory::Local { node, path },
        Root::Shared { path } => TopicDirectory::Shared { path },
    }
}

const fn placed_by(source: PlacementSource) -> PlacedBy {
    match source {
        PlacementSource::You => PlacedBy::You,
        PlacementSource::Categorizer => PlacedBy::Categorizer,
    }
}

pub(super) fn record(topic: Topic) -> TopicRecord {
    TopicRecord {
        id: topic.id.to_string(),
        slug: topic.slug.to_string(),
        name: topic.name.as_str().to_owned(),
        aliases: topic
            .aliases
            .iter()
            .map(|alias| alias.as_str().to_owned())
            .collect(),
        parent: topic.parent.map(|parent| parent.to_string()),
        directory: topic.directory.map(directory),
        created_at: topic.created_at,
        updated_at: topic.updated_at,
    }
}

pub(super) fn summary(listing: TopicListing) -> TopicSummary {
    TopicSummary {
        topic: record(listing.topic),
        session_count: listing.session_count,
        last_active: listing.last_active,
    }
}

pub(super) fn placement(placement: Placement) -> TopicPlacement {
    TopicPlacement {
        session_id: placement.session,
        topic_id: placement.topic.to_string(),
        by: placed_by(placement.by),
        at: placement.at,
    }
}

pub(super) fn session(placed: PlacedSession) -> TopicSession {
    TopicSession {
        placement: placement(placed.placement),
        last_active: placed.last_active,
    }
}
