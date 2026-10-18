use std::path::PathBuf;
use std::thread::JoinHandle;

use async_trait::async_trait;
use kernel::agent_presets::AgentPresetStore;
use kernel::agent_presets::AgentPresetStoreError;
use parking_lot::Mutex;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use wire::gateway::agents::AgentPreset;
use wire::gateway::agents::AgentPresetUpdate;
use wire::gateway::agents::AgentStoreBackend;
use wire::gateway::agents::AgentsFile;

use super::changes;
use super::check;
use super::error::AgentError;
use super::paths;

const QUEUE: usize = 32;

type Reply<T> = oneshot::Sender<Result<T, AgentPresetStoreError>>;

struct Files {
    state_dir: PathBuf,
    agents: Vec<AgentPreset>,
    loaded: bool,
}

enum Command {
    List {
        node: Option<String>,
        reply: Reply<Vec<AgentPreset>>,
    },
    Get {
        id: String,
        reply: Reply<Option<AgentPreset>>,
    },
    Lookup {
        key: String,
        reply: Reply<Option<AgentPreset>>,
    },
    Create {
        preset: Box<AgentPreset>,
        reply: Reply<AgentPreset>,
    },
    Update {
        id: String,
        changes: Box<AgentPresetUpdate>,
        reply: Reply<Option<AgentPreset>>,
    },
    Delete {
        id: String,
        reply: Reply<bool>,
    },
}

#[derive(Debug)]
enum Slot {
    Idle(PathBuf),
    Running {
        tx: mpsc::Sender<Command>,
        join: JoinHandle<()>,
    },
    Failed,
}

#[derive(Debug)]
pub(crate) struct FilePresets {
    slot: Mutex<Slot>,
}

impl FilePresets {
    pub(crate) fn new(state_dir: PathBuf) -> Self {
        Self {
            slot: Mutex::new(Slot::Idle(state_dir)),
        }
    }

    async fn call<T>(
        &self,
        command: impl FnOnce(Reply<T>) -> Command,
    ) -> Result<T, AgentPresetStoreError> {
        let (reply, rx) = oneshot::channel();
        self.sender()?
            .send(command(reply))
            .await
            .map_err(|_| unavailable(AgentError::Unavailable))?;
        rx.await.map_err(|_| unavailable(AgentError::Unavailable))?
    }

    fn sender(&self) -> Result<mpsc::Sender<Command>, AgentPresetStoreError> {
        let mut slot = self.slot.lock();
        if let Slot::Running { tx, .. } = &*slot {
            return Ok(tx.clone());
        }
        if matches!(&*slot, Slot::Failed) {
            return Err(unavailable(AgentError::Unavailable));
        }
        let Slot::Idle(state_dir) = std::mem::replace(&mut *slot, Slot::Failed) else {
            return Err(unavailable(AgentError::Unavailable));
        };
        let (tx, rx) = mpsc::channel(QUEUE);
        match std::thread::Builder::new()
            .name("agent-presets".to_owned())
            .spawn(move || run(rx, state_dir))
        {
            Ok(join) => {
                let sender = tx.clone();
                *slot = Slot::Running { tx, join };
                drop(slot);
                Ok(sender)
            }
            Err(error) => {
                drop(slot);
                tracing::error!(%error, "agent preset registry thread did not start");
                Err(unavailable(AgentError::Unavailable))
            }
        }
    }
}

#[async_trait]
impl AgentPresetStore for FilePresets {
    fn backend(&self) -> AgentStoreBackend {
        AgentStoreBackend::File
    }

    async fn list(&self, node: Option<&str>) -> Result<Vec<AgentPreset>, AgentPresetStoreError> {
        let node = node.map(str::to_owned);
        self.call(|reply| Command::List { node, reply }).await
    }

    async fn get(&self, id: &str) -> Result<Option<AgentPreset>, AgentPresetStoreError> {
        let id = id.to_owned();
        self.call(|reply| Command::Get { id, reply }).await
    }

    async fn lookup(&self, id_or_name: &str) -> Result<Option<AgentPreset>, AgentPresetStoreError> {
        let key = id_or_name.to_owned();
        self.call(|reply| Command::Lookup { key, reply }).await
    }

    async fn create(&self, preset: AgentPreset) -> Result<AgentPreset, AgentPresetStoreError> {
        let preset = Box::new(preset);
        self.call(|reply| Command::Create { preset, reply }).await
    }

    async fn update(
        &self,
        id: &str,
        changes: AgentPresetUpdate,
    ) -> Result<Option<AgentPreset>, AgentPresetStoreError> {
        let id = id.to_owned();
        let changes = Box::new(changes);
        self.call(|reply| Command::Update { id, changes, reply })
            .await
    }

    async fn delete(&self, id: &str) -> Result<bool, AgentPresetStoreError> {
        let id = id.to_owned();
        self.call(|reply| Command::Delete { id, reply }).await
    }
}

impl Drop for FilePresets {
    fn drop(&mut self) {
        let slot = std::mem::replace(&mut *self.slot.lock(), Slot::Failed);
        let Slot::Running { tx, join } = slot else {
            return;
        };
        drop(tx);
        if let Err(panic) = join.join() {
            tracing::error!(?panic, "agent preset registry thread panicked");
        }
    }
}

fn run(mut rx: mpsc::Receiver<Command>, state_dir: PathBuf) {
    let mut files = Files {
        state_dir,
        agents: Vec::new(),
        loaded: false,
    };
    while let Some(command) = rx.blocking_recv() {
        dispatch(&mut files, command);
    }
}

fn dispatch(files: &mut Files, command: Command) {
    match command {
        Command::List { node, reply } => send(reply, files.list(node.as_deref())),
        Command::Get { id, reply } => send(reply, files.get(&id)),
        Command::Lookup { key, reply } => send(reply, files.lookup(&key)),
        Command::Create { preset, reply } => send(reply, files.create(*preset)),
        Command::Update { id, changes, reply } => send(reply, files.update(&id, *changes)),
        Command::Delete { id, reply } => send(reply, files.delete(&id)),
    }
}

fn send<T>(reply: oneshot::Sender<T>, value: T) {
    if let Err(unsent) = reply.send(value) {
        drop(unsent);
        tracing::debug!("agent preset caller dropped the request");
    }
}

impl Files {
    fn list(&mut self, node: Option<&str>) -> Result<Vec<AgentPreset>, AgentPresetStoreError> {
        self.ensure()?;
        Ok(self
            .agents
            .iter()
            .filter(|agent| node.is_none_or(|wanted| agent.node.as_deref() == Some(wanted)))
            .cloned()
            .collect())
    }

    fn get(&mut self, id: &str) -> Result<Option<AgentPreset>, AgentPresetStoreError> {
        self.ensure()?;
        Ok(self.agents.iter().find(|agent| agent.id == id).cloned())
    }

    fn lookup(&mut self, key: &str) -> Result<Option<AgentPreset>, AgentPresetStoreError> {
        self.ensure()?;
        let wanted = key.to_lowercase();
        let by_id = self.agents.iter().find(|agent| agent.id == key);
        let found = match by_id {
            Some(agent) => Some(agent),
            None => self
                .agents
                .iter()
                .find(|agent| agent.name.to_lowercase() == wanted),
        };
        Ok(found.cloned())
    }

    fn create(&mut self, preset: AgentPreset) -> Result<AgentPreset, AgentPresetStoreError> {
        self.ensure()?;
        name_free(&self.agents, &preset.name, None)?;
        if self.agents.iter().any(|agent| agent.id == preset.id) {
            tracing::warn!(id = preset.id, "agent preset id is already stored");
            return Err(unavailable(AgentError::Unavailable));
        }
        self.agents.push(preset.clone());
        self.persist()?;
        Ok(preset)
    }

    fn update(
        &mut self,
        id: &str,
        changes: AgentPresetUpdate,
    ) -> Result<Option<AgentPreset>, AgentPresetStoreError> {
        self.ensure()?;
        let Some(index) = self.agents.iter().position(|agent| agent.id == id) else {
            return Ok(None);
        };
        if let Some(name) = &changes.name {
            name_free(&self.agents, name, Some(id))?;
        }
        let Some(slot) = self.agents.get_mut(index) else {
            return Ok(None);
        };
        changes::apply(slot, changes);
        slot.updated_at = changes::now_millis().map_err(unavailable)?;
        let next = slot.clone();
        self.persist()?;
        Ok(Some(next))
    }

    fn delete(&mut self, id: &str) -> Result<bool, AgentPresetStoreError> {
        self.ensure()?;
        let Some(index) = self.agents.iter().position(|agent| agent.id == id) else {
            return Ok(false);
        };
        self.agents.remove(index);
        self.persist()?;
        Ok(true)
    }

    fn ensure(&mut self) -> Result<(), AgentPresetStoreError> {
        if self.loaded {
            return Ok(());
        }
        self.agents = read_agents(&self.state_dir).map_err(unavailable)?;
        self.loaded = true;
        Ok(())
    }

    fn persist(&mut self) -> Result<(), AgentPresetStoreError> {
        match save(&self.state_dir, &self.agents) {
            Ok(()) => Ok(()),
            Err(error) => {
                self.loaded = false;
                self.agents.clear();
                Err(unavailable(error))
            }
        }
    }
}

fn name_free(
    agents: &[AgentPreset],
    name: &str,
    except: Option<&str>,
) -> Result<(), AgentPresetStoreError> {
    if check::name_taken(agents, name, except) {
        Err(AgentPresetStoreError::NameTaken {
            name: name.to_owned(),
        })
    } else {
        Ok(())
    }
}

fn unavailable(error: AgentError) -> AgentPresetStoreError {
    AgentPresetStoreError::Unavailable(Box::new(error))
}

fn read_agents(state_dir: &std::path::Path) -> Result<Vec<AgentPreset>, AgentError> {
    let file = paths::registry_file(state_dir)?;
    match std::fs::read(&file) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => {
            tracing::warn!(%error, "agent registry read failed");
            Err(AgentError::Unavailable)
        }
        Ok(bytes) => match serde_json::from_slice::<AgentsFile>(&bytes) {
            Ok(parsed) => Ok(parsed.agents),
            Err(error) => {
                tracing::warn!(%error, "agent registry file is not a preset list");
                Err(AgentError::Unavailable)
            }
        },
    }
}

fn save(state_dir: &std::path::Path, agents: &[AgentPreset]) -> Result<(), AgentError> {
    let file = paths::registry_file(state_dir)?;
    let body = AgentsFile {
        agents: agents.to_vec(),
    };
    let mut bytes = serde_json::to_vec_pretty(&body).map_err(|error| {
        tracing::warn!(%error, "agent registry encode failed");
        AgentError::Unavailable
    })?;
    bytes.push(b'\n');
    paths::write_file(&file, &bytes)
}
