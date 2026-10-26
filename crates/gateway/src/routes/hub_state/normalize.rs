use std::collections::BTreeMap;
use std::collections::BTreeSet;

use thiserror::Error;
use wire::gateway::hub_state::HubLastSession;
use wire::gateway::hub_state::HubProject;
use wire::gateway::hub_state::HubProjectDefaults;
use wire::gateway::hub_state::HubState;

const PROJECTS: usize = 200;
const MEMBERSHIP: usize = 2000;
const NAMES: usize = 500;
const ARCHIVED: usize = 1000;
const PINNED: usize = 500;
const HIDDEN: usize = 1000;
const MODES: usize = 500;
const TEXT: usize = 200;
const DIRECTORY: usize = 4096;
const KEY: usize = 1024;
const SEPARATOR: &str = "::";

#[derive(Debug, Error, PartialEq, Eq)]
#[error("too many {field} entries (max {max})")]
pub(super) struct OverCap {
    field: &'static str,
    max: usize,
}

pub(super) fn normalize(state: HubState) -> Result<HubState, OverCap> {
    within("projects", state.projects.len(), PROJECTS)?;
    within("membership", state.membership.len(), MEMBERSHIP)?;
    within("names", state.names.len(), NAMES)?;
    within("archived", state.archived.len(), ARCHIVED)?;
    within("pinned", state.pinned.len(), PINNED)?;
    within("hidden", state.hidden.len(), HIDDEN)?;
    within("modes", state.modes.len(), MODES)?;

    let mut ids = BTreeSet::new();
    let projects: Vec<HubProject> = state
        .projects
        .into_iter()
        .filter_map(project)
        .filter(|project| ids.insert(project.id.clone()))
        .collect();

    let membership = state
        .membership
        .into_iter()
        .filter(|(key, _)| session_key(key))
        .filter_map(|(key, id)| text(&id, TEXT).map(|id| (key, id)))
        .filter(|(_, id)| ids.contains(id))
        .collect();
    let names = state
        .names
        .into_iter()
        .filter(|(key, _)| session_key(key))
        .filter_map(|(key, name)| text(&name, TEXT).map(|name| (key, name)))
        .collect::<BTreeMap<_, _>>();
    let modes = state
        .modes
        .into_iter()
        .filter(|(key, _)| session_key(key))
        .collect();

    Ok(HubState {
        default_project_id: state
            .default_project_id
            .and_then(|id| text(&id, TEXT))
            .filter(|id| ids.contains(id)),
        projects,
        membership,
        names,
        archived: keys(state.archived),
        pinned: keys(state.pinned),
        hidden: keys(state.hidden),
        modes,
        last_session: state
            .last_session
            .filter(|last: &HubLastSession| session_key(&last.key) && last.at >= 0),
    })
}

fn within(field: &'static str, len: usize, max: usize) -> Result<(), OverCap> {
    if len > max {
        return Err(OverCap { field, max });
    }
    Ok(())
}

fn project(raw: HubProject) -> Option<HubProject> {
    Some(HubProject {
        id: text(&raw.id, TEXT)?,
        name: text(&raw.name, TEXT)?,
        order: raw.order,
        created_at: raw.created_at.max(0),
        defaults: raw.defaults.and_then(defaults),
    })
}

fn defaults(raw: HubProjectDefaults) -> Option<HubProjectDefaults> {
    let defaults = HubProjectDefaults {
        agent_id: raw.agent_id.and_then(|id| text(&id, TEXT)),
        model: raw.model.and_then(|model| text(&model, TEXT)),
        effort: raw.effort,
        harness_id: raw.harness_id,
        node: raw.node.and_then(|node| text(&node, TEXT)),
        directory: raw
            .directory
            .and_then(|directory| text(&directory, DIRECTORY))
            .filter(|directory| directory.starts_with('/')),
    };
    (defaults != HubProjectDefaults::default()).then_some(defaults)
}

fn keys(raw: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    raw.into_iter()
        .filter(|key| session_key(key))
        .filter(|key| seen.insert(key.clone()))
        .collect()
}

fn session_key(key: &str) -> bool {
    key.chars().count() <= KEY
        && key
            .split_once(SEPARATOR)
            .is_some_and(|(node, session)| !node.is_empty() && !session.is_empty())
}

fn text(value: &str, max: usize) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty() && trimmed.chars().count() <= max).then(|| trimmed.to_owned())
}
