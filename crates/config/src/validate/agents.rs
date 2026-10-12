use std::collections::BTreeMap;

use crate::model::{AgentEntry, ToolsSection};
use crate::report::ValidationResult;

pub(super) fn validate(out: &mut ValidationResult, agents: &BTreeMap<String, AgentEntry>) {
    if agents.is_empty() {
        out.error("agents", "\"agents\" is empty — define at least one agent");
        return;
    }
    for (name, agent) in agents {
        agent_fields(out, name, agent);
    }
}

fn agent_fields(out: &mut ValidationResult, name: &str, agent: &AgentEntry) {
    super::required_string(
        out,
        &format!("agents.{name}.provider"),
        agent.provider.as_deref(),
        &format!("Agent \"{name}\" is missing required field \"provider\""),
    );
    if let Some(tools) = &agent.tools {
        tools_filters(out, name, tools);
    }
}

fn tools_filters(out: &mut ValidationResult, name: &str, tools: &ToolsSection) {
    if tools.exclude.is_some() && tools.include.is_some() {
        out.warning(
            format!("agents.{name}.tools"),
            format!(
                "Agent \"{name}\" has both \"exclude\" and \"include\" — include takes precedence"
            ),
        );
    }
}
