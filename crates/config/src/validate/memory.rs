use std::sync::LazyLock;

use regress::Regex;

use crate::model::{
    EmbeddedSection, MemorySection, PostgresSection, RedactionSection, SqliteSection,
};
use crate::report::ValidationResult;

use super::looks_like_api_key;

const NESTED_QUANTIFIER_PATTERN: &str =
    r"\((?:[^\\)]|\\.)*[+*](?:[^\\)]|\\.)*\)(?:[+*?]|\{\d+,?\d*\})";

#[expect(
    clippy::expect_used,
    reason = "the nested-quantifier pattern is a constant; the validate tests match it"
)]
static NESTED_QUANTIFIERS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(NESTED_QUANTIFIER_PATTERN).expect("the nested-quantifier pattern is a constant")
});

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
    if key.contains("${") || key.is_empty() || !looks_like_api_key(key) {
        return;
    }
    out.warning(
        "memory.postgres.embed_api_key",
        "memory.postgres.embed_api_key appears hardcoded — prefer environment variables",
    );
}

fn embedded_section(out: &mut ValidationResult, embedded: &EmbeddedSection) {
    if embedded
        .data_dir
        .as_ref()
        .is_some_and(|dir| dir.as_os_str().is_empty())
    {
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
        Some(path) if path.as_os_str().is_empty() => out.error(
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
    if NESTED_QUANTIFIERS.find(pattern).is_some() {
        out.error(
            path,
            concat!(
                "Pattern looks ReDoS-prone (nested quantifiers); rewrite without nested +/",
                "* groups"
            ),
        );
        return;
    }
    if let Err(reason) = Regex::new(pattern) {
        out.error(path, format!("Invalid regex: {reason}"));
    }
}
