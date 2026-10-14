use std::path::Path;

use crate::error::Result;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub body: String,
}

pub fn dylint_lints() -> &'static [&'static str] {
    &[
        "counttype",
        "customserde",
        "dispatch",
        "fallback",
        "fallback_waiver",
        "dupnames",
        "duptype",
        "dynvalue",
        "jsmodel",
        "manualderive",
        "naming",
        "nullable",
        "opaque",
        "redundant_rename",
        "runtime",
        "sleep",
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
        "BASELINE",
        "COMMENT",
        "DEPS",
        "DUPFIXTURE",
        "ENV",
        "FALLBACK",
        "JSMODEL",
        "JSONBYTES",
        "LABPATH",
        "LANGUAGES",
        "LINTPOLICY",
        "MANIFEST",
        "NAMING",
        "PATHCONTAIN",
        "PATTERNS",
        "PRIVILEGE",
        "PRIVACY",
        "REASON",
        "SECRET",
        "SIZE",
        "SQLOFFLINE",
        "TESTPROTO",
        "TOOLCONFIG",
        "UTF16",
        "UTF8",
        "WAIVER",
    ]
}

pub fn check(root: &Path, findings: &mut Vec<String>) -> Result<()> {
    let path = root.join("docs").join("PATTERNS.md");
    let Ok(text) = std::fs::read_to_string(&path) else {
        findings.push("PATTERNS docs/PATTERNS.md is missing".to_owned());
        return Ok(());
    };
    let entries = entries(&text);
    let mut known: Vec<String> = dylint_lints()
        .iter()
        .map(|lint| (*lint).to_owned())
        .collect();
    known.extend(checks().iter().map(|name| name.to_ascii_lowercase()));
    known.extend(
        ["clippy", "deny.toml", "rustc", "deny", "machete"]
            .iter()
            .map(|name| (*name).to_owned()),
    );
    for entry in &entries {
        let check_at = entry.body.find("Check:").map(|index| &entry.body[index..]);
        let Some(check_line) = check_at else {
            findings.push(format!(
                "PATTERNS {}: entry has no Check: line naming its lint or check",
                entry.name
            ));
            continue;
        };
        let lower = check_line.to_ascii_lowercase();
        if !known
            .iter()
            .any(|enforcer| lower.contains(enforcer.as_str()))
        {
            findings.push(format!(
                "PATTERNS {}: Check line names no known lint or check",
                entry.name
            ));
        }
    }
    for lint in dylint_lints() {
        let mentioned = entries
            .iter()
            .any(|entry| entry.body.to_ascii_lowercase().contains(lint));
        if !mentioned {
            findings.push(format!(
                "PATTERNS dylint lint `{lint}` has no entry in docs/PATTERNS.md"
            ));
        }
    }
    for check_name in checks() {
        let mentioned = entries.iter().any(|entry| entry.body.contains(check_name));
        if !mentioned {
            findings.push(format!(
                "PATTERNS check `{check_name}` has no entry in docs/PATTERNS.md"
            ));
        }
    }
    Ok(())
}

pub fn entries(text: &str) -> Vec<Entry> {
    let mut out: Vec<Entry> = Vec::new();
    let mut current: Option<(String, Vec<String>)> = None;
    for line in text.lines() {
        if let Some(name) = line.strip_prefix("## ") {
            if let Some((previous, body)) = current.take() {
                out.push(Entry {
                    name: previous,
                    body: body.join(" "),
                });
            }
            current = Some((name.trim().to_owned(), Vec::new()));
        } else if let Some((_, body)) = &mut current {
            body.push(line.trim().to_owned());
        }
    }
    if let Some((previous, body)) = current.take() {
        out.push(Entry {
            name: previous,
            body: body.join(" "),
        });
    }
    out
}
