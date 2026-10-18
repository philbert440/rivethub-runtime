use chrono::DateTime;
use chrono::Utc;
use wire::gateway::agents::AgentPreset;
use wire::gateway::agents::AgentPresetUpdate;
use wire::harness::HarnessId;

use super::error::AgentError;

pub(crate) fn apply(preset: &mut AgentPreset, changes: AgentPresetUpdate) {
    let AgentPresetUpdate {
        name,
        color,
        harness_id,
        model,
        effort,
        system_prompt,
        node: _,
        directory,
        shared_link,
        node_base_url,
        sort_order,
    } = changes;
    if let Some(name) = name {
        preset.name = name;
    }
    if let Some(color) = color {
        preset.color = color;
    }
    if let Some(harness) = harness_id {
        preset.harness_id = harness.map(HarnessId::from);
    }
    if let Some(model) = model {
        preset.model = model;
    }
    if let Some(effort) = effort {
        preset.effort = effort;
    }
    if let Some(prompt) = system_prompt {
        preset.system_prompt = prompt;
    }
    if let Some(directory) = directory {
        preset.directory = Some(directory);
    }
    if let Some(shared) = shared_link {
        preset.shared_link = Some(shared);
    }
    if let Some(url) = node_base_url {
        preset.node_base_url = Some(url);
    }
    if let Some(order) = sort_order {
        preset.sort_order = order;
    }
}

pub(crate) fn now_millis() -> Result<DateTime<Utc>, AgentError> {
    DateTime::from_timestamp_millis(Utc::now().timestamp_millis()).ok_or(AgentError::Unavailable)
}
