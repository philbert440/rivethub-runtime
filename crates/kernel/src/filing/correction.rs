use chrono::DateTime;
use chrono::Utc;

use super::Target;
use crate::topics::PlacementSource;
use crate::topics::TopicId;
use crate::topics::TopicName;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Correction {
    pub target: Target,
    pub from: TopicId,
    pub to: Option<TopicId>,
    pub at: DateTime<Utc>,
}

impl Correction {
    #[must_use]
    pub fn of(
        target: Target,
        filed: Option<(PlacementSource, TopicId)>,
        to: Option<TopicId>,
        at: DateTime<Utc>,
    ) -> Option<Self> {
        match filed {
            Some((PlacementSource::Categorizer, from)) if to != Some(from) => Some(Self {
                target,
                from,
                to,
                at,
            }),
            Some(_) | None => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorrectionExample {
    pub title: Option<String>,
    pub from: TopicName,
    pub to: Option<TopicName>,
}
