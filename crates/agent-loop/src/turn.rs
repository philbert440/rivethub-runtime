use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use kernel::ToolContext;
use kernel::model::{ModelChunk, ModelProvider, ModelRequest};
use kernel::tool::Tool;
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use wire::hooks::{HookContext, HookContextBase, HookContextKind, HookUsage};
use wire::message::{Message, MessageContent, Role, ToolCall};
use wire::stream::LlmUsage;

use crate::Error;
use crate::hooks::HookPipeline;
use crate::stream::{Response, collect};

#[derive(Debug, Clone, Copy)]
pub struct TurnLimits {
    pub steps: u32,
    pub timeout: Duration,
    pub cleanup_timeout: Duration,
    pub response_bytes: usize,
}

#[derive(Debug)]
pub struct Turn {
    pub request: ModelRequest,
    pub context: HookContextBase,
    pub tool_context: ToolContext,
    pub authorized_tools: BTreeSet<String>,
    pub limits: TurnLimits,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    Completed,
    StepLimit,
}

#[derive(Debug, PartialEq, Eq)]
pub struct TurnResult {
    pub messages: Vec<Message>,
    pub response: String,
    pub tools_used: Vec<String>,
    pub iterations: u32,
    pub usage: Option<HookUsage>,
    pub stop: StopReason,
}

pub struct AgentLoop {
    provider_id: String,
    provider: Arc<dyn ModelProvider>,
    tools: BTreeMap<String, Arc<dyn Tool>>,
    hooks: Arc<HookPipeline>,
}

type ToolTasks = JoinSet<Result<(usize, (Message, Option<String>)), Error>>;

#[cfg_attr(
    dylint_lib = "rr_lints",
    expect(
        manualderive,
        reason = "kernel ModelProvider and Tool do not require Debug; this formatter reports only provider ID and catalog names"
    )
)]
impl std::fmt::Debug for AgentLoop {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AgentLoop")
            .field("provider_id", &self.provider_id)
            .field("tools", &self.tools.keys())
            .finish_non_exhaustive()
    }
}

impl AgentLoop {
    pub fn new(
        provider_id: String,
        provider: Arc<dyn ModelProvider>,
        tools: Vec<Arc<dyn Tool>>,
        hooks: Arc<HookPipeline>,
    ) -> Result<Self, Error> {
        let mut catalog = BTreeMap::new();
        for tool in tools {
            let name = tool.definition().name.into_owned();
            if catalog.insert(name.clone(), tool).is_some() {
                return Err(Error::DuplicateTool(name));
            }
        }
        Ok(Self {
            provider_id,
            provider,
            tools: catalog,
            hooks,
        })
    }

    pub async fn run(
        &self,
        mut turn: Turn,
        events: mpsc::Sender<ModelChunk>,
        cancel: CancellationToken,
    ) -> Result<TurnResult, Error> {
        let child = cancel.child_token();
        let _guard = child.clone().drop_guard();
        let mut state = TurnResult {
            messages: std::mem::take(&mut turn.request.messages),
            response: String::new(),
            tools_used: Vec::new(),
            iterations: 0,
            usage: None,
            stop: StopReason::StepLimit,
        };
        let mut tasks = ToolTasks::new();
        let mut outcome = tokio::select! {
            biased;
            () = child.cancelled() => Err(Error::Cancelled),
            result = tokio::time::timeout(turn.limits.timeout, self.advance(&turn, &mut state, &events, &mut tasks)) => {
                match result {
                    Ok(result) => result,
                    Err(_) => Err(Error::Timeout),
                }
            }
        };
        child.cancel();
        tasks.abort_all();
        while let Some(result) = tasks.join_next().await {
            match result {
                Ok(Ok(_)) => {}
                Ok(Err(error)) => outcome = Err(error),
                Err(error) if error.is_cancelled() => {}
                Err(error) => outcome = Err(error.into()),
            }
        }
        let finished = HookContextKind::TurnAfter {
            response: state.response.clone(),
            tools_used: state.tools_used.clone(),
            iterations: state.iterations,
            aborted: outcome.is_err() || state.stop == StopReason::StepLimit,
            usage: state.usage.clone(),
        };
        tokio::time::timeout(
            turn.limits.cleanup_timeout,
            self.emit(&turn.context, finished),
        )
        .await
        .map_err(|_| Error::Timeout)??;
        outcome?;
        Ok(state)
    }

    async fn emit(&self, base: &HookContextBase, kind: HookContextKind) -> Result<(), Error> {
        let result = self
            .hooks
            .run(HookContext {
                base: base.clone(),
                kind,
            })
            .await?;
        if result.aborted || result.skipped {
            return Err(Error::Refused);
        }
        Ok(())
    }

    async fn advance(
        &self,
        turn: &Turn,
        state: &mut TurnResult,
        events: &mpsc::Sender<ModelChunk>,
        tasks: &mut ToolTasks,
    ) -> Result<(), Error> {
        let user_message = state
            .messages
            .last()
            .filter(|message| message.role == Role::User)
            .ok_or(Error::MissingUserMessage)?
            .clone();
        self.emit(
            &turn.context,
            HookContextKind::TurnBefore {
                user_message,
                skip: None,
                skip_reason: None,
            },
        )
        .await?;
        for _ in 0..turn.limits.steps {
            state.iterations += 1;
            let response = self.response(turn, &state.messages, events).await?;
            add_usage(&mut state.usage, response.usage)?;
            if let MessageContent::Text(text) = &response.message.content {
                state.response.clone_from(text);
            }
            let calls = response.message.tool_calls.clone();
            state.messages.push(response.message);
            let Some(calls) = calls else {
                state.stop = StopReason::Completed;
                return Ok(());
            };
            self.call_tools(turn, state, calls, tasks).await?;
        }
        Ok(())
    }

    async fn response(
        &self,
        turn: &Turn,
        messages: &[Message],
        events: &mpsc::Sender<ModelChunk>,
    ) -> Result<Response, Error> {
        self.emit(
            &turn.context,
            HookContextKind::ProviderBefore {
                provider_id: self.provider_id.clone(),
                model: turn.request.model.clone(),
                messages: messages.to_vec(),
                skip: None,
            },
        )
        .await?;
        let started = Instant::now();
        let request = ModelRequest {
            model: turn.request.model.clone(),
            messages: messages.to_vec(),
            tools: self
                .tools
                .iter()
                .filter(|(name, _)| turn.authorized_tools.contains(*name))
                .map(|(_, tool)| tool.definition())
                .collect(),
            thinking: turn.request.thinking,
            max_tokens: turn.request.max_tokens,
        };
        let result = async {
            let receiver = self.provider.stream(request).await?;
            collect(receiver, events, turn.limits.response_bytes).await
        }
        .await;
        match &result {
            Ok(response) => {
                self.emit(
                    &turn.context,
                    HookContextKind::ProviderAfter {
                        provider_id: self.provider_id.clone(),
                        model: turn.request.model.clone(),
                        usage: response.usage.as_ref().map(hook_usage),
                        latency_ms: elapsed_ms(started)?,
                        has_tool_calls: response.message.tool_calls.is_some(),
                    },
                )
                .await?;
            }
            Err(error) => {
                self.emit(
                    &turn.context,
                    HookContextKind::ProviderError {
                        provider_id: self.provider_id.clone(),
                        model: turn.request.model.clone(),
                        error: error.to_string(),
                        status_code: None,
                    },
                )
                .await?;
            }
        }
        result
    }

    async fn call_tools(
        &self,
        turn: &Turn,
        state: &mut TurnResult,
        calls: Vec<ToolCall>,
        tasks: &mut ToolTasks,
    ) -> Result<(), Error> {
        for (index, call) in calls.into_iter().enumerate() {
            let tool = self.tools.get(&call.name).cloned();
            let authorized = turn.authorized_tools.contains(&call.name);
            let hooks = Arc::clone(&self.hooks);
            let base = turn.context.clone();
            let context = turn.tool_context.clone();
            tasks.spawn(async move {
                let completed = execute(tool, authorized, hooks, base, context, call).await?;
                Ok::<_, Error>((index, completed))
            });
        }
        let mut completed = BTreeMap::new();
        while let Some(result) = tasks.join_next().await {
            let (index, output) = result??;
            completed.insert(index, output);
        }
        for (message, used) in completed.into_values() {
            state.messages.push(message);
            if let Some(name) = used {
                state.tools_used.push(name);
            }
        }
        Ok(())
    }
}

async fn execute(
    tool: Option<Arc<dyn Tool>>,
    authorized: bool,
    hooks: Arc<HookPipeline>,
    base: HookContextBase,
    context: ToolContext,
    call: ToolCall,
) -> Result<(Message, Option<String>), Error> {
    let blocked = !authorized || tool.is_none();
    let checked = hooks
        .run(HookContext {
            base: base.clone(),
            kind: HookContextKind::ToolBefore {
                tool_name: call.name.clone(),
                args: call.arguments.clone(),
                blocked: Some(blocked),
                block_reason: if blocked {
                    Some("tool is not authorized or available".to_owned())
                } else {
                    None
                },
            },
        })
        .await?;
    let started = Instant::now();
    let mut used = None;
    let result = if checked.aborted || checked.skipped {
        tool_failure("tool refused by hook pipeline".to_owned())
    } else {
        let tool = tool.ok_or(Error::Refused)?;
        used = Some(call.name.clone());
        match tool
            .call(Some(call.arguments.as_map().clone()), context)
            .await
        {
            Ok(result) => result,
            Err(error) => tool_failure(error.to_string()),
        }
    };
    let after = hooks
        .run(HookContext {
            base,
            kind: HookContextKind::ToolAfter {
                tool_name: call.name,
                args: call.arguments,
                result: wire::JsonObject::from_serialize(&result)?,
                duration_ms: elapsed_ms(started)?,
                is_error: result.is_error == Some(true),
            },
        })
        .await?;
    if after.aborted || after.skipped {
        return Err(Error::Refused);
    }
    Ok((
        Message {
            role: Role::Tool,
            content: MessageContent::Text(serde_json::to_string(&result)?),
            reasoning: None,
            tool_calls: None,
            tool_call_id: Some(call.id),
        },
        used,
    ))
}

fn tool_failure(message: String) -> rmcp::model::CallToolResult {
    rmcp::model::CallToolResult::error(vec![rmcp::model::ContentBlock::text(message)])
}

fn hook_usage(usage: &LlmUsage) -> HookUsage {
    HookUsage {
        prompt_tokens: usage.prompt_tokens,
        completion_tokens: usage.completion_tokens,
    }
}

fn add_usage(total: &mut Option<HookUsage>, usage: Option<LlmUsage>) -> Result<(), Error> {
    if let Some(usage) = usage {
        if let Some(total) = total {
            let prompt_tokens = total
                .prompt_tokens
                .checked_add(usage.prompt_tokens)
                .ok_or(Error::UsageOverflow)?;
            let completion_tokens = total
                .completion_tokens
                .checked_add(usage.completion_tokens)
                .ok_or(Error::UsageOverflow)?;
            *total = HookUsage {
                prompt_tokens,
                completion_tokens,
            };
        } else {
            *total = Some(hook_usage(&usage));
        }
    }
    Ok(())
}

fn elapsed_ms(started: Instant) -> Result<u64, Error> {
    u64::try_from(started.elapsed().as_millis()).map_err(|_| Error::DurationOverflow)
}
