use std::collections::BTreeMap;

use crate::enums::{HarnessId, ModelsMode, listed};
use crate::model::{EvalSection, HarnessEntry, HarnessSlot, TasksSection};
use crate::report::ValidationResult;

pub(super) fn validate(out: &mut ValidationResult, section: &TasksSection) {
    if let Some(eval) = &section.eval {
        eval_section(out, eval);
    }
    if let Some(harnesses) = &section.harnesses {
        harness_map(out, harnesses);
    }
}

fn eval_section(out: &mut ValidationResult, eval: &EvalSection) {
    if let Some(verifier) = &eval.verifier
        && verifier.executor.is_none()
    {
        out.error(
            "tasks.eval.verifier.executor",
            "\"tasks.eval.verifier.executor\" must be 'chat-loop' or 'harness-session'",
        );
    }
    if let Some(escalation) = &eval.escalation
        && escalation.channel.is_none()
    {
        out.error(
            "tasks.eval.escalation.channel",
            "\"tasks.eval.escalation.channel\" must be a string",
        );
    }
}

fn harness_map(out: &mut ValidationResult, harnesses: &BTreeMap<HarnessSlot, HarnessEntry>) {
    for (id, harness) in harnesses {
        if let HarnessSlot::Unknown(id) = id {
            out.warning(
                format!("tasks.harnesses.{id}"),
                format!(
                    "Unknown harness id \"{id}\" — expected one of: {}",
                    listed::<HarnessId>()
                ),
            );
        }
        harness_entry(out, id, harness);
    }
}

fn harness_entry(out: &mut ValidationResult, id: &HarnessSlot, harness: &HarnessEntry) {
    let shown = id.as_text();
    let prefix = format!("tasks.harnesses.{shown}");
    sheet_list(out, &prefix, "models", harness.models.as_ref());
    sheet_list(out, &prefix, "efforts", harness.efforts.as_ref());
    allowed_tools(out, &prefix, harness);
    models_mode_warnings(out, &prefix, harness);
    if !matches!(id, HarnessSlot::Known(HarnessId::ClaudeCode)) {
        let message = format!(
            "\"isolation\" / \"allowed_tools\" are read by the claude-code executor only; ignored for \"{shown}\""
        );
        if harness.isolation.is_some() {
            out.warning(format!("{prefix}.isolation"), message.clone());
        }
        if harness.allowed_tools.is_some() {
            out.warning(format!("{prefix}.allowed_tools"), message);
        }
    }
}

fn sheet_list<T>(out: &mut ValidationResult, prefix: &str, label: &str, entries: Option<&Vec<T>>) {
    let Some(entries) = entries else {
        return;
    };
    if entries.is_empty() {
        out.warning(
            format!("{prefix}.{label}"),
            format!("\"{prefix}.{label}\" is empty and will be ignored (sheet list is kept)"),
        );
    }
}

fn allowed_tools(out: &mut ValidationResult, prefix: &str, harness: &HarnessEntry) {
    let Some(rules) = &harness.allowed_tools else {
        return;
    };
    if rules.iter().any(|rule| !valid_rule(rule)) {
        out.error(
            format!("{prefix}.allowed_tools"),
            format!(
                "\"{prefix}.allowed_tools\" must be an array of permission rules: non-empty strings of at most 200 characters, not starting with \"-\", with no control characters"
            ),
        );
    }
}

fn valid_rule(rule: &str) -> bool {
    !rule.is_empty()
        && rule.len() <= 200
        && !rule.starts_with('-')
        && !rule.chars().any(char::is_control)
}

fn models_mode_warnings(out: &mut ValidationResult, prefix: &str, harness: &HarnessEntry) {
    let Some(mode) = harness.models_mode else {
        return;
    };
    let lists_present = harness.models.is_some() || harness.efforts.is_some();
    if mode == ModelsMode::Discover && lists_present {
        out.warning(
            format!("{prefix}.models_mode"),
            format!(
                "\"{prefix}.models_mode\" is 'discover', so \"models\" / \"efforts\" are ignored"
            ),
        );
    }
    if matches!(mode, ModelsMode::Replace | ModelsMode::Merge) && !lists_present {
        out.warning(
            format!("{prefix}.models_mode"),
            format!(
                "\"{prefix}.models_mode\" is '{mode}' but no \"models\" or \"efforts\" list is set — the discovered list is kept"
            ),
        );
    }
}
