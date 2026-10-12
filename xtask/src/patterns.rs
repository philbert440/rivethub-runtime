use std::path::Path;

use crate::error::Result;

pub fn dylint_lints() -> &'static [&'static str] {
    &[
        "counttype",
        "customserde",
        "dispatch",
        "dupnames",
        "duptype",
        "dynvalue",
        "jsmodel",
        "manualderive",
        "naming",
        "nullable",
        "opaque",
        "redundant_rename",
        "stringly",
        "strset",
        "tagfield",
        "typemacro",
        "untyped",
        "wirepub",
    ]
}

pub fn checks() -> &'static [&'static str] {
    &[
        "AUTHORSHIP",
        "COMMENT",
        "DEPS",
        "LANGUAGES",
        "LINTPOLICY",
        "MANIFEST",
        "NAMING",
        "PATTERNS",
        "PRIVACY",
        "SECRET",
        "SIZE",
    ]
}

pub fn check(root: &Path, findings: &mut Vec<String>) -> Result<()> {
    let path = root.join("docs").join("PATTERNS.md");
    let Ok(text) = std::fs::read_to_string(&path) else {
        findings.push("PATTERNS docs/PATTERNS.md is missing".to_owned());
        return Ok(());
    };
    let mut entries: Vec<(String, String)> = Vec::new();
    let mut current: Option<(String, Vec<String>)> = None;
    for line in text.lines() {
        if let Some(name) = line.strip_prefix("## ") {
            if let Some((previous, body)) = current.take() {
                entries.push((previous, body.join(" ")));
            }
            current = Some((name.trim().to_owned(), Vec::new()));
        } else if let Some((_, body)) = &mut current {
            body.push(line.trim().to_owned());
        }
    }
    if let Some((previous, body)) = current.take() {
        entries.push((previous, body.join(" ")));
    }
    let mut known: Vec<String> = dylint_lints()
        .iter()
        .map(|lint| (*lint).to_owned())
        .collect();
    known.extend(checks().iter().map(|name| name.to_ascii_lowercase()));
    known.extend(
        ["clippy", "deny.toml", "rustc", "deny"]
            .iter()
            .map(|name| (*name).to_owned()),
    );
    for (name, body) in &entries {
        let check_at = body.find("Check:").map(|index| &body[index..]);
        let Some(check_line) = check_at else {
            findings.push(format!(
                "PATTERNS {name}: entry has no Check: line naming its lint or check"
            ));
            continue;
        };
        let lower = check_line.to_ascii_lowercase();
        if !known
            .iter()
            .any(|enforcer| lower.contains(enforcer.as_str()))
        {
            findings.push(format!(
                "PATTERNS {name}: Check line names no known lint or check"
            ));
        }
    }
    for lint in dylint_lints() {
        let mentioned = entries
            .iter()
            .any(|(_, body)| body.to_ascii_lowercase().contains(lint));
        if !mentioned {
            findings.push(format!(
                "PATTERNS dylint lint `{lint}` has no entry in docs/PATTERNS.md"
            ));
        }
    }
    for check_name in checks() {
        let mentioned = entries.iter().any(|(_, body)| body.contains(check_name));
        if !mentioned {
            findings.push(format!(
                "PATTERNS check `{check_name}` has no entry in docs/PATTERNS.md"
            ));
        }
    }
    Ok(())
}
