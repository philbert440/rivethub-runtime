use std::sync::LazyLock;

use regress::Regex;

use crate::model::{
    EmbeddedSection, MemorySection, PostgresSection, RedactionSection, SqliteSection,
};
use crate::report::ValidationResult;

const NESTED_QUANTIFIER_PATTERN: &str =
    r"\((?:[^\\)]|\\.)*[+*](?:[^\\)]|\\.)*\)(?:[+*?]|\{\d+,?\d*\})";

static NESTED_QUANTIFIERS: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(NESTED_QUANTIFIER_PATTERN).ok());

pub(super) fn validate(out: &mut ValidationResult, memory: &MemorySection) {
    if memory.postgres.is_some() && memory.sqlite.is_some() {
        out.error(
            "memory",
            "\"postgres\" and \"sqlite\" cannot both be set — pick one memory backend",
        );
    }
    if let Some(postgres) = &memory.postgres {
        postgres_section(out, postgres);
    }
    if let Some(sqlite) = &memory.sqlite {
        sqlite_section(out, sqlite);
    }
    redaction_section(out, &memory.capture.redaction);
}

fn postgres_section(out: &mut ValidationResult, postgres: &PostgresSection) {
    if postgres.embedded.is_some() && postgres.connection_string.is_some() {
        out.error(
            "memory.postgres",
            "\"embedded\" and \"connection_string\" cannot both be set — embedded owns the loopback URL",
        );
    }
    embed_api_key(postgres, out);
    embed_token_command(postgres, out);
    super::positive(
        out,
        "memory.postgres.embed_token_ttl_ms",
        postgres.embed_token_ttl_ms,
        "memory.postgres embed_token_ttl_ms must be a positive number",
    );
    super::positive(
        out,
        "memory.postgres.embed_token_command_timeout_ms",
        postgres.embed_token_command_timeout_ms,
        "memory.postgres embed_token_command_timeout_ms must be a positive number",
    );
    if postgres
        .embed_expected_dims
        .is_some_and(|dims| dims != 1024)
    {
        out.error(
            "memory.postgres.embed_expected_dims",
            "\"embed_expected_dims\" must equal the embedding column width (1024); any other value bricks halfvec inserts and vector search",
        );
    }
    if let Some(embedded) = &postgres.embedded {
        embedded_section(out, embedded);
    }
}

fn embed_api_key(postgres: &PostgresSection, out: &mut ValidationResult) {
    let Some(key) = &postgres.embed_api_key else {
        return;
    };
    if key.contains("${") || key.is_empty() || !looks_like_key(key) {
        return;
    }
    out.warning(
        "memory.postgres.embed_api_key",
        "memory.postgres.embed_api_key appears hardcoded — prefer environment variables or embed_token_command",
    );
}

fn looks_like_key(key: &str) -> bool {
    key.strip_prefix("sk-").is_some_and(|rest| {
        rest.len() >= 20
            && rest
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    }) || (key.len() >= 64
        && key
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)))
}

fn embed_token_command(postgres: &PostgresSection, out: &mut ValidationResult) {
    if let Some(entries) = &postgres.embed_token_command
        && entries.is_empty()
    {
        out.error(
            "memory.postgres.embed_token_command",
            "memory.postgres embed_token_command must be a non-empty argv array of strings",
        );
    }
    if postgres.embed_api_key.is_some() && postgres.embed_token_command.is_some() {
        out.warning(
            "memory.postgres.embed_token_command",
            "memory.postgres has both embed_api_key and embed_token_command — embed_token_command wins",
        );
    }
}

fn embedded_section(out: &mut ValidationResult, embedded: &EmbeddedSection) {
    if embedded.data_dir.as_deref() == Some("") {
        out.error(
            "memory.postgres.embedded.data_dir",
            "\"data_dir\" must be a non-empty string",
        );
    }
    if embedded.port == 0 {
        out.error(
            "memory.postgres.embedded.port",
            "\"port\" must be an integer between 1 and 65535",
        );
    }
    if embedded.max_connections == 0 {
        out.error(
            "memory.postgres.embedded.max_connections",
            "\"max_connections\" must be a positive integer",
        );
    }
}

fn sqlite_section(out: &mut ValidationResult, sqlite: &SqliteSection) {
    match sqlite.path.as_deref() {
        Some("") => out.error(
            "memory.sqlite.path",
            "\"memory.sqlite.path\" must be a non-empty file path",
        ),
        None => out.error(
            "memory.sqlite.path",
            "\"memory.sqlite.path\" is required (file path or \":memory:\")",
        ),
        Some(_) => {}
    }
}

fn redaction_section(out: &mut ValidationResult, redaction: &RedactionSection) {
    for (index, pattern) in redaction.patterns.iter().enumerate() {
        pattern_finding(out, index, pattern);
    }
}

fn pattern_finding(out: &mut ValidationResult, index: usize, pattern: &str) {
    let path = format!("memory.capture.redaction.patterns[{index}]");
    if pattern.is_empty() {
        out.error(
            path,
            "Each pattern must be a non-empty string (JS regex source)",
        );
        return;
    }
    if NESTED_QUANTIFIERS
        .as_ref()
        .is_some_and(|regex| regex.find(pattern).is_some())
    {
        out.error(
            path,
            format!(
                "Pattern looks ReDoS-prone (nested quantifiers); rewrite without nested +/{} groups",
                "*"
            ),
        );
        return;
    }
    if let Err(reason) = Regex::new(pattern) {
        out.error(path, format!("Invalid regex: {reason}"));
    }
}
