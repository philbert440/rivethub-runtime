use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TermSpawnRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub force: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cols: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rows: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TermAttachInfo {
    pub socket: String,
    pub session: String,
    pub host: String,
    pub ssh_user: String,
    pub local: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Mux {
    Tmux,
    Herdr,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TermSpawnResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness_session_id: Option<String>,
    pub id: String,
    #[serde(rename = "denSession")]
    pub session: String,
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    pub pid: u32,
    pub created_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mux: Option<Mux>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reattached: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attach: Option<TermAttachInfo>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PtyState {
    Running,
    Exited,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PtyInfo {
    pub id: String,
    #[serde(rename = "denSession")]
    pub session: String,
    pub command: String,
    pub state: PtyState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    pub attached: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub created_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_output_ts: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cols: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rows: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mux: Option<Mux>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub persisted: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reattached: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attach: Option<TermAttachInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PtyList {
    pub ptys: Vec<PtyInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TermCommand {
    pub id: String,
    pub label: String,
    pub room: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TermConfig {
    pub enabled: bool,
    pub default: String,
    pub max_ptys: u64,
    pub active: u64,
    pub commands: Vec<TermCommand>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TermInjectRequest {
    pub session: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submit: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interrupt: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bypass_dialog_gate: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TermInjectResponse {
    pub ok: bool,
    pub pty_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dismissed_dialog: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HelloState {
    Running,
    Exited,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceOwner {
    pub device: Option<String>,
    #[serde(rename = "self")]
    pub self_owner: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum TermServerFrame {
    #[serde(rename_all = "camelCase")]
    Hello {
        v: i64,
        id: String,
        #[serde(rename = "denSession")]
        session: String,
        command: String,
        cols: u16,
        rows: u16,
        state: HelloState,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exit_code: Option<i32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mux: Option<Mux>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        owner: Option<DeviceOwner>,
    },
    Exit {
        code: Option<i32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signal: Option<i32>,
    },
    #[serde(rename_all = "camelCase")]
    Owner {
        device: Option<String>,
        #[serde(rename = "self")]
        self_owner: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        since: Option<i64>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum TermClientFrame {
    #[serde(rename_all = "camelCase")]
    Resize {
        cols: u16,
        rows: u16,
    },
    #[serde(rename_all = "camelCase")]
    Claim {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cols: Option<u16>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rows: Option<u16>,
    },
    Kill,
}
