use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use agent_loop::fence::WorkspaceFence;
use agent_loop::shell::ShellDanger;
use serde::Deserialize;
use wire::JsonObject;

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Outcome {
    Allow,
    Block,
}

#[derive(Deserialize)]
struct ShellCase {
    name: String,
    outcome: Outcome,
    #[serde(flatten)]
    arguments: JsonObject,
}

#[derive(Deserialize)]
struct ShellFixture {
    cases: Vec<ShellCase>,
}

#[derive(Deserialize)]
struct FenceCase {
    name: String,
    tool: String,
    args: JsonObject,
    symlink: Option<BTreeMap<PathBuf, PathBuf>>,
    outcome: Outcome,
}

#[derive(Deserialize)]
struct FenceFixture {
    allowed: Vec<PathBuf>,
    cases: Vec<FenceCase>,
}

#[derive(Deserialize)]
struct Skip {
    name: String,
    reason: String,
}

fn skips() -> Vec<Skip> {
    serde_json::from_str(include_str!("../fixtures/skips.json")).expect("typed skip list")
}

fn skipped(name: &str) -> bool {
    skips().iter().any(|entry| entry.name == name)
}

fn shell_fixture() -> ShellFixture {
    serde_json::from_str(
        &std::fs::read_to_string(testkit::recording("agent-loop/shell.json"))
            .expect("shell recording"),
    )
    .expect("typed shell cases")
}

fn fence_fixture() -> FenceFixture {
    serde_json::from_str(
        &std::fs::read_to_string(testkit::recording("agent-loop/fence.json"))
            .expect("fence recording"),
    )
    .expect("typed fence cases")
}

#[test]
fn skip_list_names_existing_scenarios_and_explains_each() {
    let mut names = BTreeSet::new();
    for case in shell_fixture().cases {
        assert!(names.insert(format!("shell/{}", case.name)));
    }
    for case in fence_fixture().cases {
        assert!(names.insert(format!("fence/{}", case.name)));
    }
    let skipped = skips();
    let mut unique = BTreeSet::new();
    for entry in &skipped {
        assert!(names.contains(&entry.name), "unknown skip {}", entry.name);
        assert!(unique.insert(&entry.name), "duplicate skip");
        assert!(!entry.reason.trim().is_empty(), "skip requires reason");
    }
    assert_eq!(names.len() - skipped.len(), 22);
}

#[test]
fn replay_shell_safety_outcomes() {
    let shell = ShellDanger::new(BTreeSet::from(["shell".to_owned()])).expect("patterns compile");
    for case in shell_fixture().cases {
        if skipped(&format!("shell/{}", case.name)) {
            continue;
        }
        let outcome = if shell.check("shell", &case.arguments).is_ok() {
            Outcome::Allow
        } else {
            Outcome::Block
        };
        assert_eq!(outcome, case.outcome, "{}", case.name);
    }
}

fn mounted(root: &Path, recorded: &Path) -> PathBuf {
    root.join(recorded.strip_prefix("/").expect("recorded absolute path"))
}

#[tokio::test]
async fn replay_workspace_fence_outcomes() {
    let fixture = fence_fixture();
    let dir = crate::support::scratch();
    let roots = fixture
        .allowed
        .iter()
        .map(|path| mounted(dir.path(), path))
        .collect::<Vec<_>>();
    for root in &roots {
        tokio::fs::create_dir_all(root)
            .await
            .expect("workspace root");
    }
    let fence = WorkspaceFence::new(
        roots,
        BTreeSet::from(["file_read".to_owned(), "file_write".to_owned()]),
        None,
    )
    .await
    .expect("fence");
    for mut case in fixture.cases {
        if skipped(&format!("fence/{}", case.name)) {
            continue;
        }
        if let Some(links) = case.symlink {
            for (link, target) in links {
                let target = mounted(dir.path(), &target);
                tokio::fs::create_dir_all(&target)
                    .await
                    .expect("symlink target");
                tokio::fs::symlink(target, mounted(dir.path(), &link))
                    .await
                    .expect("fixture symlink");
            }
        }
        case.args.visit_strings_mut(&mut |_, text| {
            if Path::new(text).is_absolute() {
                *text = mounted(dir.path(), Path::new(text))
                    .to_str()
                    .expect("utf8 path")
                    .to_owned();
            }
        });
        let outcome = if fence.check(&case.tool, &case.args).await.is_ok() {
            Outcome::Allow
        } else {
            Outcome::Block
        };
        assert_eq!(outcome, case.outcome, "{}", case.name);
    }
}
