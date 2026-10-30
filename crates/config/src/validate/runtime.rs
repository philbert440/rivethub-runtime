use crate::model::RuntimeSection;
use crate::report::ValidationResult;

use super::{required_path, required_string};

pub(super) fn validate(out: &mut ValidationResult, section: &RuntimeSection) {
    required_path(
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
}
