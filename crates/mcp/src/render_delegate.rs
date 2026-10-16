use crate::Error;
use kernel::tasks::{AgentDirectory, DelegateOutcome};
use std::fmt::Write as _;

pub(crate) fn agents(directory: &AgentDirectory) -> Result<String, Error> {
    let mut lines = Vec::new();
    for preset in &directory.presets {
        let mut line = format!("- {} (agent:", preset.name);
        if let Some(harness) = preset.harness {
            write!(line, " {harness}")?;
        }
        if let Some(node) = &preset.node {
            write!(line, " on {node}")?;
            if preset.local {
                line.push_str(" — this node");
            }
        }
        if let Some(path) = &preset.directory {
            write!(line, ", dir {path}")?;
        }
        line.push(')');
        lines.push(line);
    }
    if !directory.runtime.is_empty() {
        lines.push("\nRuntime agents (mesh):".to_owned());
        for agent in &directory.runtime {
            let nodes = agent
                .nodes
                .iter()
                .map(|node| node.id.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            lines.push(format!("- {} ({nodes})", agent.id));
        }
    }
    lines.push("\nto_agent accepts a preset name or id, or a runtime agent id (agent@node pins the node).\n".to_owned());
    Ok(lines.join("\n"))
}

pub(crate) fn task(outcome: &DelegateOutcome) -> Result<String, Error> {
    let mut text = format!("[{}]", outcome.status);
    if let Some(result) = &outcome.result {
        text.push(' ');
        text.push_str(&result.summary);
        if let Some(output) = &result.output {
            text.push_str("\n\n");
            text.push_str(output);
        }
    }
    if let Some(error) = &outcome.error {
        write!(text, "\n{error}")?;
    }
    if outcome.timed_out {
        text.push_str("\nTimed out waiting for the delegate.");
    }
    write!(text, "\n\nTask: {}", outcome.id)?;
    Ok(text)
}
