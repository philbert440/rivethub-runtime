use crate::events::Activity;
use crate::task::TaskStatus;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeshOverview {
    pub updated_at: i64,
    pub nodes: Vec<MeshOverviewNode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeshOverviewNode {
    pub id: String,
    pub name: String,
    #[serde(rename = "denUrl")]
    pub url: String,
    pub online: bool,
    #[serde(default)]
    pub sessions: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest: Option<MeshLatest>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeshLatest {
    pub activity: Activity,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeCapabilities {
    pub subsystems: Subsystems,
    pub harnesses: Vec<HarnessCapability>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Subsystems {
    pub den: bool,
    pub devices: bool,
    pub mesh: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessCapability {
    pub harness_id: crate::harness::HarnessId,
    #[serde(flatten)]
    pub inspect: HarnessInspectCapability,
    #[serde(flatten)]
    pub drive: HarnessDriveCapability,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessInspectCapability {
    pub list: bool,
    pub read: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessDriveCapability {
    pub drive: bool,
    pub resume: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeshDevice {
    pub id: String,
    pub name: String,
    pub public_key: String,
    pub address: String,
    pub created_at: i64,
    #[serde(default)]
    pub enrolled_at: Option<i64>,
    #[serde(default)]
    pub last_handshake: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pg_role: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceListResponse {
    pub devices: Vec<MeshDevice>,
    pub pending: Vec<MeshDevice>,
    pub relay_configured: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceEnrollConfig {
    pub shared_host: String,
    pub shared_export: String,
    pub pg_url: String,
    pub embed_url: String,
    pub wg_endpoint: String,
    pub wg_peer_public_key: String,
    pub wg_address: String,
    pub wg_allowed_ips: String,
    pub home_subnet: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceEnrollRequest {
    pub token: String,
    pub public_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhonePairingCode {
    pub device_id: String,
    pub gateway: String,
    pub expires_at: i64,
    pub qr_text: String,
    pub reshown: i64,
    pub added_to_users: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PhonePairingState {
    Pending,
    Paired,
    Expired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhonePairingStatus {
    pub state: PhonePairingState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhonePairingAvailability {
    pub available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gateway: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhonePairingRequest {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Notification {
    #[serde(rename = "escalation")]
    Escalation {
        task_id: String,
        agent_id: String,
        summary: String,
        href: String,
        ts: i64,
    },
    #[serde(rename = "task.done", rename_all = "camelCase")]
    TaskDone {
        task_id: String,
        status: TaskStatus,
        ts: i64,
    },
    #[serde(rename = "workflow.gate", rename_all = "camelCase")]
    WorkflowGate {
        run_id: String,
        workflow_id: String,
        label: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prompt: Option<String>,
        href: String,
        ts: i64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AudioBackend {
    FifoShim,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AudioMicErrorCode {
    Busy,
    Disabled,
    BadHello,
    NoRuntime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum AudioMicServerFrame {
    #[serde(rename_all = "camelCase")]
    Ready {
        device: String,
        sample_rate: i64,
        format: AudioFormat,
        backend: AudioBackend,
    },
    #[serde(rename_all = "camelCase")]
    Error {
        code: AudioMicErrorCode,
        message: String,
    },
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AudioFormat {
    S16Le,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum AudioMicClientFrame {
    #[serde(rename_all = "camelCase")]
    Hello {
        v: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sample_rate: Option<i64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        channels: Option<i64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        format: Option<AudioFormat>,
    },
    Start,
    Stop,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioStatus {
    pub enabled: bool,
    pub backend: AudioBackend,
    pub device: String,
    pub sample_rate: i64,
    pub channels: i64,
    pub format: AudioFormat,
    pub fifo_path: String,
    pub armed: bool,
    #[serde(default)]
    pub publisher_id: Option<String>,
    #[serde(default)]
    pub armed_at: Option<i64>,
    pub bytes_written: u64,
    pub runtime_ready: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceSpeakRequest {
    pub input: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceTranscribeResponse {
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    pub name: String,
    pub r#type: FileEntryKind,
    pub size: u64,
    pub mtime: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FileEntryKind {
    Dir,
    File,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilesListResponse {
    pub path: String,
    pub root: String,
    pub entries: Vec<FileEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StagedUploadResponse {
    pub uri: String,
    pub name: String,
    pub mime: String,
    pub size: u64,
    pub expires_at: i64,
}
