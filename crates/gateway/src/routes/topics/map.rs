use kernel::topics::Alias;
use kernel::topics::Decision;
use kernel::topics::DecisionStatus;
use kernel::topics::PlacedSession;
use kernel::topics::Placement;
use kernel::topics::PlacementSource;
use kernel::topics::Root;
use kernel::topics::Suggestion;
use kernel::topics::Topic;
use kernel::topics::TopicError;
use kernel::topics::TopicListing;
use kernel::topics::TopicPageError;
use wire::gateway::topics::PlacedBy;
use wire::gateway::topics::TopicDecision;
use wire::gateway::topics::TopicDirectory;
use wire::gateway::topics::TopicPlacement;
use wire::gateway::topics::TopicRecord;
use wire::gateway::topics::TopicSession;
use wire::gateway::topics::TopicSuggestion;
use wire::gateway::topics::TopicSummary;

use crate::error::ApiError;

pub(super) fn error(error: TopicError) -> ApiError {
    match error {
        TopicError::EmptyName
        | TopicError::EmptyAlias
        | TopicError::EmptySlug { .. }
        | TopicError::OwnParent
        | TopicError::SelfMerge
        | TopicError::EmptyDecision
        | TopicError::ForeignReplacement => ApiError::BadRequest(error.to_string()),
        TopicError::SlugTaken { .. }
        | TopicError::AliasTaken { .. }
        | TopicError::ParentIsSubtopic { .. }
        | TopicError::HasSubtopics
        | TopicError::MergeWouldNest { .. }
        | TopicError::DecisionSuperseded { .. } => ApiError::Conflict(error.to_string()),
        TopicError::NotFound { .. } | TopicError::UnknownDecision { .. } => {
            ApiError::Missing(error.to_string())
        }
        TopicError::Unavailable(source) => {
            tracing::error!("topic store failed: {source}");
            ApiError::Unavailable("topic store unavailable".into())
        }
    }
}

pub(super) fn page_error(error: TopicPageError) -> ApiError {
    match error {
        TopicPageError::Topic(error) => self::error(error),
        TopicPageError::Memory(error) => ApiError::Store(error),
        TopicPageError::Wiki(_)
        | TopicPageError::Payload(_)
        | TopicPageError::ParentPlacement(_) => {
            tracing::error!("topic page lookup failed: {error}");
            ApiError::Internal
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

pub(super) fn suggestion(suggestion: &Suggestion) -> TopicSuggestion {
    TopicSuggestion {
        name: suggestion.name.as_str().to_owned(),
        topic_id: suggestion.topic.map(|topic| topic.to_string()),
    }
}

pub(super) fn session(placed: PlacedSession) -> TopicSession {
    TopicSession {
        placement: placement(placed.placement),
        last_active: placed.last_active,
    }
}

pub(super) fn decision(decision: Decision) -> TopicDecision {
    TopicDecision {
        id: decision.id.to_string(),
        topic_id: decision.topic.to_string(),
        text: decision.text.as_str().to_owned(),
        status: match decision.status {
            DecisionStatus::Active => wire::gateway::topics::DecisionStatus::Active,
            DecisionStatus::Superseded => wire::gateway::topics::DecisionStatus::Superseded,
        },
        superseded_by: decision.superseded_by.map(|id| id.to_string()),
        by: decision.by,
        session_id: decision.session,
        created_at: decision.created_at,
        updated_at: decision.updated_at,
    }
}
