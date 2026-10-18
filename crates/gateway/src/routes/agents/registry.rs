use std::cmp::Ordering;
use std::path::PathBuf;
use std::sync::Arc;

use kernel::agent_presets::AgentPresetStore;
use uuid::Uuid;
use wire::gateway::agents::AgentPreset;
use wire::gateway::agents::AgentPresetCreateRequest;
use wire::gateway::agents::AgentPresetUpdate;
use wire::gateway::agents::AgentStoreBackend;
use wire::harness::HarnessId;

use super::changes;
use super::check;
use super::error::AgentError;
use super::paths;
use crate::Settings;

const ID_ATTEMPTS: usize = 8;
const ID_LEN: usize = 12;

#[derive(Debug, Clone)]
struct Config {
    directory_root: PathBuf,
    shared_dir: PathBuf,
    node: String,
    allowed: Option<Vec<HarnessId>>,
}

pub(crate) struct Registry {
    config: Config,
    store: Arc<dyn AgentPresetStore>,
}

impl Registry {
    pub(crate) fn new(settings: &Settings, store: Arc<dyn AgentPresetStore>) -> Self {
        Self {
            config: Config {
                directory_root: settings.directory_root.clone(),
                shared_dir: settings.shared_dir.clone(),
                node: settings.node.clone(),
                allowed: settings.allowed_harnesses.clone(),
            },
            store,
        }
    }

    pub(crate) fn backend(&self) -> AgentStoreBackend {
        self.store.backend()
    }

    pub(crate) async fn list(&self, node: Option<String>) -> Result<Vec<AgentPreset>, AgentError> {
        let mut agents = self
            .store
            .list(node.as_deref())
            .await
            .map_err(AgentError::from_store)?;
        agents.sort_by(preset_order);
        Ok(agents)
    }

    pub(crate) async fn get(&self, id: String) -> Result<AgentPreset, AgentError> {
        self.find(&id).await
    }

    pub(crate) async fn create(
        &self,
        body: AgentPresetCreateRequest,
    ) -> Result<AgentPreset, AgentError> {
        let agents = self.all().await?;
        let agent = draft(&self.config, &agents, body)?;
        let directory = path_of(agent.directory.as_deref())?;
        let shared = self.config.shared_dir.clone();
        let link = wants_link(agent.shared_link);
        blocking(move || {
            paths::prepare_directory(&directory)?;
            paths::ensure_shared_link(&directory, &shared, link)
        })
        .await?;
        self.store
            .create(agent)
            .await
            .map_err(AgentError::from_store)
    }

    pub(crate) async fn update(
        &self,
        id: String,
        body: AgentPresetUpdate,
    ) -> Result<AgentPreset, AgentError> {
        let current = self.find(&id).await?;
        if let Some(node) = check::foreign(&current, &self.config.node) {
            return Err(check::hosted(&current.name, node));
        }
        let agents = match body.name {
            Some(_) => self.all().await?,
            None => Vec::new(),
        };
        let changes = merge(&self.config, &agents, &current, body)?;
        let mut next = current.clone();
        changes::apply(&mut next, changes.clone());
        if next == current {
            return Ok(current);
        }
        let shared = self.config.shared_dir.clone();
        let before = current.clone();
        let after = next.clone();
        blocking(move || apply_links(&shared, &before, &after)).await?;
        self.store
            .update(&current.id, changes)
            .await
            .map_err(AgentError::from_store)?
            .ok_or(AgentError::NotFound)
    }

    pub(crate) async fn delete(&self, id: String) -> Result<(), AgentError> {
        let current = self.find(&id).await?;
        if let Some(node) = check::foreign(&current, &self.config.node) {
            return Err(check::hosted(&current.name, node));
        }
        let removed = self
            .store
            .delete(&current.id)
            .await
            .map_err(AgentError::from_store)?;
        if removed {
            Ok(())
        } else {
            Err(AgentError::NotFound)
        }
    }

    async fn all(&self) -> Result<Vec<AgentPreset>, AgentError> {
        self.store.list(None).await.map_err(AgentError::from_store)
    }

    async fn find(&self, id: &str) -> Result<AgentPreset, AgentError> {
        if !check::valid_id(id) {
            return Err(AgentError::NotFound);
        }
        self.store
            .get(id)
            .await
            .map_err(AgentError::from_store)?
            .ok_or(AgentError::NotFound)
    }
}

async fn blocking(
    work: impl FnOnce() -> Result<(), AgentError> + Send + 'static,
) -> Result<(), AgentError> {
    match tokio::task::spawn_blocking(work).await {
        Ok(result) => result,
        Err(error) => {
            tracing::error!(%error, "agent directory task failed");
            Err(AgentError::Unavailable)
        }
    }
}

fn draft(
    config: &Config,
    agents: &[AgentPreset],
    body: AgentPresetCreateRequest,
) -> Result<AgentPreset, AgentError> {
    let name = check::name_create(&body.name)?;
    check::name_free(agents, &name, None)?;
    let node = check::host_node(body.node.as_deref(), &config.node)?;
    let color = chosen_color(body.color)?;
    let mut model = provided_text(body.model);
    check::model(&model)?;
    let harness = migrated_harness(body.harness_id.map(HarnessId::from), &mut model);
    if let Some(id) = harness {
        check::allow(config.allowed.as_deref(), id)?;
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
    let id = fresh_id(agents)?;
    let directory = match body.directory {
        Some(raw) => check::directory_str(&raw)?,
        None => default_directory(&config.directory_root, &id)?,
    };
    let now = changes::now_millis()?;
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

fn merge(
    config: &Config,
    agents: &[AgentPreset],
    current: &AgentPreset,
    body: AgentPresetUpdate,
) -> Result<AgentPresetUpdate, AgentError> {
    let mut changes = AgentPresetUpdate::default();
    if let Some(node) = body.node {
        check::node_change(current.node.as_deref(), &node)?;
    }
    if let Some(name) = body.name {
        let name = check::name_update(&name)?;
        check::name_free(agents, &name, Some(current.id.as_str()))?;
        changes.name = Some(name);
    }
    if let Some(color) = body.color {
        check::color(&color)?;
        changes.color = Some(color);
    }
    if let Some(harness) = body.harness_id {
        if let Some(id) = harness {
            check::allow(config.allowed.as_deref(), HarnessId::from(id))?;
        }
        changes.harness_id = Some(harness);
    }
    if let Some(model) = body.model {
        check::model(&model)?;
        changes.model = Some(model);
    }
    if let Some(effort) = body.effort {
        check::effort(&effort)?;
        changes.effort = Some(effort);
    }
    changes.system_prompt = body.system_prompt;
    if let Some(directory) = body.directory {
        let path = check::directory_str(&directory)?;
        changes.directory = Some(utf8_path(&path)?);
    }
    changes.shared_link = body.shared_link;
    if let Some(url) = body.node_base_url {
        check::node_base_url(&url)?;
        changes.node_base_url = Some(url);
    }
    if let Some(order) = body.sort_order {
        if let Some(value) = order {
            check::sort_order(value)?;
        }
        changes.sort_order = Some(order);
    }
    Ok(changes)
}

fn apply_links(
    shared_dir: &std::path::Path,
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
    paths::ensure_shared_link(&path, shared_dir, wants_link(next.shared_link))
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
pub(super) fn wants_link(flag: Option<bool>) -> bool {
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
