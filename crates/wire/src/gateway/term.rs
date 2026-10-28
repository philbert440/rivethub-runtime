use crate::events::ProtocolVersion;
use crate::session_id::SessionId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_with::serde_as;

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TermSpawnRequest {
    pub command: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub session: Option<String>,
    pub resume: Option<String>,
    pub agent_id: Option<String>,
    pub force: Option<bool>,
    pub cols: Option<u16>,
    pub rows: Option<u16>,
    pub topic_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TermAttachInfo {
    pub socket: String,
    pub session: String,
    pub host: String,
    pub ssh_user: String,
    pub local: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Mux {
    Tmux,
    Herdr,
}

#[serde_as]
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TermSpawnResponse {
    pub harness_session_id: Option<SessionId>,
    pub id: String,
    #[serde(rename = "denSession")]
    pub session: String,
    pub command: String,
    pub cwd: Option<String>,
    pub pid: u32,
    #[serde_as(as = "serde_with::TimestampMilliSeconds")]
    pub created_at: DateTime<Utc>,
    pub mux: Option<Mux>,
    pub reattached: Option<bool>,
    pub attach: Option<TermAttachInfo>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum PtyState {
    Running,
    Exited,
}

#[serde_as]
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PtyInfo {
    pub id: String,
    #[serde(rename = "denSession")]
    pub session: String,
    pub command: String,
    pub state: PtyState,
    pub pid: Option<u32>,
    pub attached: u64,
    pub exit_code: Option<i32>,
    #[serde_as(as = "serde_with::TimestampMilliSeconds")]
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    #[serde_as(as = "Option<serde_with::TimestampMilliSeconds>")]
    pub last_output_ts: Option<DateTime<Utc>>,
    pub cols: Option<u16>,
    pub rows: Option<u16>,
    pub mux: Option<Mux>,
    pub persisted: Option<bool>,
    pub reattached: Option<bool>,
    pub attach: Option<TermAttachInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PtyList {
    pub ptys: Vec<PtyInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TermCommand {
    pub id: String,
    pub label: String,
    #[serde(rename = "room")]
    pub session: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TermConfig {
    pub enabled: bool,
    pub default: String,
    pub max_ptys: u64,
    pub active: u64,
    pub commands: Vec<TermCommand>,
}

impl TermConfig {
    #[must_use]
    pub fn disabled(max_ptys: u64) -> Self {
        Self {
            enabled: false,
            default: super::roster::BUILTIN_DEFAULT_ID.to_owned(),
            max_ptys,
            active: 0,
            commands: super::roster::builtin_commands()
                .iter()
                .map(|command| TermCommand {
                    id: command.id.to_owned(),
                    label: command.label.to_owned(),
                    session: command.session,
                })
                .collect(),
        }
    }
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TermInjectRequest {
    pub session: String,
    pub text: String,
    pub submit: Option<bool>,
    pub interrupt: Option<bool>,
    pub bypass_dialog_gate: Option<bool>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TermInjectResponse {
    pub ok: bool,
    pub pty_id: String,
    pub dismissed_dialog: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum HelloState {
    Running,
    Exited,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DeviceOwner {
    pub device: Option<String>,
    #[serde(rename = "self")]
    pub self_owner: bool,
}

#[serde_as]
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum TermServerFrame {
    #[serde(rename_all = "camelCase")]
    Hello {
        v: ProtocolVersion,
        id: String,
        #[serde(rename = "denSession")]
        session: String,
        command: String,
        cols: u16,
        rows: u16,
        state: HelloState,
        exit_code: Option<i32>,
        mux: Option<Mux>,
        owner: Option<DeviceOwner>,
    },
    Exit {
        code: Option<i32>,
        signal: Option<i32>,
    },
    #[serde(rename_all = "camelCase")]
    Owner {
        device: Option<String>,
        #[serde(rename = "self")]
        self_owner: bool,
        #[serde(default)]
        #[serde_as(as = "Option<serde_with::TimestampMilliSeconds>")]
        since: Option<DateTime<Utc>>,
    },
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum TermClientFrame {
    #[serde(rename_all = "camelCase")]
    Resize {
        cols: u16,
        rows: u16,
    },
    #[serde(rename_all = "camelCase")]
    Claim {
        cols: Option<u16>,
        rows: Option<u16>,
    },
    Kill,
}
