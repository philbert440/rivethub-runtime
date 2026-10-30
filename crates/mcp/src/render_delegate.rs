use crate::Error;
use kernel::tasks::{AgentDirectory, DelegateOutcome, TaskStatus};
use std::fmt::Write as _;

pub(crate) fn agents(directory: &AgentDirectory) -> Result<String, Error> {
    let mut lines = Vec::new();
    for preset in &directory.presets {
        let mut line = format!("- {} (agent:", preset.name);
        if let Some(harness) = &preset.harness {
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
        "still running".to_owned()
    } else {
        outcome.status.to_string()
    };
    let mut meta = vec![format!("{}ms", elapsed.as_millis())];
    if let Some(total) = outcome.tokens {
        meta.push(format!("tokens: {total}"));
    }
    let session = outcome.child.as_deref().unwrap_or("not started");
    let footer = format!(
        "\n\n---\n_Delegation [{status}]: {}_\ntask {} · session {session} · node {}",
        meta.join(" | "),
        outcome.id,
        outcome.node
    );
    if outcome.timed_out {
        return format!(
            "[still running] task {} is {} and keeps running; its result comes back to this session when it finishes{footer}",
            outcome.id, outcome.status
        );
    }
    if outcome.status == TaskStatus::Completed {
        let response = outcome
            .summary
            .clone()
            .filter(|summary| !summary.is_empty())
            .unwrap_or_else(|| "[no response from the delegate]".to_owned());
        return format!("{response}{footer}");
    }
    let reason = outcome
        .error
        .clone()
        .or_else(|| outcome.summary.clone())
        .unwrap_or_default();
    format!("[{status}] {reason}{footer}")
}
