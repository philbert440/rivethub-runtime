use std::path::Path;

use tempfile::TempDir;
use xtask::{baseline, git, kernelwire};

type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

const SOURCE: &str = "use wire::session_id::SessionId;\nuse std::fmt;\n\n#[serde_as(as = \"wire::time::Millis\")]\npub fn kind(value: wire::gateway::topics::TopicKind) -> bool {\n    matches!(value, wire::gateway::topics::TopicKind::Work)\n}\n";

fn repository(files: &[(&str, &str)]) -> Result<TempDir> {
    let repo = tempfile::tempdir()?;
    git::git(repo.path(), &["init", "-q"])?;
    git::git(repo.path(), &["config", "user.name", "Rivet Philbot"])?;
    git::git(repo.path(), &["config", "user.email", "rivetphilbot@gmail.com"])?;
    write(repo.path(), files)?;
    Ok(repo)
}

fn write(root: &Path, files: &[(&str, &str)]) -> Result {
    for (rel, source) in files {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, source)?;
    }
    git::git(root, &["add", "."])?;
    Ok(())
}

fn entry(file: &str, count: usize) -> String {
    format!(
        "[[entry]]\ncheck = \"KERNELWIRE\"\nfile = \"{file}\"\ntext = \"wire\"\ncount = {count}\n"
    )
}

fn findings(root: &Path) -> Result<Vec<String>> {
    let mut findings = Vec::new();
    kernelwire::check(root, &baseline::Baseline::load(root)?, &mut findings)?;
    Ok(findings)
}

#[test]
fn a_kernel_file_over_its_count_fails_and_lists_its_references() -> Result {
    let file = "crates/kernel/src/topics.rs";
    let repo = repository(&[
        (file, SOURCE),
        ("crates/gateway/src/topics.rs", SOURCE),
        ("xtask/baseline.toml", &entry(file, 2)),
    ])?;
    assert_eq!(
        findings(repo.path())?,
        [
            format!(
                "KERNELWIRE {file}: 4 references to wire where the baseline allows 2 → docs/PATTERNS.md#kernelwire"
            ),
            format!("KERNELWIRE {file}:1: wire::session_id::SessionId"),
            format!("KERNELWIRE {file}:4: wire::time::Millis"),
            format!("KERNELWIRE {file}:5: wire::gateway::topics::TopicKind"),
            format!("KERNELWIRE {file}:6: wire::gateway::topics::TopicKind::Work"),
        ]
    );
    write(repo.path(), &[("xtask/baseline.toml", &entry(file, 4))])?;
    assert_eq!(findings(repo.path())?, Vec::<String>::new());
    write(repo.path(), &[("xtask/baseline.toml", &entry(file, 5))])?;
    assert_eq!(
        findings(repo.path())?,
        [format!(
            "BASELINE xtask/baseline.toml: KERNELWIRE {file} count 5 no longer matches code (4); reduce or remove the entry"
        )]
    );
    Ok(())
}

#[test]
fn a_check_the_merge_base_does_not_name_sets_its_own_floor() -> Result {
    let fallback = "[[entry]]\ncheck = \"FALLBACK\"\nfile = \"crates/gateway/src/auth.rs\"\ntext = \"ok\"\ncount = 1\n";
    let repo = repository(&[
        ("xtask/src/baseline.rs", "\"FALLBACK\""),
        ("xtask/baseline.toml", ""),
    ])?;
    git::git(repo.path(), &["commit", "-q", "-m", "build: seed"])?;
    git::git(
        repo.path(),
        &["update-ref", "refs/remotes/origin/main", "HEAD"],
    )?;
    let kernel = entry("crates/kernel/src/topics.rs", 3);
    write(repo.path(), &[("xtask/baseline.toml", &kernel)])?;
    let mut findings = Vec::new();
    baseline::shrink(repo.path(), &mut findings)?;
    assert_eq!(findings, Vec::<String>::new());
    write(
        repo.path(),
        &[("xtask/baseline.toml", &format!("{kernel}\n{fallback}"))],
    )?;
    baseline::shrink(repo.path(), &mut findings)?;
    assert!(
        findings
            .iter()
            .any(|finding| finding.contains("crates/gateway/src/auth.rs")
                && finding.contains("may only shrink")),
        "{findings:?}"
    );
    Ok(())
}

#[test]
fn workspace_kernel_references_match_the_baseline() -> Result {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("xtask must live under the workspace root")?;
    assert_eq!(findings(root)?, Vec::<String>::new());
    Ok(())
}
