use agent_client_protocol::schema::v1 as acp;
use serde::Deserialize;
use serde::Serialize;
use serde::de::IntoDeserializer as _;
use wire::harness::SteerOutcome;

use crate::error::AcpError;

pub(crate) const METHOD: &str = "_session/steering";

#[derive(Debug, Clone, Serialize, Deserialize, agent_client_protocol::JsonRpcRequest)]
#[request(method = "_session/steering", response = SteeringResponse)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SteeringRequest {
    session_id: acp::SessionId,
    prompt: Vec<acp::ContentBlock>,
    #[serde(rename = "_meta")]
    meta: SteeringMeta,
}

impl SteeringRequest {
    pub(crate) fn new(session_id: acp::SessionId, prompt: Vec<acp::ContentBlock>) -> Self {
        Self {
            session_id,
            prompt,
            meta: SteeringMeta {
                steering: IdleChoice {
                    idle_behavior: IdleBehavior::PromptRequired,
                },
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SteeringMeta {
    steering: IdleChoice,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdleChoice {
    idle_behavior: IdleBehavior,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum IdleBehavior {
    PromptRequired,
}

#[derive(Debug, Clone, Serialize, Deserialize, agent_client_protocol::JsonRpcResponse)]
pub(crate) struct SteeringResponse {
    outcome: AgentOutcome,
    reason: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum AgentOutcome {
    Injected,
    StartedNewTurn,
    PromptRequired,
    Failed,
}

impl SteeringResponse {
    pub(crate) fn outcome(self) -> Result<SteerOutcome, AcpError> {
        match self.outcome {
            AgentOutcome::Injected => Ok(SteerOutcome::Steered),
            AgentOutcome::StartedNewTurn => Ok(SteerOutcome::StartedNewTurn),
            AgentOutcome::PromptRequired => Ok(SteerOutcome::PromptRequired),
            AgentOutcome::Failed => Err(AcpError::SteerFailed {
                reason: self.reason,
            }),
        }
    }
}

#[derive(Debug, Deserialize)]
struct AgentMeta {
    steering: Option<Advertised>,
}

#[derive(Debug, Deserialize)]
struct Advertised {
    supported: bool,
}

pub(crate) fn advertised(meta: Option<&acp::Meta>) -> Result<bool, AcpError> {
    let Some(meta) = meta else {
        return Ok(false);
    };
    let meta = AgentMeta::deserialize(meta.into_deserializer())
        .map_err(|source| AcpError::SteeringMeta { source })?;
    Ok(meta.steering.is_some_and(|steering| steering.supported))
}
