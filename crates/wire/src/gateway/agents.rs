use crate::harness::HarnessId;
use crate::session_id::SessionId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_with::serde_as;

#[serde_as]
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPreset {
    pub id: String,
    pub name: String,
    pub color: String,
    pub harness_id: Option<HarnessId>,
    pub model: String,
    pub effort: String,
    pub system_prompt: String,
    pub node: Option<String>,
    pub directory: Option<String>,
    pub shared_link: Option<bool>,
    pub node_base_url: Option<String>,
    pub sort_order: Option<u32>,
    #[serde_as(as = "serde_with::TimestampMilliSeconds")]
    pub created_at: DateTime<Utc>,
    #[serde_as(as = "serde_with::TimestampMilliSeconds")]
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentsFile {
    pub agents: Vec<AgentPreset>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPresetCreateRequest {
    pub name: String,
    pub color: Option<String>,
    pub harness_id: Option<HarnessId>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub system_prompt: Option<String>,
    pub node: Option<String>,
    pub directory: Option<String>,
    pub shared_link: Option<bool>,
    pub node_base_url: Option<String>,
    pub sort_order: Option<u32>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPresetUpdate {
    pub name: Option<String>,
    pub color: Option<String>,
    #[serde(default, with = "::serde_with::rust::double_option")]
    pub harness_id: Option<Option<HarnessId>>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub system_prompt: Option<String>,
    pub node: Option<String>,
    pub directory: Option<String>,
    pub shared_link: Option<bool>,
    pub node_base_url: Option<String>,
    #[serde(default, with = "::serde_with::rust::double_option")]
    pub sort_order: Option<Option<u32>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentStoreBackend {
    Postgres,
    File,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentListResponse {
    pub agents: Vec<AgentPreset>,
    pub node: String,
    pub directory_root: Option<String>,
    pub shared_dir: Option<String>,
    pub backend: Option<AgentStoreBackend>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentResponse {
    pub agent: AgentPreset,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessDescriptor {
    pub installed: Option<bool>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DelegatedSessionLink {
    pub task_id: String,
    pub parent_task_id: Option<String>,
    pub spawned_session_id: SessionId,
    pub parent_session_id: SessionId,
    pub agent_name: Option<String>,
    pub model: Option<String>,
    pub owner: Option<String>,
    pub harness_id: Option<HarnessId>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserTurnAttachment {
    pub mime: String,
    pub path_or_uri: String,
    pub name: Option<String>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserTurn {
    pub delivery_id: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub text: String,
    pub attachments: Option<Vec<UserTurnAttachment>>,
    pub system_prompt: Option<String>,
    pub bypass_dialog_gate: Option<bool>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendUserTurnResult {
    pub dismissed_dialog: Option<bool>,
}
