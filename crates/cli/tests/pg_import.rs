use memory_sqlite::import::ImportTable;

type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

#[test]
fn migration_command_imports_and_reports_rows() -> Result {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("memory.sqlite");
    let command = || {
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_rivethub-migrate"));
        command
            .arg("sqlite")
            .arg("--path")
            .arg(&path)
            .arg("--import-pg-dump")
            .arg(testkit::recording("memory-sqlite/import/copy-cases.txt"));
        command
    };
    let output = command().output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8(output.stderr)?
    );
    let report: memory_sqlite::import::ImportReport = serde_json::from_slice(&output.stdout)?;
    assert_eq!(report.rows.get(&ImportTable::Summaries), Some(&2));
    let again = command().output()?;
    assert!(!again.status.success());
    assert!(String::from_utf8(again.stderr)?.contains("not empty"));
    Ok(())
}
