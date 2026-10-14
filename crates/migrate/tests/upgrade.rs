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
use common::read_master_entries;
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
        assert_eq!(
            read_pragmas(&conn)?,
            migrated.pragmas,
            "pragmas after opening {}",
            case.input_file
        );
        assert_eq!(
            read_master_entries(&conn)?,
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
