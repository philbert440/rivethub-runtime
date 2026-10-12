use crate::{Extra, JsonObject, harness::SessionHarness};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub enum ProtocolVersion {
    V1,
}

impl TryFrom<u8> for ProtocolVersion {
    type Error = String;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        if value == 1 {
            Ok(Self::V1)
        } else {
            Err("protocol version must be 1".to_string())
        }
    }
}

impl From<ProtocolVersion> for u8 {
    fn from(_value: ProtocolVersion) -> Self {
        1
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentEvent {
    pub v: ProtocolVersion,
    pub session: String,
    #[serde(flatten)]
    pub kind: AgentEventKind,
    #[serde(flatten, default)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum AgentEventKind {
    #[serde(rename = "session.start", rename_all = "camelCase")]
    SessionStart {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        harness: Option<SessionHarness>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ts: Option<i64>,
    },
    #[serde(rename = "session.end")]
    SessionEnd {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ts: Option<i64>,
    },
    #[serde(rename = "message.user")]
    MessageUser {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ts: Option<i64>,
    },
    #[serde(rename = "message.agent", rename_all = "camelCase")]
    MessageAgent {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_ms: Option<i64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<AgentUsage>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ts: Option<i64>,
    },
    #[serde(rename = "thinking.delta")]
    ThinkingDelta {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ts: Option<i64>,
    },
    #[serde(rename = "tool.start", rename_all = "camelCase")]
    ToolStart {
        tool: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        args: Option<JsonObject>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ts: Option<i64>,
    },
    #[serde(rename = "task.plan")]
    TaskPlan {
        tasks: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ts: Option<i64>,
    },
    #[serde(rename = "task.check")]
    TaskCheck {
        index: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ts: Option<i64>,
    },
    #[serde(rename = "activity")]
    Activity {
        activity: Activity,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ts: Option<i64>,
    },
    #[serde(rename = "turn.end")]
    TurnEnd {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ts: Option<i64>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Activity {
    Idle,
    Thinking,
    Writing,
    Speaking,
    EditingCode,
    Sleeping,
    Computer,
    Toolbox,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub cached_tokens: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ContextSource {
    Spawn,
    Default,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<SessionHarness>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_event_ts: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pty: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compact_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_source: Option<ContextSource>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionLogLine {
    pub who: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionView {
    pub title: String,
    pub activity: Activity,
    pub tool: Option<String>,
    pub tasks: Vec<String>,
    pub thought: String,
    pub last_message: String,
    pub log: Vec<SessionLogLine>,
    pub term: Vec<String>,
    pub ended: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionList {
    pub sessions: Vec<SessionSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StateResponse {
    pub session: String,
    pub state: SessionView,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename = "snapshot", rename_all = "camelCase")]
pub struct Snapshot {
    pub v: i64,
    pub sessions: Vec<SessionSummary>,
    pub rooms: BTreeMap<String, SessionView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename = "session.removed", rename_all = "camelCase")]
pub struct SessionRemoved {
    pub v: i64,
    pub session: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SessionBroadcast {
    Snapshot(Snapshot),
    Removed(SessionRemoved),
    Event(AgentEvent),
}
