use crate::model::{HeartbeatEntry, RuntimeSection};
use crate::report::ValidationResult;

use super::required_string;

pub(super) fn validate(out: &mut ValidationResult, section: &RuntimeSection) {
    required_string(
        out,
        "runtime.workspace",
        section.workspace.as_deref(),
        "Missing required field \"runtime.workspace\"",
    );
    required_string(
        out,
        "runtime.default_agent",
        section.default_agent.as_deref(),
        "Missing required field \"runtime.default_agent\"",
    );
    if section.turn_timeout == Some(0) {
        out.error(
            "runtime.turn_timeout",
            "\"runtime.turn_timeout\" must be a positive number (seconds)",
        );
    }
    for (index, heartbeat) in section.heartbeats.iter().enumerate() {
        heartbeat_entry(out, index, heartbeat);
    }
}

fn heartbeat_entry(out: &mut ValidationResult, index: usize, heartbeat: &HeartbeatEntry) {
    let prefix = format!("runtime.heartbeats[{index}]");
    if heartbeat.agent.as_deref().is_none_or(str::is_empty) {
        out.error(
            format!("{prefix}.agent"),
            "Heartbeat requires a string \"agent\" field",
        );
    }
    if heartbeat.schedule.is_none() {
        out.error(
            format!("{prefix}.schedule"),
            "Heartbeat requires a \"schedule\" field (e.g., \"30m\", \"1h\")",
        );
    }
    if heartbeat.prompt.as_deref().is_none_or(str::is_empty) {
        out.error(
            format!("{prefix}.prompt"),
            "Heartbeat requires a string \"prompt\" field",
        );
    }
    if let Some(quiet_hours) = &heartbeat.quiet_hours {
        hour_field(out, &prefix, "start", quiet_hours.start);
        hour_field(out, &prefix, "end", quiet_hours.end);
    }
}

fn hour_field(out: &mut ValidationResult, prefix: &str, name: &str, value: Option<u8>) {
    if value.is_none_or(|value| value > 23) {
        out.error(
            format!("{prefix}.quiet_hours.{name}"),
            format!("\"quiet_hours.{name}\" must be a number 0-23"),
        );
    }
}
