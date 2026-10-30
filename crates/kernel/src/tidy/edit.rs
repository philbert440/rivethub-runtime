use std::str::FromStr;

use uuid::Uuid;
use wire::gateway::tidy::TidyChange as WireChange;

use super::Fingerprint;
use super::TidyError;
use crate::filing::Target;
use crate::topics::Slug;
use crate::topics::TopicId;
use crate::topics::TopicName;

const ROOT: &str = "root";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeEdit {
    Merge {
        source: TopicId,
        into: TopicId,
    },
    Move {
        topic: TopicId,
        parent: Option<TopicId>,
    },
    Rename {
        topic: TopicId,
        name: TopicName,
    },
    Close {
        topic: TopicId,
    },
    Delete {
        topic: TopicId,
    },
    CreateTopic {
        topic: TopicId,
        name: TopicName,
        parent: Option<TopicId>,
        members: Vec<Target>,
    },
    AddType {
        topic: TopicId,
        name: TopicName,
        parent: Option<TopicId>,
    },
}

impl TreeEdit {
    pub fn fingerprint(&self) -> Result<Fingerprint, TidyError> {
        Ok(match self {
            Self::Merge { source, into } => {
                let (low, high) = if source < into {
                    (source, into)
                } else {
                    (into, source)
                };
                Fingerprint::of(&["merge", &low.to_string(), &high.to_string()])
            }
            Self::Move { topic, parent } => Fingerprint::of(&[
                "move",
                &topic.to_string(),
                &parent.map_or_else(|| ROOT.to_owned(), |parent| parent.to_string()),
            ]),
            Self::Rename { topic, name } => {
                Fingerprint::of(&["rename", &topic.to_string(), slug(name)?.as_str()])
            }
            Self::Close { topic } => Fingerprint::of(&["close", &topic.to_string()]),
            Self::Delete { topic } => Fingerprint::of(&["delete", &topic.to_string()]),
            Self::CreateTopic { name, .. } => Fingerprint::of(&["create", slug(name)?.as_str()]),
            Self::AddType { name, .. } => Fingerprint::of(&["type", slug(name)?.as_str()]),
        })
    }

    #[must_use]
    pub fn to_wire(&self) -> WireChange {
        let id = |topic: &TopicId| topic.to_string();
        let parent = |parent: &Option<TopicId>| parent.as_ref().map(id);
        match self {
            Self::Merge { source, into } => WireChange::Merge {
                source: id(source),
                into: id(into),
            },
            Self::Move { topic, parent: to } => WireChange::Move {
                topic: id(topic),
                parent: parent(to),
            },
            Self::Rename { topic, name } => WireChange::Rename {
                topic: id(topic),
                name: name.as_str().to_owned(),
            },
            Self::Close { topic } => WireChange::Close { topic: id(topic) },
            Self::Delete { topic } => WireChange::Delete { topic: id(topic) },
            Self::CreateTopic {
                topic,
                name,
                parent: under,
                members,
            } => WireChange::CreateTopic {
                topic: id(topic),
                name: name.as_str().to_owned(),
                parent: parent(under),
                sessions: members
                    .iter()
                    .filter_map(Target::session)
                    .cloned()
                    .collect(),
                conversations: members
                    .iter()
                    .filter_map(Target::conversation)
                    .map(|conversation| conversation.to_string())
                    .collect(),
            },
            Self::AddType {
                topic,
                name,
                parent: under,
            } => WireChange::AddType {
                topic: id(topic),
                name: name.as_str().to_owned(),
                parent: parent(under),
            },
        }
    }

    pub fn from_wire(change: WireChange) -> Result<Self, TidyError> {
        Ok(match change {
            WireChange::Merge { source, into } => Self::Merge {
                source: topic(&source)?,
                into: topic(&into)?,
            },
            WireChange::Move {
                topic: moved,
                parent,
            } => Self::Move {
                topic: topic(&moved)?,
                parent: parent.as_deref().map(topic).transpose()?,
            },
            WireChange::Rename { topic: id, name } => Self::Rename {
                topic: topic(&id)?,
                name: TopicName::new(&name)?,
            },
            WireChange::Close { topic: id } => Self::Close { topic: topic(&id)? },
            WireChange::Delete { topic: id } => Self::Delete { topic: topic(&id)? },
            WireChange::CreateTopic {
                topic: id,
                name,
                parent,
                sessions,
                conversations,
            } => Self::CreateTopic {
                topic: topic(&id)?,
                name: TopicName::new(&name)?,
                parent: parent.as_deref().map(topic).transpose()?,
                members: sessions
                    .into_iter()
                    .map(Target::Session)
                    .map(Ok)
                    .chain(conversations.iter().map(|text| {
                        Uuid::parse_str(text)
                            .map(Target::Conversation)
                            .map_err(|_| invalid(text))
                    }))
                    .collect::<Result<_, TidyError>>()?,
            },
            WireChange::AddType {
                topic: id,
                name,
                parent,
            } => Self::AddType {
                topic: topic(&id)?,
                name: TopicName::new(&name)?,
                parent: parent.as_deref().map(topic).transpose()?,
            },
        })
    }
}

fn slug(name: &TopicName) -> Result<Slug, TidyError> {
    Ok(Slug::fold(name.as_str())?)
}

fn topic(text: &str) -> Result<TopicId, TidyError> {
    TopicId::from_str(text).map_err(|_| invalid(text))
}

fn invalid(text: &str) -> TidyError {
    TidyError::Invalid {
        what: text.to_owned(),
    }
}
