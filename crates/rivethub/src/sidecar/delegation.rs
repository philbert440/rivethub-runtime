use std::time::Duration;

use async_trait::async_trait;
use config::{Config, Environment};
use kernel::tasks::AgentDirectory;
use kernel::tasks::AgentNode;
use kernel::tasks::DelegateOutcome;
use kernel::tasks::DelegateRequest;
use kernel::tasks::Delegation;
use kernel::tasks::DelegationError;
use kernel::tasks::PresetAgent;
use kernel::tasks::RuntimeAgent;
use kernel::tasks::TaskStatus;
use reqwest::StatusCode;
use serde::de::DeserializeOwned;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use wire::gateway::GatewayError;
use wire::gateway::catalog::CatalogAgent;
use wire::gateway::catalog::CatalogAgentsResponse;
use wire::task::AcceptanceCriterion;
use wire::task::CriterionKind;
use wire::task::TaskCreateRequest;
use wire::task::TaskResponse;
use wire::task::TaskStatus as WireStatus;
use wire::task::TaskWaitTimeoutResponse;
use wire::task::TaskWire;

use super::dial::{DialError, GatewayEndpoint};

const POLL: Duration = Duration::from_secs(120);
const SESSION_KEY: &str = "RIVETHUB_SESSION_KEY";

#[derive(Debug)]
pub(crate) struct GatewayDelegation {
    http: reqwest::Client,
    base: url::Url,
    parent: Option<String>,
}

impl GatewayDelegation {
    pub(crate) async fn open(
        config: &Config,
        env: &Environment,
        parent: Option<String>,
    ) -> Result<Self, DialError> {
        let node = crate::identity::resolve(config, env)
            .map_err(|error| DialError::Io(std::io::Error::other(error.to_string())))?
            .name;
        let endpoint = super::dial::endpoint(config, env, &node).await?;
        Self::at(endpoint, parent)
    }

    pub(crate) async fn local(
        config: &Config,
        env: &Environment,
        url: url::Url,
    ) -> Result<Self, DialError> {
        if url.scheme() == "http" {
            return Self::at(GatewayEndpoint { url, trust: None }, None);
        }
        let mut endpoint = Self::open(config, env, None).await?;
        endpoint.base = url;
        Ok(endpoint)
    }

    pub(crate) fn session_parent(env: &Environment) -> Option<String> {
        env.text(SESSION_KEY)
            .filter(|session| !session.is_empty())
            .map(str::to_owned)
    }

    fn at(endpoint: GatewayEndpoint, parent: Option<String>) -> Result<Self, DialError> {
        let mut builder = reqwest::Client::builder().no_proxy();
        if let Some(trust) = endpoint.trust {
            builder = builder.https_only(true).tls_certs_only([trust]);
        }
        let http = builder
            .build()
            .map_err(|error| DialError::Io(std::io::Error::other(error)))?;
        Ok(Self {
            http,
            base: endpoint.url,
            parent,
        })
    }

    fn url(&self, path: &str) -> Result<url::Url, DelegationError> {
        self.base.join(path).map_err(|error| failed(&error))
    }

    async fn submit(&self, request: &DelegateRequest) -> Result<TaskWire, DelegationError> {
        let body = TaskCreateRequest {
            goal: Some(request.prompt.clone()),
            agent_id: Some(request.target.clone()),
            acceptance_criteria: Some(criteria(&request.criteria)),
            parent_session_id: self.parent.clone(),
            model: request.model.clone(),
            requested_by: Some("delegate_task".to_owned()),
            ..TaskCreateRequest::default()
        };
        let response = self
            .http
            .post(self.url("api/tasks")?)
            .json(&body)
            .send()
            .await
            .map_err(|error| failed(&error))?;
        Ok(answer::<TaskResponse>(response).await?.task)
    }

    async fn wait(&self, id: &str, window: Duration) -> Result<(TaskWire, bool), DelegationError> {
        let millis = u64::try_from(window.as_millis()).map_err(|error| failed(&error))?;
        let response = self
            .http
            .get(self.url(&format!("api/tasks/{id}/wait?timeoutMs={}", millis.max(1)))?)
            .timeout(window + Duration::from_secs(30))
            .send()
            .await
            .map_err(|error| failed(&error))?;
        if response.status() == StatusCode::GATEWAY_TIMEOUT {
            let left: TaskWaitTimeoutResponse = decode(response).await?;
            let task = left
                .task
                .ok_or_else(|| DelegationError::Rejected("the wait answered no task".into()))?;
            return Ok((task, false));
        }
        Ok((answer::<TaskResponse>(response).await?.task, true))
    }
}

#[async_trait]
impl Delegation for GatewayDelegation {
    async fn agents(&self) -> Result<AgentDirectory, DelegationError> {
        let response = self
            .http
            .get(self.url("api/catalog/agents")?)
            .send()
            .await
            .map_err(|error| failed(&error))?;
        let listed: CatalogAgentsResponse = answer(response).await?;
        Ok(directory(listed.agents))
    }

    async fn delegate(
        &self,
        request: DelegateRequest,
        cancel: CancellationToken,
    ) -> Result<DelegateOutcome, DelegationError> {
        let deadline = tokio::time::Instant::now() + request.timeout;
        let mut task = tokio::select! {
            () = cancel.cancelled() => return Err(DelegationError::Cancelled),
            task = self.submit(&request) => task?,
        };
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            if left.is_zero() {
                return outcome(&task, true);
            }
            let (current, done) = tokio::select! {
                () = cancel.cancelled() => return Err(DelegationError::Cancelled),
                waited = self.wait(&task.id, left.min(POLL)) => waited?,
            };
            task = current;
            if done {
                return outcome(&task, false);
            }
        }
    }
}

fn criteria(items: &[String]) -> Vec<AcceptanceCriterion> {
    items
        .iter()
        .zip(1_u32..)
        .map(|(description, index)| AcceptanceCriterion {
            id: index.to_string(),
            description: description.clone(),
            kind: CriterionKind::Manual,
            check: None,
        })
        .collect()
}

fn outcome(task: &TaskWire, timed_out: bool) -> Result<DelegateOutcome, DelegationError> {
    Ok(DelegateOutcome {
        id: Uuid::parse_str(&task.id).map_err(|error| failed(&error))?,
        status: status_in(task.status),
        summary: task.result.as_ref().map(|result| result.summary.clone()),
        error: task.error.clone(),
        child: task.child_session_id.clone(),
        node: task.node.clone(),
        tokens: task.usage.and_then(|usage| usage.total_tokens),
        timed_out,
    })
}

const fn status_in(status: WireStatus) -> TaskStatus {
    match status {
        WireStatus::Queued => TaskStatus::Queued,
        WireStatus::Running => TaskStatus::Running,
        WireStatus::AwaitingInput => TaskStatus::AwaitingInput,
        WireStatus::Completed => TaskStatus::Completed,
        WireStatus::Failed => TaskStatus::Failed,
        WireStatus::Killed => TaskStatus::Killed,
        WireStatus::Timeout => TaskStatus::Timeout,
    }
}

fn directory(agents: Vec<CatalogAgent>) -> AgentDirectory {
    let mut presets = Vec::new();
    let mut runtime: Vec<RuntimeAgent> = Vec::new();
    for agent in agents {
        match agent {
            CatalogAgent::Preset(preset) => presets.push(PresetAgent {
                id: preset.id,
                name: preset.name,
                node: Some(preset.node),
                harness: preset.harness_id.map(gateway::tasks::harness_in),
                directory: preset.directory,
                local: preset.local,
            }),
            CatalogAgent::Node(node) => {
                let host = AgentNode {
                    id: node.node,
                    last_seen: None,
                };
                match runtime.iter_mut().find(|agent| agent.id == node.id) {
                    Some(agent) => agent.nodes.push(host),
                    None => runtime.push(RuntimeAgent {
                        id: node.id,
                        nodes: vec![host],
                    }),
                }
            }
        }
    }
    AgentDirectory { presets, runtime }
}

async fn answer<T: DeserializeOwned>(response: reqwest::Response) -> Result<T, DelegationError> {
    let status = response.status();
    if status.is_success() {
        return decode(response).await;
    }
    let refused: GatewayError = decode(response).await?;
    if status.is_client_error() {
        Err(DelegationError::Rejected(refused.error))
    } else {
        Err(DelegationError::Failed(refused.error.into()))
    }
}

async fn decode<T: DeserializeOwned>(response: reqwest::Response) -> Result<T, DelegationError> {
    let bytes = response.bytes().await.map_err(|error| failed(&error))?;
    serde_json::from_slice(&bytes).map_err(|error| failed(&error))
}

fn failed(error: &dyn std::fmt::Display) -> DelegationError {
    DelegationError::Failed(error.to_string().into())
}
