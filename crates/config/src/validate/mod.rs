mod agents;
mod channels;
mod deployment;
mod gateway;
mod memory;
mod mesh;
mod providers;
mod runtime;
mod tasks;

use crate::model::{AgentEntry, Config};
use crate::report::ValidationResult;

#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::Display)]
enum RemovedKey {
    #[strum(
        serialize = "Provider fallback was removed in the AI SDK migration. Remove \"runtime.fallbacks\" from your config."
    )]
    RuntimeFallbacks,
    #[strum(
        serialize = "The coding-pipeline plugin was removed. Remove \"runtime.coding_pipeline\"; use delegate_task (and the upcoming task engine) instead."
    )]
    RuntimeCodingPipeline,
    #[strum(
        serialize = "Per-agent fallback chains were removed in the AI SDK migration. Remove \"fallbacks\" from this agent."
    )]
    AgentFallbacks,
    #[strum(
        serialize = "The background review loop was removed — compaction is the single consolidation path. Remove \"review_endpoint\"; for delegation-event persistence set \"delegation_tracking: true\"."
    )]
    PostgresReviewEndpoint,
    #[strum(
        serialize = "The background review loop was removed. Remove \"review_model\" from memory.postgres."
    )]
    PostgresReviewModel,
    #[strum(
        serialize = "The background review loop was removed. Remove \"review_api_key\" from memory.postgres."
    )]
    PostgresReviewApiKey,
}

impl RemovedKey {
    fn from_path(path: &str) -> Option<Self> {
        match path {
            "runtime.fallbacks" => Some(Self::RuntimeFallbacks),
            "runtime.coding_pipeline" => Some(Self::RuntimeCodingPipeline),
            "memory.postgres.review_endpoint" => Some(Self::PostgresReviewEndpoint),
            "memory.postgres.review_model" => Some(Self::PostgresReviewModel),
            "memory.postgres.review_api_key" => Some(Self::PostgresReviewApiKey),
            other if agent_fallbacks(other) => Some(Self::AgentFallbacks),
            _ => None,
        }
    }
}

fn agent_fallbacks(path: &str) -> bool {
    path.split('.').count() == 3 && path.starts_with("agents.") && path.ends_with(".fallbacks")
}

pub const PLUGINS_IGNORED: &str = "\"plugins\" is ignored — the Rust runtime loads no plugins.";
pub const WORKFLOWS_IGNORED: &str = "\"workflows\" is ignored — workflows moved out of the runtime into the rivethub-workflows package.";

#[derive(Debug, Default, Clone, Copy)]
pub struct Presence {
    pub plugins: bool,
    pub workflows: bool,
}

pub fn validate(config: &Config, ignored: &[String], presence: &Presence) -> ValidationResult {
    let mut out = ValidationResult::default();
    ignored_findings(config, ignored, &mut out);
    if presence.plugins {
        out.warning("plugins", PLUGINS_IGNORED);
    }
    if presence.workflows {
        out.warning("workflows", WORKFLOWS_IGNORED);
    }
    sections(config, &mut out);
    cross_references(config, &mut out);
    out
}

fn sections(config: &Config, out: &mut ValidationResult) {
    match &config.runtime {
        Some(section) => runtime::validate(out, section),
        None => out.error("runtime", "Missing required section \"runtime\""),
    }
    match &config.agents {
        Some(agents) => agents::validate(out, agents),
        None => out.error(
            "agents",
            "Missing required section \"agents\" — define at least one agent",
        ),
    }
    match &config.providers {
        Some(_) => {}
        None => out.error(
            "providers",
            "Missing required section \"providers\" — define at least one provider",
        ),
    }
    providers::validate(out, config.providers.as_ref());
    if let Some(channels) = &config.channels {
        channels::validate(out, channels);
    }
    if let Some(memory) = &config.memory {
        memory::validate(out, memory);
    }
    if let Some(section) = &config.deployment {
        deployment::validate(out, *section);
    }
    if let Some(section) = &config.mesh {
        mesh::validate(out, section);
    }
    if let Some(section) = &config.gateway {
        gateway::validate(out, section);
    }
    if let Some(section) = &config.tasks {
        tasks::validate(out, section);
    }
}

fn ignored_findings(config: &Config, ignored: &[String], out: &mut ValidationResult) {
    for raw in ignored {
        let path = normalized_path(raw);
        if let Some(message) = RemovedKey::from_path(&path).map(|removed| removed.to_string()) {
            out.error(&path, message);
        } else if let Some(message) = ignored_message(config, &path) {
            out.warning(&path, message);
        }
    }
}

fn normalized_path(raw: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    for segment in raw.split('.') {
        let segment = normalized_segment(segment);
        if segment.is_empty() {
            continue;
        }
        if let Some(previous) = parts.last_mut()
            && segment.bytes().all(|byte| byte.is_ascii_digit())
        {
            previous.push('[');
            previous.push_str(&segment);
            previous.push(']');
            continue;
        }
        parts.push(segment);
    }
    parts.join(".")
}

fn normalized_segment(segment: &str) -> String {
    match segment.rsplit_once('?') {
        Some((name, index))
            if !index.is_empty() && index.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            format!("{name}[{index}]")
        }
        _ => segment.replace('?', ""),
    }
}

fn ignored_message(config: &Config, path: &str) -> Option<String> {
    if !path.contains('.') {
        return top_level_message(path);
    }
    let (head, rest) = path.split_once('.')?;
    let segments: Vec<&str> = rest.split('.').collect();
    match head {
        "runtime" => match segments.as_slice() {
            [key] => Some(format!("Unknown runtime key \"{key}\"")),
            [parent, key] if parent.starts_with("heartbeats[") => {
                Some(format!("Unknown heartbeat key \"{key}\""))
            }
            _ => None,
        },
        "agents" => match segments.as_slice() {
            [_id, key] => Some(format!("Unknown agent key \"{key}\"")),
            [_id, "tools", key] => Some(format!(
                "Unknown tools filter key \"{key}\" — expected \"exclude\" or \"include\""
            )),
            _ => None,
        },
        "providers" => match segments.as_slice() {
            [id, key] => {
                let kind = config.provider_kind(id)?;
                Some(format!(
                    "Unknown key \"{key}\" for provider type \"{kind}\""
                ))
            }
            _ => None,
        },
        "channels" => match segments.as_slice() {
            ["agent", key] => Some(format!("Unknown key \"{key}\" for channel type \"agent\"")),
            _ => None,
        },
        "memory" => match segments.as_slice() {
            [key] => Some(format!(
                "Unknown memory key \"{key}\" — supported: \"postgres\" / \"sqlite\" (backend), \"capture\" (write-path options)"
            )),
            ["postgres", key] => Some(format!("Unknown memory.postgres key \"{key}\"")),
            ["postgres", "embedded", key] => {
                Some(format!("Unknown memory.postgres.embedded key \"{key}\""))
            }
            ["sqlite", key] => Some(format!("Unknown memory.sqlite key \"{key}\"")),
            ["capture", key] => Some(format!("Unknown memory.capture key \"{key}\"")),
            ["capture", "redaction", key] => {
                Some(format!("Unknown memory.capture.redaction key \"{key}\""))
            }
            _ => None,
        },
        "deployment" => match segments.as_slice() {
            [key] => Some(format!("Unknown deployment key \"{key}\"")),
            _ => None,
        },
        "den" => match segments.as_slice() {
            [key] => Some(format!("Unknown den key \"{key}\"")),
            ["terminal", key] => Some(format!("Unknown den.terminal key \"{key}\"")),
            ["devices", key] => Some(format!("Unknown den.devices key \"{key}\"")),
            _ => None,
        },
        "tasks" => match segments.as_slice() {
            [key] => Some(format!("Unknown tasks key \"{key}\"")),
            ["eval", key] => Some(format!("Unknown tasks.eval key \"{key}\"")),
            ["harnesses", _id, key] => Some(format!("Unknown key \"{key}\"")),
            _ => None,
        },
        _ => None,
    }
}

fn top_level_message(path: &str) -> Option<String> {
    match path {
        "plugins" | "workflows" | "mcp" | "providers" => None,
        _ => Some(format!(
            "Unknown top-level key \"{path}\" — will be ignored"
        )),
    }
}

fn cross_references(config: &Config, out: &mut ValidationResult) {
    let agent_names: Vec<&str> = config
        .agents
        .iter()
        .flatten()
        .map(|(name, _)| name.as_str())
        .collect();
    let provider_names: Vec<&str> = config
        .providers
        .iter()
        .flatten()
        .map(|(name, _)| name.as_str())
        .collect();
    if let Some(runtime) = &config.runtime
        && let Some(default_agent) = &runtime.default_agent
        && !default_agent.is_empty()
        && !agent_names.contains(&default_agent.as_str())
    {
        out.error(
            "runtime.default_agent",
            format!(
                "Default agent \"{default_agent}\" is not defined in [agents]. Available: {}",
                available_list(&agent_names)
            ),
        );
    }
    if let Some(agents) = &config.agents {
        for (name, agent) in agents {
            agent_provider_cross_ref(out, name, agent, &provider_names);
        }
    }
    if let Some(runtime) = &config.runtime {
        for (index, heartbeat) in runtime.heartbeats.iter().enumerate() {
            let Some(agent) = heartbeat.agent.as_deref() else {
                continue;
            };
            if agent.is_empty() || agent_names.contains(&agent) {
                continue;
            }
            out.error(
                format!("runtime.heartbeats[{index}].agent"),
                format!(
                    "Heartbeat agent \"{agent}\" is not defined in [agents]. Available: {}",
                    available_list(&agent_names)
                ),
            );
        }
    }
}

fn agent_provider_cross_ref(
    out: &mut ValidationResult,
    name: &str,
    agent: &AgentEntry,
    provider_names: &[&str],
) {
    let Some(provider) = agent.provider.as_deref() else {
        return;
    };
    if provider_names.contains(&provider) {
        return;
    }
    out.error(
        format!("agents.{name}.provider"),
        format!(
            "Provider \"{provider}\" referenced by agent \"{name}\" is not defined in [providers]. Available: {}",
            available_list(provider_names)
        ),
    );
}

fn available_list(names: &[&str]) -> String {
    if names.is_empty() {
        "(none)".to_owned()
    } else {
        names.join(", ")
    }
}

pub(super) fn required_string(
    out: &mut ValidationResult,
    path: &str,
    value: Option<&str>,
    message: &str,
) {
    if value.is_none_or(str::is_empty) {
        out.error(path, message);
    }
}

pub(super) fn positive<T: Copy + Default + PartialEq>(
    out: &mut ValidationResult,
    path: &str,
    value: Option<T>,
    message: &str,
) {
    if value == Some(T::default()) {
        out.error(path, message);
    }
}

pub(super) fn non_empty_entries(
    out: &mut ValidationResult,
    path: impl Into<String>,
    entries: Option<&Vec<String>>,
    message: impl Into<String>,
) {
    if let Some(entries) = entries
        && entries.iter().any(String::is_empty)
    {
        out.error(path, message);
    }
}

pub(super) fn hardcoded_key_warning(out: &mut ValidationResult, path: &str, name: &str, key: &str) {
    if key.contains("${") || !looks_like_api_key(key) {
        return;
    }
    let env_name = name.to_uppercase().replace('-', "_");
    out.warning(
        path,
        format!(
            "Provider \"{name}\" appears to have a hardcoded API key — use environment variables instead (e.g., ${{{env_name}_API_KEY}})"
        ),
    );
}

fn looks_like_api_key(key: &str) -> bool {
    key.strip_prefix("sk-").is_some_and(|rest| {
        rest.len() >= 20
            && rest
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    }) || key.strip_prefix("xai-").is_some_and(|rest| {
        rest.len() >= 20 && rest.bytes().all(|byte| byte.is_ascii_alphanumeric())
    }) || key.strip_prefix("AIza").is_some_and(|rest| {
        rest.len() >= 30
            && rest
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    }) || (key.len() >= 64
        && key
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)))
}
