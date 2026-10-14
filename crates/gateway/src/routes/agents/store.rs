use std::cmp::Ordering;
use std::path::PathBuf;
use std::thread::JoinHandle;

use chrono::DateTime;
use chrono::Utc;
use parking_lot::Mutex;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use uuid::Uuid;
use wire::gateway::agents::AgentPreset;
use wire::gateway::agents::AgentPresetCreateRequest;
use wire::gateway::agents::AgentPresetUpdate;
use wire::gateway::agents::AgentsFile;
use wire::harness::HarnessId;

use super::check;
use super::error::AgentError;
use super::paths;
use crate::Settings;

const QUEUE: usize = 32;
const ID_ATTEMPTS: usize = 8;
const ID_LEN: usize = 12;

#[derive(Debug)]
struct Config {
    state_dir: PathBuf,
    directory_root: PathBuf,
    shared_dir: PathBuf,
    node: String,
    allowed: Option<Vec<HarnessId>>,
}

struct Files {
    config: Config,
    agents: Vec<AgentPreset>,
    loaded: bool,
}

enum Command {
    List {
        node: Option<String>,
        reply: oneshot::Sender<Result<Vec<AgentPreset>, AgentError>>,
    },
    Create {
        body: AgentPresetCreateRequest,
        reply: oneshot::Sender<Result<AgentPreset, AgentError>>,
    },
    Get {
        id: String,
        reply: oneshot::Sender<Result<AgentPreset, AgentError>>,
    },
    Update {
        id: String,
        body: AgentPresetUpdate,
        reply: oneshot::Sender<Result<AgentPreset, AgentError>>,
    },
    Delete {
        id: String,
        reply: oneshot::Sender<Result<(), AgentError>>,
    },
}

#[derive(Debug)]
enum Slot {
    Idle(Config),
    Running {
        tx: mpsc::Sender<Command>,
        join: JoinHandle<()>,
    },
    Failed,
}

#[derive(Debug)]
pub(crate) struct Registry {
    slot: Mutex<Slot>,
}

impl Registry {
    pub(crate) fn new(settings: &Settings) -> Self {
        let config = Config {
            state_dir: settings.state_dir.clone(),
            directory_root: settings.directory_root.clone(),
            shared_dir: settings.shared_dir.clone(),
            node: settings.node.clone(),
            allowed: settings.allowed_harnesses.clone(),
        };
        Self {
            slot: Mutex::new(Slot::Idle(config)),
        }
    }

    pub(crate) async fn list(&self, node: Option<String>) -> Result<Vec<AgentPreset>, AgentError> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::List { node, reply }).await?;
        rx.await.map_err(|_| AgentError::Unavailable)?
    }

    pub(crate) async fn create(
        &self,
        body: AgentPresetCreateRequest,
    ) -> Result<AgentPreset, AgentError> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::Create { body, reply }).await?;
        rx.await.map_err(|_| AgentError::Unavailable)?
    }

    pub(crate) async fn get(&self, id: String) -> Result<AgentPreset, AgentError> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::Get { id, reply }).await?;
        rx.await.map_err(|_| AgentError::Unavailable)?
    }

    pub(crate) async fn update(
        &self,
        id: String,
        body: AgentPresetUpdate,
    ) -> Result<AgentPreset, AgentError> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::Update { id, body, reply }).await?;
        rx.await.map_err(|_| AgentError::Unavailable)?
    }

    pub(crate) async fn delete(&self, id: String) -> Result<(), AgentError> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::Delete { id, reply }).await?;
        rx.await.map_err(|_| AgentError::Unavailable)?
    }

    async fn send(&self, command: Command) -> Result<(), AgentError> {
        self.sender()?
            .send(command)
            .await
            .map_err(|_| AgentError::Unavailable)
    }

    fn sender(&self) -> Result<mpsc::Sender<Command>, AgentError> {
        let mut slot = self.slot.lock();
        if let Slot::Running { tx, .. } = &*slot {
            return Ok(tx.clone());
        }
        if matches!(&*slot, Slot::Failed) {
            return Err(AgentError::Unavailable);
        }
        let Slot::Idle(config) = std::mem::replace(&mut *slot, Slot::Failed) else {
            return Err(AgentError::Unavailable);
        };
        let (tx, rx) = mpsc::channel(QUEUE);
        match std::thread::Builder::new()
            .name("agent-presets".to_owned())
            .spawn(move || run(rx, config))
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
                Err(AgentError::Unavailable)
            }
        }
    }
}

impl Drop for Registry {
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

fn run(mut rx: mpsc::Receiver<Command>, config: Config) {
    let mut files = Files {
        config,
        agents: Vec::new(),
        loaded: false,
    };
    while let Some(command) = rx.blocking_recv() {
        dispatch(&mut files, command);
    }
}

fn dispatch(files: &mut Files, command: Command) {
    match command {
        Command::List { node, reply } => {
            send(reply, files.list(node.as_deref()));
        }
        Command::Create { body, reply } => send(reply, files.create(body)),
        Command::Get { id, reply } => send(reply, files.get(&id)),
        Command::Update { id, body, reply } => send(reply, files.update(&id, body)),
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
    fn list(&mut self, node: Option<&str>) -> Result<Vec<AgentPreset>, AgentError> {
        self.ensure()?;
        let mut agents: Vec<AgentPreset> = self
            .agents
            .iter()
            .filter(|agent| node.is_none_or(|wanted| agent.node.as_deref() == Some(wanted)))
            .cloned()
            .collect();
        agents.sort_by(preset_order);
        Ok(agents)
    }

    fn get(&mut self, id: &str) -> Result<AgentPreset, AgentError> {
        self.ensure()?;
        let index = self.find(id)?;
        self.agents.get(index).cloned().ok_or(AgentError::NotFound)
    }

    fn create(&mut self, body: AgentPresetCreateRequest) -> Result<AgentPreset, AgentError> {
        self.ensure()?;
        let agent = self.draft(body)?;
        let directory = path_of(agent.directory.as_deref())?;
        paths::prepare_directory(&directory)?;
        paths::ensure_shared_link(
            &directory,
            &self.config.shared_dir,
            wants_link(agent.shared_link),
        )?;
        self.agents.push(agent.clone());
        self.persist()?;
        Ok(agent)
    }

    fn update(&mut self, id: &str, body: AgentPresetUpdate) -> Result<AgentPreset, AgentError> {
        self.ensure()?;
        let index = self.find(id)?;
        let current = self
            .agents
            .get(index)
            .cloned()
            .ok_or(AgentError::NotFound)?;
        if let Some(node) = check::foreign(&current, &self.config.node) {
            return Err(check::hosted(&current.name, node));
        }
        let next = merge(&self.config, &self.agents, &current, body)?;
        if next == current {
            return Ok(current);
        }
        apply_links(&self.config, &current, &next)?;
        let Some(slot) = self.agents.get_mut(index) else {
            return Err(AgentError::NotFound);
        };
        *slot = next.clone();
        self.persist()?;
        Ok(next)
    }

    fn delete(&mut self, id: &str) -> Result<(), AgentError> {
        self.ensure()?;
        let index = self.find(id)?;
        let current = self
            .agents
            .get(index)
            .cloned()
            .ok_or(AgentError::NotFound)?;
        if let Some(node) = check::foreign(&current, &self.config.node) {
            return Err(check::hosted(&current.name, node));
        }
        self.agents.remove(index);
        self.persist()?;
        Ok(())
    }

    fn draft(&self, body: AgentPresetCreateRequest) -> Result<AgentPreset, AgentError> {
        let name = check::name_create(&body.name)?;
        check::name_free(&self.agents, &name, None)?;
        let node = check::host_node(body.node.as_deref(), &self.config.node)?;
        let color = chosen_color(body.color)?;
        let mut model = provided_text(body.model);
        check::model(&model)?;
        let harness = migrated_harness(body.harness_id.map(HarnessId::from), &mut model);
        if let Some(id) = harness {
            check::allow(self.config.allowed.as_deref(), id)?;
        }
        let effort = chosen_effort(body.effort);
        check::effort(&effort)?;
        let system_prompt = provided_text(body.system_prompt);
        let node_base_url = provided_text(body.node_base_url);
        check::node_base_url(&node_base_url)?;
        if let Some(order) = body.sort_order {
            check::sort_order(order)?;
        }
        let shared_link = wants_link(body.shared_link);
        let id = fresh_id(&self.agents)?;
        let directory = match body.directory {
            Some(raw) => check::directory_str(&raw)?,
            None => default_directory(&self.config.directory_root, &id)?,
        };
        let now = now_millis()?;
        Ok(AgentPreset {
            id,
            name,
            color,
            harness_id: harness,
            model,
            effort,
            system_prompt,
            node: Some(node),
            directory: Some(utf8_path(&directory)?),
            shared_link: Some(shared_link),
            node_base_url: Some(node_base_url),
            sort_order: body.sort_order,
            created_at: now,
            updated_at: now,
            extra: wire::Extra::default(),
        })
    }

    fn ensure(&mut self) -> Result<(), AgentError> {
        if self.loaded {
            return Ok(());
        }
        self.agents = read_agents(&self.config)?;
        self.loaded = true;
        Ok(())
    }

    fn persist(&mut self) -> Result<(), AgentError> {
        match save(&self.config, &self.agents) {
            Ok(()) => Ok(()),
            Err(error) => {
                self.loaded = false;
                self.agents.clear();
                Err(error)
            }
        }
    }

    fn find(&self, id: &str) -> Result<usize, AgentError> {
        if !check::valid_id(id) {
            return Err(AgentError::NotFound);
        }
        self.agents
            .iter()
            .position(|agent| agent.id == id)
            .ok_or(AgentError::NotFound)
    }
}

fn merge(
    config: &Config,
    agents: &[AgentPreset],
    current: &AgentPreset,
    body: AgentPresetUpdate,
) -> Result<AgentPreset, AgentError> {
    let mut next = current.clone();
    if let Some(node) = body.node {
        check::node_change(current.node.as_deref(), &node)?;
    }
    if let Some(name) = body.name {
        let name = check::name_update(&name)?;
        check::name_free(agents, &name, Some(current.id.as_str()))?;
        next.name = name;
    }
    if let Some(color) = body.color {
        check::color(&color)?;
        next.color = color;
    }
    if let Some(harness) = body.harness_id {
        let harness = harness.map(HarnessId::from);
        if let Some(id) = harness {
            check::allow(config.allowed.as_deref(), id)?;
        }
        next.harness_id = harness;
    }
    if let Some(model) = body.model {
        check::model(&model)?;
        next.model = model;
    }
    if let Some(effort) = body.effort {
        check::effort(&effort)?;
        next.effort = effort;
    }
    if let Some(prompt) = body.system_prompt {
        next.system_prompt = prompt;
    }
    if let Some(directory) = body.directory {
        let path = check::directory_str(&directory)?;
        next.directory = Some(utf8_path(&path)?);
    }
    if let Some(shared) = body.shared_link {
        next.shared_link = Some(shared);
    }
    if let Some(url) = body.node_base_url {
        check::node_base_url(&url)?;
        next.node_base_url = Some(url);
    }
    if let Some(order) = body.sort_order {
        match order {
            Some(value) => {
                check::sort_order(value)?;
                next.sort_order = Some(value);
            }
            None => next.sort_order = None,
        }
    }
    if next != *current {
        next.updated_at = now_millis()?;
    }
    Ok(next)
}

fn apply_links(
    config: &Config,
    current: &AgentPreset,
    next: &AgentPreset,
) -> Result<(), AgentError> {
    let directory_changed = next.directory != current.directory;
    let link_changed = next.shared_link != current.shared_link;
    if !directory_changed && !link_changed {
        return Ok(());
    }
    let Some(directory) = next.directory.as_deref() else {
        return Ok(());
    };
    let path = path_of(Some(directory))?;
    paths::prepare_directory(&path)?;
    paths::ensure_shared_link(&path, &config.shared_dir, wants_link(next.shared_link))
}

fn read_agents(config: &Config) -> Result<Vec<AgentPreset>, AgentError> {
    let file = paths::registry_file(&config.state_dir)?;
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

fn save(config: &Config, agents: &[AgentPreset]) -> Result<(), AgentError> {
    let file = paths::registry_file(&config.state_dir)?;
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

fn fresh_id(agents: &[AgentPreset]) -> Result<String, AgentError> {
    for _ in 0..ID_ATTEMPTS {
        let raw = Uuid::new_v4().simple().to_string();
        let Some(short) = raw.get(..ID_LEN) else {
            return Err(AgentError::Unavailable);
        };
        let id = format!("p-{short}");
        if agents.iter().all(|agent| agent.id != id) {
            return Ok(id);
        }
    }
    Err(AgentError::Unavailable)
}

fn default_directory(root: &std::path::Path, id: &str) -> Result<PathBuf, AgentError> {
    let root = match check::directory_path(root) {
        Ok(normalized) => normalized,
        Err(error) => {
            tracing::warn!(%error, directory = %root.display(), "agent directory root is not usable");
            return Err(AgentError::Unavailable);
        }
    };
    if !check::valid_id(id) {
        tracing::warn!(id, "agent id is not a single path component");
        return Err(AgentError::Unavailable);
    }
    let path = root.join(id);
    match check::contained(&root, &path) {
        Ok(true) => Ok(path),
        Ok(false) => {
            tracing::warn!(directory = %path.display(), "agent directory leaves its root");
            Err(AgentError::Unavailable)
        }
        Err(error) => {
            tracing::warn!(%error, directory = %path.display(), "agent directory is not a contained path");
            Err(AgentError::Unavailable)
        }
    }
}

#[expect(
    clippy::manual_unwrap_or_default,
    reason = "an omitted model, system prompt, or nodeBaseUrl is stored as an empty string so the preset file matches the agents.json contract (CONTRACT B11)"
)]
fn provided_text(value: Option<String>) -> String {
    match value {
        Some(text) => text,
        None => String::new(),
    }
}

fn preset_order(left: &AgentPreset, right: &AgentPreset) -> Ordering {
    let rank = match (left.sort_order, right.sort_order) {
        (Some(first), Some(second)) => first.cmp(&second),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    };
    rank.then_with(|| left.created_at.cmp(&right.created_at))
        .then_with(|| left.id.cmp(&right.id))
}

fn migrated_harness(harness: Option<HarnessId>, model: &mut String) -> Option<HarnessId> {
    if harness.is_some() {
        return harness;
    }
    match check::catalog_harness(model) {
        Some(check::CatalogHarness::Known(id)) => {
            model.clear();
            Some(id)
        }
        Some(check::CatalogHarness::Unknown(name)) => {
            *model = name;
            None
        }
        None => None,
    }
}

fn chosen_effort(value: Option<String>) -> String {
    match value {
        Some(text) => text,
        None => check::DEFAULT_EFFORT.to_owned(),
    }
}

fn chosen_color(color: Option<String>) -> Result<String, AgentError> {
    match color {
        Some(value) if !value.is_empty() => {
            check::color(&value)?;
            Ok(value)
        }
        _ => Ok(String::new()),
    }
}

#[expect(
    clippy::manual_unwrap_or,
    reason = "an omitted sharedLink is on, so a new preset links the shared directory (CONTRACT B11, gateway-api §8.1)"
)]
fn wants_link(flag: Option<bool>) -> bool {
    match flag {
        Some(value) => value,
        None => true,
    }
}

fn path_of(directory: Option<&str>) -> Result<PathBuf, AgentError> {
    let Some(directory) = directory else {
        return Err(AgentError::BadRequest("directory must be an absolute path"));
    };
    check::directory_str(directory)
}

fn utf8_path(path: &std::path::Path) -> Result<String, AgentError> {
    path.to_str()
        .map(str::to_owned)
        .ok_or(AgentError::Unavailable)
}

fn now_millis() -> Result<DateTime<Utc>, AgentError> {
    DateTime::from_timestamp_millis(Utc::now().timestamp_millis()).ok_or(AgentError::Unavailable)
}
