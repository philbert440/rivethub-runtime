use std::collections::BTreeMap;
use std::sync::Arc;

use kernel::harness::{HarnessAdapterError, SessionOption, TurnAttachment};
use tokio::sync::broadcast::{self, error::RecvError};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use wire::harness::{ApprovalDecision, HarnessEvent, HarnessId, HarnessSessionStatus};
use wire::session_id::SessionId;
use wire::stream::LlmUsage;

use super::TerminalService;
use super::chat::ChatStart;
use crate::error::ApiError;
use crate::sessions::control::ChatHandle;
use crate::tenancy::UserId;
use crate::uploads::{AttachedTurn, Uploads};

#[derive(Debug)]
pub(crate) struct DelegatedTurn {
    pub(crate) start: ChatStart,
    pub(crate) text: String,
    pub(crate) system_prompt: Option<String>,
    pub(crate) attachments: Vec<TurnAttachment>,
    pub(crate) journal: Option<Arc<Uploads>>,
    pub(crate) delegate: Option<Delegate>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delegate {
    pub harness: HarnessId,
    pub model: Option<String>,
    pub cwd: Option<std::path::PathBuf>,
    pub permission_mode: Option<String>,
}

impl Delegate {
    fn decision(&self) -> ApprovalDecision {
        if self.permission_mode.as_deref() == Some(BYPASS_PERMISSIONS) {
            ApprovalDecision::Allow
        } else {
            ApprovalDecision::Deny
        }
    }
}

const BYPASS_PERMISSIONS: &str = "bypassPermissions";

struct Answering<'a> {
    service: &'a TerminalService,
    user: &'a UserId,
    session: &'a SessionId,
    decision: ApprovalDecision,
}

#[derive(Debug, Default)]
pub(crate) struct DelegatedReply {
    pub(crate) text: String,
    pub(crate) usage: Option<LlmUsage>,
}

impl TerminalService {
    #[must_use]
    pub fn with_delegates(mut self, delegates: BTreeMap<String, Delegate>) -> Self {
        self.delegates = delegates;
        self
    }

    pub(crate) fn delegate(&self, agent: &str) -> Option<&Delegate> {
        self.delegates.get(agent)
    }

    async fn delegate_mode(&self, session: &SessionId, mode: String) -> Result<(), ApiError> {
        let harness = self
            .harness
            .clone()
            .ok_or_else(|| ApiError::Unavailable("chat harness unavailable".into()))?;
        match harness.set_option(session, SessionOption::Mode(mode)).await {
            Ok(()) | Err(HarnessAdapterError::OptionUnavailable { .. }) => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    pub(crate) async fn delegated_turn(
        self: &Arc<Self>,
        user: &UserId,
        turn: DelegatedTurn,
    ) -> Result<DelegatedReply, ApiError> {
        let DelegatedTurn {
            mut start,
            text,
            system_prompt,
            attachments,
            journal,
            delegate,
        } = turn;
        start.cwd = start.cwd.map(|directory| self.options.root.join(directory));
        let summary = self.start_chat(user, start).await?;
        let session = summary.session_id.as_str();
        let key = self.registry.authorize(user, session)?;
        let chat = self.chat(user, session)?;
        let canonical = summary.session_id.clone();
        let events = self.subscribe_harness(&key);
        let abandoned = CancellationToken::new();
        let _abandon_on_drop = abandoned.clone().drop_guard();
        let (reply, answer) = oneshot::channel();
        let service = Arc::clone(self);
        let owner = user.clone();
        self.track(async move {
            let sending = async {
                let prompt = system_prompt.as_deref();
                journaled(journal.as_deref(), &canonical, &text, prompt, &attachments).await?;
                if let Some(mode) = delegate
                    .as_ref()
                    .and_then(|agent| agent.permission_mode.clone())
                {
                    service.delegate_mode(&canonical, mode).await?;
                }
                let answering = delegate.as_ref().map(|agent| Answering {
                    service: &service,
                    user: &owner,
                    session: &canonical,
                    decision: agent.decision(),
                });
                exchange(&chat, events, text, system_prompt, attachments, answering).await
            };
            let outcome = tokio::select! {
                outcome = sending => outcome,
                () = abandoned.cancelled() => chat
                    .stop()
                    .await
                    .and(Err(ApiError::Unavailable("delegated turn abandoned".into()))),
                () = service.shutdown.cancelled() => {
                    Err(ApiError::Unavailable("gateway is shutting down".into()))
                }
            };
            service.release_harness(&key);
            let closed = chat.close().await;
            if reply
                .send(outcome.and_then(|reply| closed.map(|()| reply)))
                .is_err()
            {
                tracing::debug!("delegated turn caller disconnected");
            }
        });
        answer
            .await
            .map_err(|_| ApiError::Unavailable("delegated turn stopped".into()))?
    }
}

async fn journaled(
    journal: Option<&Uploads>,
    session: &SessionId,
    text: &str,
    system_prompt: Option<&str>,
    attachments: &[TurnAttachment],
) -> Result<(), ApiError> {
    let Some(journal) = journal else {
        return Ok(());
    };
    let sent = crate::sessions::control::with_system_prompt(system_prompt, text.to_owned());
    journal
        .record_turn(session, AttachedTurn::new(&sent, attachments))
        .await
        .map_err(crate::routes::harnesses::upload_error)
}

async fn exchange(
    chat: &ChatHandle,
    mut events: broadcast::Receiver<HarnessEvent>,
    text: String,
    system_prompt: Option<String>,
    attachments: Vec<TurnAttachment>,
    answering: Option<Answering<'_>>,
) -> Result<DelegatedReply, ApiError> {
    chat.prompt(text, system_prompt, attachments, Vec::new())
        .await?;
    let mut reply = DelegatedReply::default();
    loop {
        let event = match events.recv().await {
            Ok(event) => event,
            Err(RecvError::Lagged(missed)) => {
                return Err(ApiError::Unavailable(format!(
                    "delegated turn missed {missed} harness events"
                )));
            }
            Err(RecvError::Closed) => {
                return Err(ApiError::Unavailable("harness event stream closed".into()));
            }
        };
        if let (Some(answering), HarnessEvent::ApprovalRequest { request_id, .. }) =
            (&answering, &event)
        {
            answering
                .service
                .answer_approval(
                    answering.user,
                    answering.session,
                    request_id,
                    answering.decision,
                )
                .await?;
            continue;
        }
        if observe(&mut reply, event)? {
            return Ok(reply);
        }
    }
}

fn observe(reply: &mut DelegatedReply, event: HarnessEvent) -> Result<bool, ApiError> {
    match event {
        HarnessEvent::AssistantDelta { text, .. } => {
            reply.text.push_str(&text);
            Ok(false)
        }
        HarnessEvent::TurnComplete { usage, .. } => {
            reply.usage = usage;
            Ok(true)
        }
        HarnessEvent::SessionUpdated {
            status: HarnessSessionStatus::Ended | HarnessSessionStatus::Error,
            ..
        } => Err(ApiError::Unavailable(
            "harness session ended before the turn completed".into(),
        )),
        HarnessEvent::Error { message, .. } => Err(ApiError::Unavailable(message)),
        HarnessEvent::SessionUpdated { .. }
        | HarnessEvent::ReasoningDelta { .. }
        | HarnessEvent::ToolUse { .. }
        | HarnessEvent::ToolResult { .. }
        | HarnessEvent::ApprovalRequest { .. }
        | HarnessEvent::ApprovalResolved { .. }
        | HarnessEvent::Plan { .. }
        | HarnessEvent::SessionCreated { .. }
        | HarnessEvent::Transcript { .. }
        | HarnessEvent::Status(_)
        | HarnessEvent::Prompt { .. } => Ok(false),
    }
}
