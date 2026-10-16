use std::sync::Arc;

use kernel::harness::SessionSpec;
use wire::events::{AgentEvent, AgentEventKind, ProtocolVersion};
use wire::harness::{HarnessId, HarnessSessionStatus, HarnessSessionSummary, Transport};
use wire::session_id::SessionId;

use super::TerminalService;
use crate::error::ApiError;
use crate::sessions::control::{ChatActor, ChatHandle};
use crate::tenancy::UserId;

impl TerminalService {
    pub(crate) fn track(&self, future: impl Future<Output = ()> + Send + 'static) {
        self.tasks.spawn(future);
    }

    pub async fn create_chat(
        self: &Arc<Self>,
        user: &UserId,
        harness_id: HarnessId,
        model: Option<String>,
        cwd: Option<std::path::PathBuf>,
    ) -> Result<HarnessSessionSummary, ApiError> {
        let harness = self
            .harness
            .clone()
            .ok_or_else(|| ApiError::Unavailable("chat harness unavailable".into()))?;
        let command = self
            .options
            .commands
            .iter()
            .find(|command| command.harness == Some(harness_id))
            .ok_or_else(|| ApiError::BadRequest("harness is not allowed on this node".into()))?;
        let directory = match cwd {
            Some(directory) => {
                let root = self.options.root.clone();
                tokio::task::spawn_blocking(move || super::contained_directory(&root, &directory))
                    .await
                    .map_err(ApiError::Task)??
            }
            None => command.cwd.clone(),
        };
        let spec = SessionSpec {
            session: SessionId::new(harness_id, &uuid::Uuid::new_v4().to_string()),
            harness: harness_id,
            model: model.clone(),
            effort: None,
            working_dir: Some(directory.clone()),
        };
        let session = harness.new_session(spec).await?;
        let initialized = self
            .register_chat(user, &harness, &session, harness_id)
            .await;
        if let Err(error) = initialized {
            harness.release(&session).await?;
            return Err(error);
        }
        let now = chrono::Utc::now();
        let summary = HarnessSessionSummary {
            session_id: session,
            harness_id,
            title: None,
            cwd: Some(directory.to_string_lossy().into_owned()),
            created_at: now,
            updated_at: now,
            status: HarnessSessionStatus::Idle,
            blocked: None,
            supersedes: None,
            model,
            parent_session_id: None,
            agent_name: None,
            task_id: None,
            effort: None,
            transport: Some(Transport::Protocol),
            extra: wire::Extra::default(),
        };
        let key = self.registry.authorize(user, summary.session_id.as_str())?;
        self.summaries.write().insert(key, summary.clone());
        Ok(summary)
    }

    async fn register_chat(
        self: &Arc<Self>,
        user: &UserId,
        harness: &Arc<dyn kernel::harness::HarnessAdapter>,
        session: &SessionId,
        harness_id: HarnessId,
    ) -> Result<(), ApiError> {
        let key = self.registry.claim(user, session.as_str())?;
        let events = harness.events(session).await?;
        let chat = ChatActor::start(
            self,
            harness.clone(),
            user.clone(),
            session.clone(),
            Some(events),
        )?;
        self.chats.write().insert(key.clone(), chat);
        self.registry.ingest(
            user,
            vec![AgentEvent {
                v: ProtocolVersion::V1,
                session: key,
                kind: AgentEventKind::SessionStart {
                    title: None,
                    name: None,
                    harness: Some(harness_id.into()),
                    ts: Some(chrono::Utc::now()),
                },
                extra: wire::Extra::default(),
            }],
        )?;
        Ok(())
    }

    pub fn chat(&self, user: &UserId, session: &str) -> Result<ChatHandle, ApiError> {
        let key = self.registry.authorize(user, session)?;
        self.chats
            .read()
            .get(&key)
            .cloned()
            .ok_or(ApiError::NotFound("unknown chat session"))
    }

    pub fn harness_sessions(&self, user: &UserId) -> wire::harness::HarnessSessionList {
        let sessions = self
            .summaries
            .read()
            .iter()
            .filter(|(key, _)| self.registry.authorize(user, key).is_ok())
            .map(|(_, summary)| summary.clone())
            .collect();
        wire::harness::HarnessSessionList { sessions }
    }

    pub async fn terminal_turn_end(&self, user: &UserId, session: &str) -> Result<(), ApiError> {
        let key = self.registry.authorize(user, session)?;
        let chat = self.chats.read().get(&key).cloned();
        if let Some(chat) = chat {
            chat.turn_end().await?;
        }
        Ok(())
    }
}
