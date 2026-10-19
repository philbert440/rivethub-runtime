use serde::Deserialize;
use wire::gateway::memory::SummaryKind;
use wire::message::Role;

pub(crate) const CONVERSATION_SQL: &str = "INSERT INTO ros_conversations \
     (id, session_key, agent, channel, created_at, updated_at) \
     VALUES (?1, 'eval-' || ?1, ?2, 'cli', ?3, ?3)";
pub(crate) const MESSAGE_SQL: &str = "INSERT INTO ros_messages \
     (id, conversation_id, role, content, agent, channel, created_at) \
     VALUES (?1, ?2, ?3, ?4, (SELECT agent FROM ros_conversations WHERE id = ?2), 'cli', ?5)";
pub(crate) const SUMMARY_SQL: &str = "INSERT INTO ros_summaries \
     (id, conversation_id, content, kind, created_at) VALUES (?1, ?2, ?3, ?4, ?5)";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Corpus {
    pub(crate) conversations: Vec<Conversation>,
    pub(crate) messages: Vec<Message>,
    pub(crate) summaries: Vec<Summary>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Conversation {
    pub(crate) id: String,
    pub(crate) agent: String,
    pub(crate) created_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Message {
    pub(crate) id: String,
    pub(crate) conversation: String,
    pub(crate) role: Role,
    pub(crate) content: String,
    pub(crate) created_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Summary {
    pub(crate) id: String,
    pub(crate) conversation: Option<String>,
    pub(crate) content: String,
    pub(crate) kind: SummaryKind,
    pub(crate) created_at: String,
}
