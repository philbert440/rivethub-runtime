#[expect(
    clippy::redundant_pub_crate,
    reason = "private shared test module: unreachable_pub requires pub(crate) here, the nursery lint disagrees"
)]
mod common;

use std::fs;

use common::TestResult;
use common::connection;
use common::fixture_copy;
use common::read_column_types;
use common::read_master_entries;
use common::read_pragmas;
use common::read_row_count;
use common::read_table_columns;
use common::schema_recording;
use memory_sqlite::SqliteStore;
use sha2::Digest;
use sha2::Sha256;
use tempfile::tempdir;

#[test]
fn the_shipped_schema_text_is_the_recorded_text() -> TestResult<()> {
    let recording = schema_recording()?;
    let shipped = include_str!("../src/schema/schema_v7.sql");
    assert_eq!(shipped, recording.schema_text);
    let mut hasher = Sha256::new();
    hasher.update(shipped.as_bytes());
    assert_eq!(
        format!("{:x}", hasher.finalize()),
        recording.schema_text_sha256
    );
    Ok(())
}

#[test]
fn a_fresh_file_matches_the_recorded_schema() -> TestResult<()> {
    let recording = schema_recording()?;
    let dir = tempdir()?;
    let path = dir.path().join("memory.sqlite");
    {
        let _store = SqliteStore::open(&path)?;
    }
    let conn = connection(&path)?;
    assert_eq!(read_pragmas(&conn)?, recording.fresh.pragmas);
    assert_eq!(
        read_master_entries(&conn)?,
        recording.fresh.sqlite_master,
        "sqlite_master of a fresh file"
    );
    for (table, expected) in &recording.fresh.table_info {
        assert_eq!(
            read_table_columns(&conn, table)?,
            *expected,
            "table_info of {table}"
        );
    }
    Ok(())
}

#[test]
fn the_recorded_v7_fixture_opens_unchanged() -> TestResult<()> {
    let recording = schema_recording()?;
    let fixture = fixture_copy("fixture-v7.sqlite")?;
    {
        let _store = SqliteStore::open(fixture.path())?;
    }
    let conn = connection(fixture.path())?;
    assert_eq!(read_pragmas(&conn)?, recording.fixture.pragmas);
    assert!(
        recording
            .fixture
            .row_counts
            .keys()
            .eq(recording.fixture.column_types.keys()),
        "the recording covers the same tables in row counts and typeof"
    );
    assert_eq!(
        read_master_entries(&conn)?,
        recording.fresh.sqlite_master,
        "sqlite_master of the fixture equals the fresh schema"
    );
    for (table, expected) in &recording.fixture.row_counts {
        assert_eq!(read_row_count(&conn, table)?, *expected, "rows of {table}");
    }
    for (table, expected) in &recording.fixture.column_types {
        assert_eq!(
            read_column_types(&conn, table)?,
            *expected,
            "typeof over {table}"
        );
    }
    Ok(())
}

#[test]
fn opening_a_v7_file_writes_nothing() -> TestResult<()> {
    let fixture = fixture_copy("fixture-v7.sqlite")?;
    let before = fs::read(fixture.path())?;
    {
        let _store = SqliteStore::open(fixture.path())?;
    }
    let after = fs::read(fixture.path())?;
    assert_eq!(before, after, "a current file must open without rewrites");
    Ok(())
}
