pub mod actor;
mod chat;
pub mod socket;

use std::collections::BTreeMap;
use std::path::{Component, PathBuf};
use std::sync::Arc;

use chrono::Utc;
use kernel::terminal::{TermSize, TerminalId, TerminalMux};
use parking_lot::RwLock;
use tokio::sync::{mpsc, oneshot, watch};
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use wire::gateway::term::{
    Mux, PtyInfo, PtyState, TermCommand, TermConfig, TermSpawnRequest, TermSpawnResponse,
};
use wire::harness::HarnessId;
use wire::session_id::SessionId;

use crate::error::ApiError;
use crate::sessions::SessionRegistry;
use crate::tenancy::UserId;
use actor::{Command, Handle};

#[derive(Debug, Clone)]
pub struct LaunchCommand {
    pub id: String,
    pub label: String,
    pub argv: Vec<String>,
    pub cwd: PathBuf,
    pub harness: Option<HarnessId>,
    pub session: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RememberedSession {
    pub id: String,
    pub session: String,
    pub command: String,
}

#[derive(Debug, Clone)]
pub struct TerminalOptions {
    pub root: PathBuf,
    pub default: String,
    pub commands: Vec<LaunchCommand>,
    pub max_ptys: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct Launch {
    pub(crate) command: LaunchCommand,
    pub(crate) size: TermSize,
    pub(crate) resume: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) effort: Option<String>,
}

#[expect(
    missing_debug_implementations,
    reason = "kernel ports intentionally do not require Debug"
)]
pub struct TerminalService {
    pub(crate) mux: Arc<dyn TerminalMux>,
    pub(crate) registry: Arc<SessionRegistry>,
    options: TerminalOptions,
    entries: RwLock<BTreeMap<String, Handle>>,
    adopted: RwLock<BTreeMap<String, (String, PtyInfo)>>,
    pub(crate) shutdown: CancellationToken,
    tasks: TaskTracker,
    permits: Arc<tokio::sync::Semaphore>,
    pub(crate) chats: RwLock<BTreeMap<String, crate::sessions::control::ChatHandle>>,
    harness: Option<Arc<dyn kernel::harness::HarnessAdapter>>,
    summaries: RwLock<BTreeMap<String, wire::harness::HarnessSessionSummary>>,
}

impl TerminalService {
    #[must_use]
    pub fn with_shutdown(mut self, shutdown: CancellationToken) -> Self {
        self.shutdown = shutdown;
        self
    }
    pub async fn new(
        mux: Arc<dyn TerminalMux>,
        registry: Arc<SessionRegistry>,
        mut options: TerminalOptions,
    ) -> Result<Self, ApiError> {
        options = tokio::task::spawn_blocking(move || validate_options(options))
            .await
            .map_err(ApiError::Task)??;
        Ok(Self {
            mux,
            registry,
            permits: Arc::new(tokio::sync::Semaphore::new(options.max_ptys)),
            options,
            entries: RwLock::new(BTreeMap::new()),
            adopted: RwLock::new(BTreeMap::new()),
            shutdown: CancellationToken::new(),
            tasks: TaskTracker::new(),
            chats: RwLock::new(BTreeMap::new()),
            harness: None,
            summaries: RwLock::new(BTreeMap::new()),
        })
    }

    #[must_use]
    pub fn with_harness(mut self, harness: Arc<dyn kernel::harness::HarnessAdapter>) -> Self {
        self.harness = Some(harness);
        self
    }

    pub(crate) fn adapter(&self) -> Option<Arc<dyn kernel::harness::HarnessAdapter>> {
        self.harness.clone()
    }

    pub(crate) fn launch(&self, request: TermSpawnRequest) -> Result<Launch, ApiError> {
        if request.agent_id.is_some() {
            return Err(ApiError::NotFound("agent not found"));
        }
        let key = match request.command {
            Some(key) => key,
            None => self.options.default.clone(),
        };
        let command = self
            .options
            .commands
            .iter()
            .find(|command| command.id == key)
            .cloned()
            .ok_or_else(|| ApiError::UnknownCommand(key))?;
        for (label, value) in [("model", &request.model), ("effort", &request.effort)] {
            if let Some(value) = value
                && (value.is_empty()
                    || value.chars().count() > 64
                    || value.contains("..")
                    || !value
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':')))
            {
                return Err(ApiError::BadRequest(format!(
                    "{label} must be a 1-64 token"
                )));
            }
        }
        let cols = match request.cols {
            Some(cols) => cols.clamp(20, 400),
            None => 80,
        };
        let rows = match request.rows {
            Some(rows) => rows.clamp(10, 200),
            None => 24,
        };
        Ok(Launch {
            command,
            size: TermSize { cols, rows },
            resume: request.resume,
            model: request.model,
            effort: request.effort,
        })
    }

    pub async fn spawn(
        self: &Arc<Self>,
        user: &UserId,
        request: TermSpawnRequest,
    ) -> Result<TermSpawnResponse, ApiError> {
        let session = match (&request.session, &request.resume) {
            (Some(session), Some(resume)) => {
                let resumed = self.registry.authorize(user, resume)?;
                if crate::sessions::session_key(session)? != resumed {
                    return Err(ApiError::Conflict(
                        "resume must retain the registered session id".into(),
                    ));
                }
                resumed
            }
            (None, Some(resume)) => self.registry.authorize(user, resume)?,
            (Some(session), None) => session.clone(),
            (None, None) => format!("rivethub-{}", uuid::Uuid::new_v4()),
        };
        let session = self.registry.claim(user, &session)?;
        let launch = self.launch(request)?;
        let chat = self.chats.read().get(&session).cloned();
        if let Some(chat) = chat {
            return chat.terminal(launch).await;
        }
        let port_session = Self::port_session(&launch.command, &session)?;
        let chat_capable = launch.command.harness.is_some();
        let response = self.spawn_raw(user, &session, launch).await?;
        if chat_capable && let Some(harness) = &self.harness {
            let mut chats = self.chats.write();
            if let std::collections::btree_map::Entry::Vacant(entry) = chats.entry(session) {
                let chat = crate::sessions::control::ChatActor::start(
                    self,
                    harness.clone(),
                    user.clone(),
                    port_session,
                    None,
                )?;
                entry.insert(chat);
            }
            drop(chats);
        }
        Ok(response)
    }

    pub(crate) async fn spawn_raw(
        self: &Arc<Self>,
        user: &UserId,
        session: &str,
        launch: Launch,
    ) -> Result<TermSpawnResponse, ApiError> {
        let handle = {
            let mut entries = self.entries.write();
            entries
                .entry(session.to_owned())
                .or_insert_with(|| {
                    let (commands, receiver) = mpsc::channel(64);
                    let (status, state) = watch::channel(None);
                    let (owner, ownership) = watch::channel(None);
                    let handle = Handle {
                        commands,
                        state,
                        ownership,
                    };
                    let actor = actor::TerminalActor::new(
                        self.clone(),
                        user.clone(),
                        session.into(),
                        status,
                        owner,
                    );
                    self.tasks.spawn(async move {
                        if let Err(error) = actor.run(receiver).await {
                            tracing::error!(%error, "terminal actor stopped");
                        }
                    });
                    handle
                })
                .clone()
        };
        let (reply, result) = oneshot::channel();
        handle.send(Command::Spawn { launch, reply }).await?;
        result
            .await
            .map_err(|_| ApiError::Unavailable("terminal actor closed".into()))?
    }

    pub(crate) fn find(
        &self,
        user: &UserId,
        id: Option<&str>,
        session: Option<&str>,
    ) -> Result<(String, Handle), ApiError> {
        let entries = self.entries.read();
        let found = if let Some(id) = id {
            entries.iter().find(|(_, handle)| {
                handle
                    .state
                    .borrow()
                    .as_ref()
                    .is_some_and(|state| state.id == id)
            })
        } else if let Some(session) = session {
            let key = self.registry.authorize(user, session)?;
            entries.get_key_value(&key)
        } else {
            None
        };
        let (key, handle) = found.ok_or(ApiError::NotFound("unknown pty"))?;
        self.registry.authorize(user, key)?;
        Ok((key.clone(), handle.clone()))
    }

    pub fn remember(&self, user: &UserId, adopted: RememberedSession) -> Result<(), ApiError> {
        let key = self.registry.claim(user, &adopted.session)?;
        let info = PtyInfo {
            id: adopted.id.clone(),
            session: key.clone(),
            command: adopted.command,
            state: PtyState::Running,
            pid: None,
            attached: 0,
            exit_code: None,
            created_at: Utc::now(),
            last_output_ts: None,
            cols: None,
            rows: None,
            mux: Some(Mux::Herdr),
            persisted: Some(true),
            reattached: Some(true),
            attach: None,
        };
        self.adopted.write().insert(adopted.id, (key, info));
        Ok(())
    }

    pub fn list(&self, user: &UserId) -> Vec<PtyInfo> {
        let mut listed: Vec<PtyInfo> = self
            .entries
            .read()
            .iter()
            .filter(|(key, _)| self.registry.authorize(user, key).is_ok())
            .filter_map(|(_, handle)| handle.state.borrow().clone())
            .collect();
        let seen: std::collections::BTreeSet<String> =
            listed.iter().map(|info| info.id.clone()).collect();
        for (key, info) in self.adopted.read().values() {
            if seen.contains(&info.id) {
                continue;
            }
            if self.registry.authorize(user, key).is_ok() {
                listed.push(info.clone());
            }
        }
        listed
    }

    pub fn config(&self, user: &UserId) -> TermConfig {
        TermConfig {
            enabled: true,
            default: self.options.default.clone(),
            max_ptys: self.options.max_ptys as u64,
            active: self
                .list(user)
                .iter()
                .filter(|pty| pty.state == wire::gateway::term::PtyState::Running)
                .count() as u64,
            commands: self
                .options
                .commands
                .iter()
                .map(|command| TermCommand {
                    id: command.id.clone(),
                    label: command.label.clone(),
                    session: command.session,
                })
                .collect(),
        }
    }

    pub async fn kill(&self, user: &UserId, id: &str) -> Result<(), ApiError> {
        match self.find(user, Some(id), None) {
            Ok((_, handle)) => {
                let (reply, result) = oneshot::channel();
                handle.send(Command::Kill { reply }).await?;
                result
                    .await
                    .map_err(|_| ApiError::Unavailable("terminal actor closed".into()))?
            }
            Err(ApiError::NotFound(_)) => self.kill_remembered(user, id).await,
            Err(error) => Err(error),
        }
    }

    async fn kill_remembered(&self, user: &UserId, id: &str) -> Result<(), ApiError> {
        let key = self.adopted.read().get(id).map(|(key, _)| key.clone());
        let Some(key) = key else {
            return Err(ApiError::NotFound("unknown pty"));
        };
        self.registry.authorize(user, &key)?;
        self.mux.kill(&TerminalId::new(id)).await?;
        self.adopted.write().remove(id);
        Ok(())
    }

    pub async fn remove_session(&self, user: &UserId, session: &str) -> Result<(), ApiError> {
        let key = self.registry.authorize(user, session)?;
        let chat = self.chats.write().remove(&key);
        self.summaries.write().remove(&key);
        if let Some(chat) = chat {
            chat.close().await?;
        }
        self.remove_terminal(user, session).await
    }

    pub(crate) async fn remove_terminal(
        &self,
        user: &UserId,
        session: &str,
    ) -> Result<(), ApiError> {
        let key = self.registry.authorize(user, session)?;
        let handle = self.entries.read().get(&key).cloned();
        if let Some(handle) = handle {
            let state = handle.state.borrow().clone();
            if let Some(state) = state {
                self.kill(user, &state.id).await?;
            }
        }
        Ok(())
    }

    pub async fn close(&self) {
        self.shutdown.cancel();
        self.tasks.close();
        self.tasks.wait().await;
    }

    pub(crate) fn port_session(
        command: &LaunchCommand,
        session: &str,
    ) -> Result<SessionId, ApiError> {
        if let Some((prefix, _)) = session.split_once(':') {
            let harness = prefix
                .parse::<HarnessId>()
                .map_err(|error| ApiError::BadRequest(error.to_string()))?;
            if command.harness.is_some_and(|selected| selected != harness) {
                return Err(ApiError::Conflict(
                    "session harness does not match terminal command".into(),
                ));
            }
            return session
                .parse()
                .map_err(|error: wire::session_id::SessionIdError| {
                    ApiError::BadRequest(error.to_string())
                });
        }
        Ok(match command.harness {
            Some(harness) => SessionId::new(harness, session),
            None => SessionId::new(HarnessId::Cowork, session),
        })
    }
}

fn validate_options(mut options: TerminalOptions) -> Result<TerminalOptions, ApiError> {
    options.root = options.root.canonicalize().map_err(ApiError::Io)?;
    if options.max_ptys == 0
        || !options
            .commands
            .iter()
            .any(|command| command.id == options.default)
    {
        return Err(ApiError::BadRequest(
            "invalid terminal configuration".into(),
        ));
    }
    for command in &mut options.commands {
        if command.argv.is_empty() {
            return Err(ApiError::BadRequest(
                "invalid terminal command or directory".into(),
            ));
        }
        command.cwd = contained_directory(&options.root, &command.cwd)?;
    }
    Ok(options)
}

fn contained_directory(
    root: &std::path::Path,
    directory: &std::path::Path,
) -> Result<PathBuf, ApiError> {
    if !directory.is_absolute()
        || directory
            .components()
            .any(|part| part == Component::ParentDir)
    {
        return Err(ApiError::BadRequest(
            "invalid terminal command or directory".into(),
        ));
    }
    let directory = directory.canonicalize().map_err(ApiError::Io)?;
    if !directory.starts_with(root) {
        return Err(ApiError::SessionForbidden);
    }
    if !directory.is_dir() {
        return Err(ApiError::BadRequest(
            "working directory is not a directory".into(),
        ));
    }
    Ok(directory)
}
