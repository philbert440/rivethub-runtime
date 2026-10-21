use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::Output;

use serde::Deserialize;
use serde::Serialize;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const EVAL: &str = env!("CARGO_BIN_EXE_search-eval");

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Baseline {
    limit: u32,
    query_count: usize,
    recall_at_k: f64,
    mrr: f64,
    must_contain_total: usize,
    must_contain_passed: usize,
    must_contain_pass_rate: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct QuerySet {
    clock: String,
    limit: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tolerance: Option<f64>,
    queries: Vec<EvalQuery>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EvalQuery {
    id: String,
    query: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mode: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    relevant: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    must_contain: Vec<String>,
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/search-eval")
        .join(name)
}

fn eval(args: &[&Path], flags: &[&str]) -> TestResult<Output> {
    let mut command = Command::new(EVAL);
    for (flag, value) in ["--corpus", "--queries", "--baseline"].iter().zip(args) {
        command.arg(flag).arg(value);
    }
    Ok(command.args(flags).output()?)
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> TestResult<T> {
    Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> TestResult {
    std::fs::write(path, serde_json::to_string_pretty(value)?)?;
    Ok(())
}

fn with_a_missing_query(queries: &Path, dir: &Path) -> TestResult<PathBuf> {
    let mut set: QuerySet = read_json(queries)?;
    set.queries.push(EvalQuery {
        id: "no-row-holds-it".to_owned(),
        query: "zebra xylophone".to_owned(),
        mode: None,
        relevant: vec!["00000000-0000-4000-8000-000000000012".to_owned()],
        must_contain: vec!["chrony".to_owned()],
    });
    let path = dir.join("queries.json");
    write_json(&path, &set)?;
    Ok(path)
}

fn committed() -> [PathBuf; 3] {
    [
        fixture("corpus.json"),
        fixture("queries.json"),
        fixture("baseline.json"),
    ]
}

#[test]
fn the_committed_fixture_meets_its_saved_baseline() -> TestResult {
    let [corpus, queries, baseline] = committed();
    let output = eval(&[&corpus, &queries, &baseline], &["--check"])?;
    let text = stdout(&output);
    assert_eq!(output.status.code(), Some(0), "{text}");
    assert!(text.contains("SUMMARY (15 queries)"), "{text}");
    assert!(text.contains("recall@10=0.964  mrr=0.702"), "{text}");
    assert!(
        text.contains("must_contain: 12 pass / 0 fail (1.000)"),
        "{text}"
    );
    assert!(text.contains("all metrics within tolerance"), "{text}");
    Ok(())
}

#[test]
fn saving_reproduces_the_committed_baseline_byte_for_byte() -> TestResult {
    let [corpus, queries, baseline] = committed();
    let dir = tempfile::tempdir()?;
    let saved = dir.path().join("baseline.json");
    let output = eval(&[&corpus, &queries, &saved], &["--save-baseline"])?;
    assert_eq!(output.status.code(), Some(0), "{}", stdout(&output));
    assert_eq!(
        std::fs::read_to_string(&saved)?,
        std::fs::read_to_string(&baseline)?
    );
    Ok(())
}

#[test]
fn every_metric_below_the_baseline_fails_the_check_by_name() -> TestResult {
    let [corpus, queries, baseline] = committed();
    let dir = tempfile::tempdir()?;
    let raised = dir.path().join("baseline.json");
    let mut perfect: Baseline = read_json(&baseline)?;
    perfect.query_count += 1;
    perfect.recall_at_k = 1.0;
    perfect.mrr = 1.0;
    perfect.must_contain_pass_rate = 1.0;
    write_json(&raised, &perfect)?;
    let missing = with_a_missing_query(&queries, dir.path())?;
    let output = eval(&[&corpus, &missing, &raised], &["--check"])?;
    let text = stdout(&output);
    assert_eq!(output.status.code(), Some(1), "{text}");
    for line in [
        "FAIL recall@k: 0.900 is 0.100 below baseline 1.000",
        "FAIL mrr: 0.656 is 0.344 below baseline 1.000",
        "FAIL must_contain_pass_rate: 0.923 is 0.077 below baseline 1.000",
    ] {
        assert!(text.contains(line), "{line} missing from {text}");
    }
    Ok(())
}

#[test]
fn the_query_set_tolerance_absorbs_a_drop_within_it() -> TestResult {
    let [corpus, queries, baseline] = committed();
    let dir = tempfile::tempdir()?;
    let mut raised: Baseline = read_json(&baseline)?;
    raised.recall_at_k += 0.05;
    raised.mrr += 0.05;
    let raised_path = dir.path().join("baseline.json");
    write_json(&raised_path, &raised)?;
    let mut tolerant: QuerySet = read_json(&queries)?;
    tolerant.tolerance = Some(0.1);
    let tolerant_path = dir.path().join("queries.json");
    write_json(&tolerant_path, &tolerant)?;
    let strict = eval(&[&corpus, &queries, &raised_path], &["--check"])?;
    assert_eq!(strict.status.code(), Some(1), "{}", stdout(&strict));
    let output = eval(&[&corpus, &tolerant_path, &raised_path], &["--check"])?;
    assert_eq!(output.status.code(), Some(0), "{}", stdout(&output));
    Ok(())
}

#[test]
fn a_baseline_saved_for_another_query_set_is_stale() -> TestResult {
    let [corpus, queries, baseline] = committed();
    let dir = tempfile::tempdir()?;
    let mut stale: Baseline = read_json(&baseline)?;
    stale.query_count = 11;
    let stale_path = dir.path().join("baseline.json");
    write_json(&stale_path, &stale)?;
    let output = eval(&[&corpus, &queries, &stale_path], &["--check"])?;
    let text = stdout(&output);
    assert_eq!(output.status.code(), Some(1), "{text}");
    assert!(
        text.contains("FAIL baseline was saved for 11 queries at limit 10; this run has 15 queries at limit 10 (regenerate it with --save-baseline)"),
        "{text}"
    );
    Ok(())
}

#[test]
fn a_plain_run_fails_only_when_a_query_misses_its_expectations() -> TestResult {
    let [corpus, queries, _] = committed();
    let output = eval(&[&corpus, &queries], &[])?;
    assert_eq!(output.status.code(), Some(0), "{}", stdout(&output));
    let dir = tempfile::tempdir()?;
    let missing = with_a_missing_query(&queries, dir.path())?;
    let output = eval(&[&corpus, &missing], &[])?;
    let text = stdout(&output);
    assert_eq!(output.status.code(), Some(1), "{text}");
    assert!(
        text.contains("[no-row-holds-it] \"zebra xylophone\" (hybrid)"),
        "{text}"
    );
    assert!(
        text.contains("relevant: 0/1 found, none in the results\n  must_contain: FAIL"),
        "{text}"
    );
    let mut passing: QuerySet = read_json(&queries)?;
    passing.queries.retain(|query| query.id == "deploy-staging");
    let passing_path = dir.path().join("queries.json");
    write_json(&passing_path, &passing)?;
    let output = eval(&[&corpus, &passing_path], &[])?;
    let text = stdout(&output);
    assert_eq!(output.status.code(), Some(0), "{text}");
    assert!(
        text.contains("relevant: 2/2 found, first at rank 1\n  must_contain: pass"),
        "{text}"
    );
    Ok(())
}

const PINNED_ROWS: &str = "INSERT INTO ros_conversations (id, session_key, agent, channel, created_at, updated_at) \
     VALUES ('pinned', 'pinned', 'rivet', 'cli', '2026-10-01T09:00:00.000Z', '2026-10-01T09:00:00.000Z'); \
     INSERT INTO ros_messages (id, conversation_id, role, content, agent, channel, created_at) \
     VALUES ('00000000-0000-4000-8000-000000000101', 'pinned', 'user', 'rotate the staging signing key', \
     'rivet', 'cli', '2026-10-01T09:01:00.000Z');";

#[test]
fn a_pinned_database_is_scored_from_a_copy_and_left_unchanged() -> TestResult {
    let dir = tempfile::tempdir()?;
    let pinned = dir.path().join("eval.sqlite");
    migrate::sqlite(&pinned)?;
    rusqlite::Connection::open(&pinned)?.execute_batch(PINNED_ROWS)?;
    let set = QuerySet {
        clock: "2026-10-10T12:00:00Z".to_owned(),
        limit: 10,
        tolerance: None,
        queries: vec![EvalQuery {
            id: "signing-key".to_owned(),
            query: "signing key".to_owned(),
            mode: None,
            relevant: vec!["00000000-0000-4000-8000-000000000101".to_owned()],
            must_contain: vec!["staging signing key".to_owned()],
        }],
    };
    let queries = dir.path().join("queries.json");
    write_json(&queries, &set)?;
    let before = std::fs::read(&pinned)?;
    let output = Command::new(EVAL)
        .arg("--sqlite")
        .arg(&pinned)
        .arg("--queries")
        .arg(&queries)
        .output()?;
    let text = stdout(&output);
    assert_eq!(output.status.code(), Some(0), "{text}");
    assert!(
        text.contains("relevant: 1/1 found, first at rank 1\n  must_contain: pass"),
        "{text}"
    );
    assert_eq!(std::fs::read(&pinned)?, before);
    let mut entries: Vec<_> = std::fs::read_dir(dir.path())?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<Result<_, _>>()?;
    entries.sort();
    assert_eq!(entries, ["eval.sqlite", "queries.json"]);
    Ok(())
}

#[test]
fn an_unreadable_baseline_is_an_error_not_a_verdict() -> TestResult {
    let [corpus, queries, _] = committed();
    let dir = tempfile::tempdir()?;
    let missing = dir.path().join("absent.json");
    let output = eval(&[&corpus, &queries, &missing], &["--check"])?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(stderr.starts_with("search-eval: cannot read "), "{stderr}");
    Ok(())
}
