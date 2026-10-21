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

pub(crate) fn task(outcome: &DelegateOutcome, elapsed: std::time::Duration) -> String {
    let status = if outcome.timed_out {
        "timeout".to_owned()
    } else {
        outcome.status.to_string()
    };
    let mut meta = vec![format!("{}ms", elapsed.as_millis())];
    if let Some(total) = outcome
        .result
        .as_ref()
        .and_then(|result| result.usage.total_tokens)
    {
        meta.push(format!("tokens: {total}"));
    }
    let meta_line = format!("\n\n---\n_Delegation [{status}]: {}_", meta.join(" | "));
    if outcome.status == wire::task::TaskStatus::Completed && !outcome.timed_out {
        let response = outcome
            .result
            .as_ref()
            .and_then(|result| {
                result
                    .output
                    .clone()
                    .filter(|output| !output.is_empty())
                    .or_else(|| Some(result.summary.clone()).filter(|summary| !summary.is_empty()))
            })
            .unwrap_or_else(|| "[no response from remote agent]".to_owned());
        return format!("{response}{meta_line}");
    }
    let reason = outcome
        .error
        .clone()
        .or_else(|| {
            outcome
                .result
                .as_ref()
                .and_then(|result| result.error.clone())
        })
        .or_else(|| outcome.result.as_ref().map(|result| result.summary.clone()))
        .unwrap_or_default();
    format!("[{status}] {reason}{meta_line}")
}
