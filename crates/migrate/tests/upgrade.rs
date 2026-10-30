#[expect(
    dead_code,
    reason = "the harness module is included whole by every test binary and this one uses a subset"
)]
mod common;

use common::TestResult;
use common::connection;
use common::fixture_copy;
use common::instant;
use common::read_column_types;
use common::read_fts_rows;
use common::read_legacy_master_entries;
use common::read_pragmas;
use common::read_rows;
use common::read_table_columns;
use common::replay_search_access;
use common::upgrade_recording;
use memory_sqlite::SqliteError;
use memory_sqlite::SqliteStore;
use tempfile::tempdir;

#[test]
fn recorded_upgrade_files_reach_the_migrated_state() -> TestResult<()> {
    let recording = upgrade_recording()?;
    assert_eq!(recording.cases.len(), 3, "the recorded upgrade inputs");
    for case in &recording.cases {
        let fixture = fixture_copy(&case.input_file)?;
        let migrated = &case.migrated;
        migrate::sqlite_at(fixture.path(), instant(&migrated.clock)?)?;
        let conn = connection(fixture.path())?;
        replay_search_access(&conn, migrated)?;
        common::assert_upgraded_pragmas(&read_pragmas(&conn)?, &migrated.pragmas);
        assert_eq!(
            read_legacy_master_entries(&conn)?,
            migrated.sqlite_master,
            "sqlite_master after opening {}",
            case.input_file
        );
        for (table, expected) in &migrated.table_info {
            assert_eq!(
                read_table_columns(&conn, table)?,
                *expected,
                "table_info of {table} in {}",
                case.input_file
            );
        }
        for (table, expected) in &migrated.tables {
            assert_eq!(
                read_rows(&conn, table)?,
                *expected,
                "rows of {table} in {}",
                case.input_file
            );
        }
        for (name, expected) in &migrated.fts {
            assert_eq!(
                read_fts_rows(&conn, name)?,
                *expected,
                "fts rows of {name} in {}",
                case.input_file
            );
        }
        for (table, expected) in &migrated.column_types {
            assert_eq!(
                read_column_types(&conn, table)?,
                *expected,
                "typeof over {table} in {}",
                case.input_file
            );
        }
    }
    Ok(())
}

#[test]
fn a_newer_schema_version_is_a_typed_error() -> TestResult<()> {
    let dir = tempdir()?;
    let path = dir.path().join("future.sqlite");
    {
        let conn = connection(&path)?;
        conn.execute_batch("PRAGMA user_version = 99;")?;
    }
    match SqliteStore::open(&path) {
        Err(SqliteError::UnsupportedSchema { found }) => assert_eq!(found, 99),
        other => panic!("expected UnsupportedSchema, got {other:?}"),
    }
    Ok(())
}

#[test]
fn an_unknown_schema_version_is_a_typed_error() -> TestResult<()> {
    let dir = tempdir()?;
    let path = dir.path().join("negative.sqlite");
    {
        let conn = connection(&path)?;
        conn.execute_batch("PRAGMA user_version = -2;")?;
    }
    match SqliteStore::open(&path) {
        Err(SqliteError::UnsupportedSchema { found }) => assert_eq!(found, -2),
        other => panic!("expected UnsupportedSchema, got {other:?}"),
    }
    Ok(())
}

#[test]
fn a_failed_open_reports_the_path() -> TestResult<()> {
    let dir = tempdir()?;
    let path = dir.path().join("directory.sqlite");
    std::fs::create_dir(&path)?;
    match SqliteStore::open(&path) {
        Err(SqliteError::Open { path: reported, .. }) => assert_eq!(reported, path),
        other => panic!("expected Open, got {other:?}"),
    }
    Ok(())
}

#[test]
fn schema_25_archives_the_tasks_the_adapter_kept() -> TestResult<()> {
    let directory = tempdir()?;
    let path = directory.path().join("memory.sqlite");
    migrate::sqlite(&path)?;
    {
        let conn = rusqlite::Connection::open(&path)?;
        conn.execute_batch(
            "DROP TABLE rh_tasks;
             CREATE TABLE rh_tasks (id TEXT PRIMARY KEY, goal TEXT NOT NULL, executor TEXT NOT NULL,
                 status TEXT NOT NULL, error TEXT, completed_at TEXT);
             INSERT INTO rh_tasks VALUES ('a', 'beat', 'chat-loop', 'queued', NULL, NULL),
                 ('b', 'done', 'harness-session', 'completed', NULL, '2026-10-01T00:00:00.000Z');
             PRAGMA user_version = 24;",
        )?;
    }
    migrate::sqlite(&path)?;
    let conn = rusqlite::Connection::open(&path)?;
    let mut statement =
        conn.prepare("SELECT id, status, error FROM rh_tasks_archive ORDER BY id")?;
    let archived = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(
        archived,
        vec![
            (
                "a".to_owned(),
                "killed".to_owned(),
                Some("archived with the tasks refactor".to_owned())
            ),
            ("b".to_owned(), "completed".to_owned(), None),
        ]
    );
    let fresh: i64 = conn.query_row("SELECT count(*) FROM rh_tasks", [], |row| row.get(0))?;
    assert_eq!(fresh, 0);
    let shaped: i64 = conn.query_row(
        "SELECT count(*) FROM pragma_table_info('rh_tasks') WHERE name = 'child_session_id'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(shaped, 1);
    Ok(())
}
