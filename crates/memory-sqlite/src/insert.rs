use rusqlite::ToSql;
use rusqlite::Transaction;

use crate::error::SqliteError;

#[derive(Default)]
pub(crate) struct Columns<'a> {
    names: Vec<&'static str>,
    values: Vec<&'a dyn ToSql>,
}

impl<'a> Columns<'a> {
    pub(crate) fn set(mut self, name: &'static str, value: &'a dyn ToSql) -> Self {
        self.names.push(name);
        self.values.push(value);
        self
    }

    pub(crate) fn set_supplied<T: ToSql>(self, name: &'static str, value: Option<&'a T>) -> Self {
        match value {
            Some(value) => self.set(name, value),
            None => self,
        }
    }

    pub(crate) fn insert(
        &self,
        tx: &Transaction<'_>,
        table: &'static str,
        operation: &'static str,
    ) -> Result<(), SqliteError> {
        let placeholders = vec!["?"; self.names.len()].join(", ");
        let sql = format!(
            "INSERT INTO {table} ({}) VALUES ({placeholders})",
            self.names.join(", ")
        );
        tx.execute(&sql, self.values.as_slice())
            .map_err(SqliteError::query(operation))?;
        Ok(())
    }
}
