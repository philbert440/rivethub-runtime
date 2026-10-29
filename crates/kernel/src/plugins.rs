use std::collections::BTreeSet;
use std::time::Duration;

use async_trait::async_trait;
use wire::plugin::host::PluginToolList;
use wire::plugin::manifest::{Permission, PluginEventKind, PluginManifest};
use wire::plugin::process::{PluginEvent, PluginToolCall, PluginToolResult};
use wire::plugin::view::ViewInput;
use wire::plugin::{PluginId, PluginNameError, ToolName};

const TOOL_SEPARATOR: &str = "__";
const RESERVED_ENV_PREFIX: &str = "RIVETHUB_";
const FIRST_RESTART: Duration = Duration::from_secs(1);
const LAST_RESTART: Duration = Duration::from_secs(60);

#[derive(Debug, thiserror::Error)]
pub enum PluginToolError {
    #[error("{0} is not a plug-in tool")]
    NotPluginTool(String),
    #[error(transparent)]
    Name(#[from] PluginNameError),
    #[error("plug-in {0} has no tool named {1}")]
    UnknownTool(String, String),
    #[error("plug-in {0} is not running")]
    NotRunning(String),
    #[error("plug-in {plugin} did not answer {tool} in time")]
    Timeout { plugin: String, tool: String },
    #[error("plug-in {plugin} answered {tool} with an invalid body: {detail}")]
    Invalid {
        plugin: String,
        tool: String,
        detail: String,
    },
    #[error("plug-in {plugin} failed {tool}: {detail}")]
    Failed {
        plugin: String,
        tool: String,
        detail: String,
    },
    #[error("the plug-in host is unreachable: {0}")]
    Unreachable(String),
}

#[async_trait]
pub trait PluginTools: Send + Sync {
    async fn tools(&self) -> Result<PluginToolList, PluginToolError>;

    async fn call(
        &self,
        plugin: &PluginId,
        tool: &ToolName,
        call: PluginToolCall,
    ) -> Result<PluginToolResult, PluginToolError>;

    async fn changed(&self, seen: u64) -> Result<u64, PluginToolError>;
}

#[must_use]
pub fn tool_name(plugin: &PluginId, tool: &ToolName) -> String {
    format!("{}{TOOL_SEPARATOR}{}", plugin.as_ref(), tool.as_ref())
}

pub fn split_tool_name(name: &str) -> Result<(PluginId, ToolName), PluginToolError> {
    let (plugin, tool) = name
        .split_once(TOOL_SEPARATOR)
        .ok_or_else(|| PluginToolError::NotPluginTool(name.to_owned()))?;
    Ok((
        PluginId::try_from(plugin.to_owned())?,
        ToolName::try_from(tool.to_owned())?,
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginAccess {
    Own,
    Needs(Permission),
    Refused,
}

#[must_use]
pub fn access(read: bool, path: &str) -> PluginAccess {
    let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    let needs = PluginAccess::Needs;
    match (read, segments.as_slice()) {
        (false, ["api", "plugin", "notify"]) => needs(Permission::NotificationsSend),
        (true, ["api", "plugin", "self"]) | (false, ["api", "plugin", "views", "changed"]) => {
            PluginAccess::Own
        }
        (false, ["api", "memory", "tool", "memory_append"] | ["api", "capture"]) => {
            needs(Permission::MemoryWrite)
        }
        (
            false,
            [
                "api",
                "memory",
                "tool",
                "memory_search" | "memory_browse" | "memory_stats" | "memory_get_full",
            ],
        )
        | (true, ["api", "memory" | "wiki", ..]) => needs(Permission::MemoryRead),
        (true, ["api", "harness-sessions" | "harnesses", ..] | ["api", "catalog"]) => {
            needs(Permission::SessionsRead)
        }
        (false, ["api", "harness-sessions", ..]) => needs(Permission::SessionsControl),
        (true, ["api", "topics" | "sessions", ..]) => needs(Permission::TopicsRead),
        (false, ["api", "topics" | "sessions", ..]) => needs(Permission::TopicsWrite),
        (true, ["api", "tasks", ..]) => needs(Permission::TasksRead),
        (false, ["api", "tasks"]) => needs(Permission::TasksDelegate),
        (true, ["api", "agents", ..]) => needs(Permission::AgentsRead),
        _ => PluginAccess::Refused,
    }
}

#[must_use]
pub const fn event_permission(kind: PluginEventKind) -> Permission {
    match kind {
        PluginEventKind::SessionStarted
        | PluginEventKind::SessionIdle
        | PluginEventKind::SessionEnded
        | PluginEventKind::SessionPlaced => Permission::SessionsRead,
        PluginEventKind::SummaryWritten => Permission::MemoryRead,
        PluginEventKind::TaskDone => Permission::TasksRead,
    }
}

#[must_use]
pub const fn event_kind(event: &PluginEvent) -> Option<PluginEventKind> {
    match event {
        PluginEvent::SessionStarted { .. } => Some(PluginEventKind::SessionStarted),
        PluginEvent::SessionIdle { .. } => Some(PluginEventKind::SessionIdle),
        PluginEvent::SessionEnded { .. } => Some(PluginEventKind::SessionEnded),
        PluginEvent::SessionPlaced { .. } => Some(PluginEventKind::SessionPlaced),
        PluginEvent::SummaryWritten { .. } => Some(PluginEventKind::SummaryWritten),
        PluginEvent::TaskDone { .. } => Some(PluginEventKind::TaskDone),
        PluginEvent::SettingsChanged { .. } => None,
    }
}

#[must_use]
pub fn wants(manifest: &PluginManifest, event: &PluginEvent) -> bool {
    event_kind(event).is_none_or(|kind| manifest.events.contains(&kind))
}

#[must_use]
pub fn pending(requested: &[Permission], granted: &[Permission]) -> Vec<Permission> {
    let granted: BTreeSet<_> = granted.iter().collect();
    let requested: BTreeSet<_> = requested.iter().collect();
    requested
        .into_iter()
        .filter(|permission| !granted.contains(permission))
        .copied()
        .collect()
}

#[must_use]
pub fn narrowed(requested: &[Permission], granted: &[Permission]) -> Vec<Permission> {
    let requested: BTreeSet<_> = requested.iter().collect();
    let kept: BTreeSet<_> = granted
        .iter()
        .filter(|permission| requested.contains(permission))
        .copied()
        .collect();
    kept.into_iter().collect()
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ApprovalError {
    #[error("approve every pending permission: {}", names(.0))]
    Unapproved(Vec<Permission>),
    #[error("not pending: {}", names(.0))]
    NotPending(Vec<Permission>),
}

fn names(permissions: &[Permission]) -> String {
    permissions
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn approve(
    requested: &[Permission],
    granted: &[Permission],
    approved: &[Permission],
) -> Result<Vec<Permission>, ApprovalError> {
    let waiting = pending(requested, granted);
    let extra: Vec<Permission> = approved
        .iter()
        .filter(|permission| !waiting.contains(permission))
        .copied()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    if !extra.is_empty() {
        return Err(ApprovalError::NotPending(extra));
    }
    let missing: Vec<Permission> = waiting
        .iter()
        .filter(|permission| !approved.contains(permission))
        .copied()
        .collect();
    if !missing.is_empty() {
        return Err(ApprovalError::Unapproved(missing));
    }
    let all: BTreeSet<_> = requested.iter().copied().collect();
    Ok(all.into_iter().collect())
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{path}: {message}")]
pub struct ManifestProblem {
    pub path: String,
    pub message: String,
}

fn problem(path: String, message: &str) -> ManifestProblem {
    ManifestProblem {
        path,
        message: message.to_owned(),
    }
}

fn first_repeat<'a>(names: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    let mut seen = BTreeSet::new();
    names.into_iter().find(|name| !seen.insert(*name))
}

pub fn check_manifest(manifest: &PluginManifest) -> Result<(), ManifestProblem> {
    if manifest.run.command.first().is_none_or(String::is_empty) {
        return Err(problem("run.command".into(), "is empty"));
    }
    if let Some(name) = manifest
        .run
        .env
        .keys()
        .find(|name| name.starts_with(RESERVED_ENV_PREFIX))
    {
        return Err(problem(
            format!("run.env.{name}"),
            "RIVETHUB_ names are reserved",
        ));
    }
    for (index, field) in manifest.settings.iter().enumerate() {
        if matches!(field.input, ViewInput::File { .. }) {
            return Err(problem(
                format!("settings[{index}].input"),
                "a setting cannot be a file",
            ));
        }
    }
    let contributes = &manifest.contributes;
    let repeats = [
        (
            "settings",
            first_repeat(manifest.settings.iter().map(|field| field.key.as_str())),
        ),
        (
            "tools",
            first_repeat(manifest.tools.iter().map(|tool| tool.name.as_ref())),
        ),
        (
            "jobs",
            first_repeat(manifest.jobs.iter().map(|job| job.id.as_ref())),
        ),
        (
            "contributes",
            first_repeat(
                contributes
                    .pages
                    .iter()
                    .map(|page| page.id.as_ref())
                    .chain(contributes.topic_cards.iter().map(|card| card.id.as_ref()))
                    .chain(
                        contributes
                            .session_actions
                            .iter()
                            .map(|action| action.id.as_ref()),
                    ),
            ),
        ),
    ];
    for (path, repeat) in repeats {
        if let Some(name) = repeat {
            return Err(problem(path.into(), &format!("{name} is declared twice")));
        }
    }
    for (index, kind) in manifest.events.iter().enumerate() {
        let needed = event_permission(*kind);
        if !manifest.permissions.contains(&needed) {
            return Err(problem(
                format!("events[{index}]"),
                &format!("needs the {needed} permission"),
            ));
        }
    }
    Ok(())
}

#[must_use]
pub fn restart_delay(failures: u32) -> Duration {
    let doublings = failures.saturating_sub(1).min(6);
    FIRST_RESTART
        .saturating_mul(1 << doublings)
        .min(LAST_RESTART)
}
