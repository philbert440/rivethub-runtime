use std::fmt::Write as _;
use std::path::Path;

use regex::Regex;
use serde::Deserialize;
use std::sync::LazyLock;

use crate::error::Result;
use crate::git::git;
use crate::privacy;

#[derive(Debug, Deserialize)]
struct PolicyFile {
    lints: LintsPolicy,
}

#[derive(Debug, Deserialize)]
struct LintsPolicy {
    #[serde(default)]
    rust: Option<toml::Table>,
    #[serde(default)]
    clippy: Option<toml::Table>,
}

const BINARY_CRATE: &str = "rivethub";

pub fn binary_dependency(metadata: &cargo_metadata::Metadata, findings: &mut Vec<String>) {
    for package in &metadata.packages {
        let depends = package.dependencies.iter().any(|dependency| {
            dependency.name == BINARY_CRATE || dependency.rename.as_deref() == Some(BINARY_CRATE)
        });
        if depends {
            findings.push(format!(
                "MANIFEST {}: workspace members must not depend on the {BINARY_CRATE} binary",
                package.name
            ));
        }
    }
}

pub fn manifests(root: &Path, findings: &mut Vec<String>) -> Result<Vec<String>> {
    let metadata = cargo_metadata::MetadataCommand::new()
        .manifest_path(root.join("Cargo.toml"))
        .no_deps()
        .exec()?;
    let mut names = Vec::new();
    for package in &metadata.packages {
        let Some(source) = package.manifest_path.strip_prefix(root).ok() else {
            continue;
        };
        let manifest_path = root.join(source);
        let name = package.name.to_string();
        if name == "xtask" {
            names.push(name);
            continue;
        }
        names.push(name.clone());
        let dir = manifest_path
            .parent()
            .and_then(|parent| parent.file_name())
            .and_then(|parent| parent.to_str())
            .unwrap_or_default();
        if name != dir {
            findings.push(format!(
                "MANIFEST {name}: package.name must equal the directory name"
            ));
        }
        let text = std::fs::read_to_string(&manifest_path)?;
        let value: toml::Table = toml::from_str(&text)?;
        let lints = value.get("lints");
        let workspace_flag = lints.and_then(|lints| lints.get("workspace"));
        let workspace_lints = workspace_flag.is_some_and(toml::Value::is_bool)
            && workspace_flag.and_then(toml::Value::as_bool) == Some(true);
        if !workspace_lints {
            findings.push(format!("MANIFEST {name}: missing [lints] workspace = true"));
        }
        let package_table = value
            .get("package")
            .and_then(toml::Value::as_table)
            .cloned()
            .unwrap_or_default();
        for key in ["edition", "license", "version", "publish", "rust-version"] {
            let inherited = package_table
                .get(key)
                .and_then(toml::Value::as_table)
                .and_then(|table| table.get("workspace"))
                .and_then(toml::Value::as_bool)
                == Some(true);
            if !inherited {
                findings.push(format!(
                    "MANIFEST {name}: package.{key} must be workspace = true"
                ));
            }
        }
        for section in ["dependencies", "dev-dependencies", "build-dependencies"] {
            let Some(entries) = value.get(section).and_then(toml::Value::as_table) else {
                continue;
            };
            for (dep, spec) in entries {
                if spec
                    .as_table()
                    .and_then(|table| table.get("path"))
                    .is_some()
                {
                    continue;
                }
                let from_workspace = spec
                    .as_table()
                    .and_then(|table| table.get("workspace"))
                    .and_then(toml::Value::as_bool)
                    == Some(true);
                if !from_workspace {
                    findings.push(format!(
                        "MANIFEST {name}: {section}.{dep} must come from [workspace.dependencies]"
                    ));
                }
            }
        }
    }
    binary_dependency(&metadata, findings);
    Ok(names)
}

pub fn lint_policy(root: &Path, findings: &mut Vec<String>) -> Result<()> {
    let text = std::fs::read_to_string(root.join("xtask").join("lints-policy.toml"))?;
    let expected: PolicyFile = toml::from_str(&text)?;
    let actual_text = std::fs::read_to_string(root.join("Cargo.toml"))?;
    let actual_value: toml::Table = toml::from_str(&actual_text)?;
    let workspace = actual_value
        .get("workspace")
        .and_then(toml::Value::as_table);
    let lints = workspace.and_then(|table| table.get("lints"));
    let actual: Option<LintsPolicy> = lints.cloned().map(LintsPolicy::deserialize).transpose()?;
    let policy_matches = actual.as_ref().is_some_and(|actual| {
        table_eq(actual.rust.as_ref(), expected.lints.rust.as_ref())
            && table_eq(actual.clippy.as_ref(), expected.lints.clippy.as_ref())
    });
    if !policy_matches {
        findings.push(
            "LINTPOLICY [workspace.lints] differs from xtask/lints-policy.toml (lint policy changes only in a reviewed commit) → docs/PATTERNS.md#lintpolicy"
                .to_owned(),
        );
    }
    Ok(())
}

fn table_eq(actual: Option<&toml::Table>, expected: Option<&toml::Table>) -> bool {
    let normalize = |table: Option<&toml::Table>| -> Vec<(String, String)> {
        table
            .into_iter()
            .flatten()
            .map(|(key, value)| (key.clone(), render(value)))
            .collect()
    };
    normalize(actual) == normalize(expected)
}

fn render(value: &toml::Value) -> String {
    match value {
        toml::Value::String(text) => text.clone(),
        toml::Value::Table(table) => {
            let mut parts: Vec<String> = table
                .iter()
                .map(|(key, value)| format!("{key}={}", render(value)))
                .collect();
            parts.sort();
            format!("{{{}}}", parts.join(","))
        }
        toml::Value::Integer(value) => value.to_string(),
        toml::Value::Float(value) => value.to_string(),
        toml::Value::Boolean(value) => value.to_string(),
        toml::Value::Datetime(value) => value.to_string(),
        toml::Value::Array(values) => {
            let parts: Vec<String> = values.iter().map(render).collect();
            format!("[{}]", parts.join(","))
        }
    }
}

pub fn identity_ok(root: &Path, name: &str, email: &str) -> Result<bool> {
    let path = root
        .join("xtask")
        .join("data")
        .join("house-identities.json");
    let file: crate::privacy::IdentityFile =
        serde_json::from_str(&std::fs::read_to_string(&path)?)?;
    let identity = format!(
        "{} <{}>",
        name.trim().to_lowercase(),
        email.trim().to_lowercase()
    );
    let digest = privacy::sha256(&identity);
    Ok(file.sha256.contains(&digest))
}

type Cell = LazyLock<std::result::Result<Regex, regex::Error>>;

static BLOCKED_TRAILERS: Cell =
    LazyLock::new(|| Regex::new(r"(?im)^(Co-authored-by|Claude-Session|Signed-off-by):"));
static BLOCKED_TEXT: Cell = LazyLock::new(|| {
    Regex::new(r"(?i)(claude|anthropic|cursor|copilot|chatgpt|openai|generated with)")
});
static AI_TEXT: Cell = LazyLock::new(|| Regex::new(r"(?i)(generated with|session_|co-authored)"));

fn regex_of(cell: &'static Cell) -> Option<&'static Regex> {
    (**cell).as_ref().ok()
}

pub fn commit_message(
    root: &Path,
    findings: &mut Vec<String>,
    text: &str,
    author: Option<(&str, &str)>,
) -> Result<()> {
    if regex_of(&BLOCKED_TRAILERS).is_some_and(|regex| regex.is_match(text)) {
        findings.push(
            "AUTHORSHIP commit message carries a Co-authored-by / Claude-Session / Signed-off-by trailer"
                .to_owned(),
        );
    }
    if regex_of(&BLOCKED_TEXT).is_some_and(|regex| regex.is_match(text))
        && regex_of(&AI_TEXT).is_some_and(|regex| regex.is_match(text))
    {
        findings.push("AUTHORSHIP commit message carries AI attribution text".to_owned());
    }
    if let Some((name, email)) = author
        && !identity_ok(root, name, email)?
    {
        findings.push(format!(
            "AUTHORSHIP {name} <{email}> is not a house identity"
        ));
    }
    Ok(())
}

pub fn commits(root: &Path, range: &str, findings: &mut Vec<String>) -> Result<usize> {
    let out = git(root, &["log", "--format=%H%x1f%an%x1f%ae%x1f%B%x1e", range])?;
    let mut count = 0;
    for record in out.split('\u{1e}') {
        let record = record.trim_matches('\n');
        if record.is_empty() {
            continue;
        }
        let mut parts = record.split('\u{1f}');
        let (Some(hash), Some(name), Some(email), Some(body)) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        count += 1;
        let before = findings.len();
        commit_message(root, findings, body, Some((name, email)))?;
        let suffix = hash.get(..8).unwrap_or(hash).to_owned();
        for finding in findings.iter_mut().skip(before) {
            let _ = write!(finding, " ({suffix})");
        }
    }
    Ok(count)
}
