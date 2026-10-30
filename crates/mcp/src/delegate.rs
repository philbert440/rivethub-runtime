use std::time::Duration;

use kernel::tasks::DelegateRequest;
use kernel::tasks::Delegation;
use kernel::tasks::DelegationError;
use rmcp::model::CallToolResult;
use schemars::JsonSchema;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use crate::error::Error;
use crate::{render_delegate, text};

const DEFAULT_TIMEOUT_MS: i64 = 1_200_000;
const MAX_TIMEOUT_MS: i64 = 1_800_000;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(!deny_unknown_fields)]
pub(crate) struct DelegateTask {
    #[schemars(
        description = "RivetHub agent name or id, or a runtime agent id (agent@node pins the node) — call list_agents first",
        length(min = 1)
    )]
    to_agent: String,
    #[schemars(description = "What the delegate should do", length(min = 1))]
    task: String,
    #[schemars(
        description = "Extra context lines included with the task",
        transform = crate::catalog::optional_non_null
    )]
    context: Option<Vec<String>>,
    #[schemars(
        description = "Acceptance criteria the delegate should meet, one per entry",
        transform = crate::catalog::optional_non_null
    )]
    criteria: Option<Vec<String>>,
    #[schemars(
        description = "Optional model override for this delegation",
        transform = crate::catalog::optional_non_null
    )]
    model: Option<String>,
    #[schemars(
        description = "How long to wait, in milliseconds (default 20 minutes, max 30)",
        extend("exclusiveMinimum" = 0, "maximum" = 1_800_000),
        transform = crate::catalog::optional_non_null
    )]
    timeout_ms: Option<i64>,
}

impl crate::catalog::ToolInput for DelegateTask {
    const NAME: &'static str = "delegate_task";
    const DESCRIPTION: &'static str = "Delegate work to a RivetHub agent (preset name or id) or a runtime agent id. Call list_agents first. When no node is given, a preset name or id wins when it also matches a runtime agent id, unless that preset has no harness configured — then the runtime agent runs. agent@node is resolved as a preset only if a preset has that exact name. The delegate runs as its own harness session, started by this one: a preset in its agent directory on its node, a runtime agent on the newest online node that hosts it (agent@node picks the node). The session stays open afterwards, filed under this session's topic, so it can be opened and continued. Waits until the task finishes or the timeout elapses (default 20 minutes, max 30). Set the client tool-call timeout above that wait — Codex tool_timeout_sec (its default of 60s aborts the call) and Claude Code MCP timeout. An aborted or timed-out call leaves the task running: the reply names its id and status, and its result comes back to this session as a new message when it finishes.";

    fn annotations() -> Option<rmcp::model::ToolAnnotations> {
        None
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(!deny_unknown_fields, transform = crate::catalog::object_properties)]
pub(crate) struct ListAgents {
    #[serde(skip)]
    #[schemars(skip)]
    _marker: (),
}

impl crate::catalog::ToolInput for ListAgents {
    const NAME: &'static str = "list_agents";
    const DESCRIPTION: &'static str = "List RivetHub agents (presets) and runtime agents on online mesh nodes. Pass a preset name or id, or a runtime agent id, as delegate_task to_agent. A preset name or id wins when it also matches a runtime agent id, unless that preset has no harness configured — then the runtime agent runs.";

    fn annotations() -> Option<rmcp::model::ToolAnnotations> {
        Some(rmcp::model::ToolAnnotations::from_raw(
            None,
            Some(true),
            None,
            Some(true),
            None,
        ))
    }
}

pub(crate) async fn task(
    port: &dyn Delegation,
    arguments: DelegateTask,
    cancel: CancellationToken,
) -> Result<CallToolResult, Error> {
    required_text(&arguments.to_agent, "to_agent")?;
    required_text(&arguments.task, "task")?;
    let request = DelegateRequest {
        target: arguments.to_agent,
        prompt: prompt(&arguments.task, arguments.context),
        criteria: arguments.criteria.into_iter().flatten().collect(),
        model: arguments.model,
        timeout: timeout_ms(arguments.timeout_ms)?,
    };
    let started = std::time::Instant::now();
    match port.delegate(request, cancel).await {
        Ok(outcome) => Ok(text(render_delegate::task(&outcome, started.elapsed()))),
        Err(error) => Err(map_delegation(error)),
    }
}

pub(crate) async fn agents(
    port: &dyn Delegation,
    _arguments: ListAgents,
) -> Result<CallToolResult, Error> {
    match port.agents().await {
        Ok(directory) => Ok(text(render_delegate::agents(&directory)?)),
        Err(error) => Err(map_delegation(error)),
    }
}

fn required_text(value: &str, field: &str) -> Result<(), Error> {
    if value.is_empty() {
        Err(Error::Invalid(format!("{field} is empty")))
    } else {
        Ok(())
    }
}

fn prompt(task: &str, context: Option<Vec<String>>) -> String {
    match context {
        None => task.to_owned(),
        Some(lines) => {
            let mut prompt = task.to_owned();
            for line in lines {
                prompt.push('\n');
                prompt.push_str(&line);
            }
            prompt
        }
    }
}

fn timeout_ms(raw: Option<i64>) -> Result<Duration, Error> {
    let millis = match raw {
        Some(millis) => millis,
        None => DEFAULT_TIMEOUT_MS,
    };
    if !(1..=MAX_TIMEOUT_MS).contains(&millis) {
        return Err(Error::Invalid(format!(
            "timeout_ms {millis} is out of range"
        )));
    }
    match u64::try_from(millis) {
        Ok(millis) => Ok(Duration::from_millis(millis)),
        Err(error) => Err(Error::Invalid(error.to_string())),
    }
}

fn map_delegation(error: DelegationError) -> Error {
    match error {
        DelegationError::Cancelled => Error::Cancelled,
        DelegationError::Rejected(message) => Error::Invalid(message),
        DelegationError::Failed(error) => Error::Invalid(error.to_string()),
    }
}
