use std::path::Path;
use std::path::PathBuf;

use rusqlite::Connection;
use rusqlite::ToSql;
use rusqlite::types::ValueRef;
use tempfile::TempDir;

use crate::Error;
use crate::json::Json;
use crate::json::json_eq;

#[derive(Debug)]
pub struct FixtureCopy {
    _dir: TempDir,
    path: PathBuf,
}

impl FixtureCopy {
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

pub fn copy_fixture(path: &Path) -> Result<FixtureCopy, Error> {
    let dir = tempfile::tempdir().map_err(|source| Error::io(path, source))?;
    let target = dir.path().join(path.file_name().map_or_else(
        || "fixture.sqlite".to_string(),
        |n| n.to_string_lossy().to_string(),
    ));
    std::fs::copy(path, &target).map_err(|source| Error::io(path, source))?;
    let mut permissions = std::fs::metadata(&target)
        .map_err(|source| Error::io(&target, source))?
        .permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        permissions.set_mode(0o644);
    }
    std::fs::set_permissions(&target, permissions).map_err(|source| Error::io(&target, source))?;
    Ok(FixtureCopy {
        _dir: dir,
        path: target,
    })
}

#[derive(Debug)]
pub struct FixtureDb {
    copy: FixtureCopy,
    pub conn: Connection,
}

impl FixtureDb {
    #[must_use]
    pub fn path(&self) -> &Path {
        self.copy.path()
    }
}

pub fn open_fixture(path: &Path) -> Result<FixtureDb, Error> {
    let copy = copy_fixture(path)?;
    let conn = Connection::open(copy.path())?;
    Ok(FixtureDb { copy, conn })
}

fn value_to_json(value: ValueRef<'_>) -> Json {
    match value {
        ValueRef::Null => Json::Null,
        ValueRef::Integer(i) => Json::from(i),
        ValueRef::Real(f) => Json::from(f),
        ValueRef::Text(text) => std::str::from_utf8(text).map_or_else(
            |_| Json::String(String::from_utf8_lossy(text).into_owned()),
            |text| Json::String(text.to_string()),
        ),
        ValueRef::Blob(blob) => {
            let mut map = serde_json::Map::new();
            map.insert("$blob_hex".to_string(), Json::String(hex::encode(blob)));
            Json::Object(map)
        }
    }
}

pub fn query_json(conn: &Connection, sql: &str, params: &[&dyn ToSql]) -> Result<Vec<Json>, Error> {
    let mut statement = conn.prepare(sql)?;
    let columns: Vec<String> = statement
        .column_names()
        .into_iter()
        .map(str::to_string)
        .collect();
    let mut rows = statement.query(params)?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let mut map = serde_json::Map::new();
        for (index, name) in columns.iter().enumerate() {
            let value = value_to_json(row.get_ref(index)?);
            map.insert(name.clone(), value);
        }
        out.push(Json::Object(map));
    }
    Ok(out)
}

#[must_use]
pub fn rows_eq(a: &[Json], b: &[Json]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| json_eq(x, y))
}

#[must_use]
pub fn sqlite_version() -> String {
    rusqlite::version().to_string()
}
