use std::path::Path;

use crate::report::ValidationResult;

pub(super) fn required_string(
    out: &mut ValidationResult,
    path: &str,
    value: Option<&str>,
    message: &str,
) {
    if value.is_none_or(str::is_empty) {
        out.error(path, message);
    }
}

pub(super) fn required_path(
    out: &mut ValidationResult,
    path: &str,
    value: Option<&Path>,
    message: &str,
) {
    if value.is_none_or(|value| value.as_os_str().is_empty()) {
        out.error(path, message);
    }
}

pub(super) fn positive<T: Copy + Default + PartialEq>(
    out: &mut ValidationResult,
    path: &str,
    value: Option<T>,
    message: &str,
) {
    if value == Some(T::default()) {
        out.error(path, message);
    }
}

pub(super) fn non_empty_entries(
    out: &mut ValidationResult,
    path: impl Into<String>,
    entries: Option<&Vec<String>>,
    message: impl Into<String>,
) {
    if let Some(entries) = entries
        && entries.iter().any(String::is_empty)
    {
        out.error(path, message);
    }
}

pub(super) fn hardcoded_key_warning(out: &mut ValidationResult, path: &str, name: &str, key: &str) {
    if key.contains("${") || !looks_like_api_key(key) {
        return;
    }
    let env_name = name.to_uppercase().replace('-', "_");
    out.warning(
        path,
        format!(
            "Provider \"{name}\" appears to have a hardcoded API key — use environment variables instead (e.g., ${{{env_name}_API_KEY}})"
        ),
    );
}

pub(super) fn looks_like_api_key(key: &str) -> bool {
    key.strip_prefix("sk-").is_some_and(|rest| {
        rest.len() >= 20
            && rest
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    }) || key.strip_prefix("xai-").is_some_and(|rest| {
        rest.len() >= 20 && rest.bytes().all(|byte| byte.is_ascii_alphanumeric())
    }) || key.strip_prefix("AIza").is_some_and(|rest| {
        rest.len() >= 30
            && rest
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    }) || (key.len() >= 64
        && key
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)))
}
