use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleSource {
    GitRemote,
    GitRoot,
    CwdBasename,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectTag {
    pub key: String,
    pub value: String,
    pub display: String,
    pub source: RuleSource,
}

#[must_use]
pub fn plan_project_rule_tag(cwd: Option<&str>, allow_filesystem: bool) -> Option<ProjectTag> {
    let trimmed = cwd.map(str::trim).filter(|c| !c.is_empty())?;
    if allow_filesystem {
        let dir = Path::new(trimmed);
        if dir.is_dir() {
            if let Some(tag) =
                git_rule(dir, RuleSource::GitRemote, &["remote", "get-url", "origin"])
            {
                return Some(tag);
            }
            if let Some(tag) = git_rule(dir, RuleSource::GitRoot, &["rev-parse", "--show-toplevel"])
            {
                return Some(tag);
            }
        }
    }
    Some(basename_rule(trimmed))
}

pub(crate) async fn resolve_project_tag_blocking(
    cwd: Option<String>,
    allow_filesystem: bool,
) -> Option<ProjectTag> {
    tokio::task::spawn_blocking(move || plan_project_rule_tag(cwd.as_deref(), allow_filesystem))
        .await
        .ok()
        .flatten()
}

fn basename_rule(cwd: &str) -> ProjectTag {
    let name = Path::new(cwd)
        .file_name()
        .map_or_else(|| cwd.to_string(), |n| n.to_string_lossy().into_owned());
    let value = slug(&name);
    ProjectTag {
        key: "project".to_string(),
        value,
        display: name,
        source: RuleSource::CwdBasename,
    }
}

fn git_rule(dir: &Path, source: RuleSource, args: &[&str]) -> Option<ProjectTag> {
    let mut command = Command::new("git");
    command.args(args).current_dir(dir);
    let out = command.output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if text.is_empty() {
        return None;
    }
    let name = match source {
        RuleSource::GitRemote => Path::new(text.trim_end_matches('/'))
            .file_name()
            .map_or_else(|| text.clone(), |n| n.to_string_lossy().into_owned())
            .trim_end_matches(".git")
            .to_string(),
        RuleSource::GitRoot | RuleSource::CwdBasename => {
            text.rsplit('/').next().unwrap_or(&text).to_string()
        }
    };
    if name.is_empty() {
        return None;
    }
    Some(ProjectTag {
        key: "project".to_string(),
        value: slug(&name),
        display: name,
        source,
    })
}

fn slug(text: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            out.extend(ch.to_lowercase());
            last_dash = false;
        } else if !last_dash && !out.is_empty() {
            out.push('-');
            last_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}
