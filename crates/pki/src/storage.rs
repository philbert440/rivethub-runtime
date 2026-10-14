use crate::error::{Error, refused};
use num_bigint::BigUint;
use rcgen::{RevokedCertParams, SerialNumber};
use rusqlite::{Connection, Transaction, params};
use sha2::{Digest, Sha256};
use std::path::Path;
use subtle::ConstantTimeEq;
use time::OffsetDateTime;

pub(crate) fn connect(base: &Path) -> Result<Connection, Error> {
    let path = crate::files::contained(base, Path::new("ca.sqlite"))?;
    let connection = Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.busy_timeout(std::time::Duration::from_secs(5))?;
    Ok(connection)
}

pub(crate) fn initialize(base: &Path, serial: &str) -> Result<Connection, Error> {
    crate::files::create(base, "ca.sqlite", &[])?;
    let connection = connect(base)?;
    connection.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
        CREATE TABLE rh_ca_meta (id INTEGER PRIMARY KEY CHECK(id=1), serial TEXT NOT NULL, crl_number INTEGER NOT NULL, ready INTEGER NOT NULL);
        CREATE TABLE rh_ca_tokens (digest BLOB PRIMARY KEY, principal TEXT NOT NULL, expires INTEGER NOT NULL);
        CREATE TABLE rh_ca_certificates (serial TEXT PRIMARY KEY, principal TEXT, revoked INTEGER);
        CREATE TABLE rh_ca_blocked (principal TEXT PRIMARY KEY);
        CREATE TABLE rh_ca_limits (operation TEXT PRIMARY KEY, started INTEGER NOT NULL, count INTEGER NOT NULL);")?;
    connection.execute("INSERT INTO rh_ca_meta VALUES(1, ?1, 0, 0)", [serial])?;
    Ok(connection)
}

pub(crate) fn serial(value: &str) -> Result<BigUint, Error> {
    if value.is_empty() || value.len() > 40 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(Error::Import("invalid X.509 serial".to_owned()));
    }
    let result = BigUint::parse_bytes(value.as_bytes(), 16)
        .ok_or_else(|| Error::Import("invalid serial".to_owned()))?;
    if result == BigUint::from(0_u8) || result.bits() > 159 {
        return Err(Error::Import(
            "serial must be positive and at most 159 bits".to_owned(),
        ));
    }
    Ok(result)
}

pub(crate) fn next_serial(transaction: &Transaction<'_>) -> Result<SerialNumber, Error> {
    let current: String =
        transaction.query_row("SELECT serial FROM rh_ca_meta WHERE id=1", [], |row| {
            row.get(0)
        })?;
    let value = serial(&current)?;
    let next = &value + BigUint::from(1_u8);
    let next_hex = next.to_str_radix(16);
    serial(&next_hex)?;
    transaction.execute("UPDATE rh_ca_meta SET serial=?1 WHERE id=1", [next_hex])?;
    Ok(SerialNumber::from_slice(&value.to_bytes_be()))
}

pub(crate) fn rate(connection: &mut Connection, operation: &str, now: i64) -> Result<(), Error> {
    let transaction =
        connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    transaction.execute(
        "INSERT INTO rh_ca_limits VALUES(?1, ?2, 0) ON CONFLICT(operation) DO NOTHING",
        params![operation, now],
    )?;
    transaction.execute(
        "UPDATE rh_ca_limits SET started=?2, count=0 WHERE operation=?1 AND started <= ?2-60",
        params![operation, now],
    )?;
    let count: i64 = transaction.query_row(
        "SELECT count FROM rh_ca_limits WHERE operation=?1",
        [operation],
        |row| row.get(0),
    )?;
    if count >= 60 {
        return Err(refused("CA operation rate limit exceeded"));
    }
    transaction.execute(
        "UPDATE rh_ca_limits SET count=count+1 WHERE operation=?1",
        [operation],
    )?;
    transaction.commit()?;
    Ok(())
}

pub(crate) fn digest(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

pub(crate) fn authorize_token(
    transaction: &Transaction<'_>,
    token: &str,
    now: i64,
) -> Result<kernel::ca::Principal, Error> {
    if token.len() != 64 {
        return Err(refused("invalid enrollment token length"));
    }
    let supplied = digest(token);
    let mut statement =
        transaction.prepare("SELECT digest, principal FROM rh_ca_tokens WHERE expires > ?1")?;
    let mut rows = statement.query([now])?;
    let mut principal = None;
    while let Some(row) = rows.next()? {
        let stored: Vec<u8> = row.get(0)?;
        let bound: String = row.get(1)?;
        if bool::from(supplied.as_slice().ct_eq(&stored)) {
            principal = Some(bound);
        }
    }
    let bound =
        principal.ok_or_else(|| refused("invalid, expired or consumed enrollment token"))?;
    Ok(serde_json::from_str(&bound)?)
}

pub(crate) fn permitted(transaction: &Transaction<'_>, principal: &str) -> Result<(), Error> {
    let blocked: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM rh_ca_blocked WHERE principal=?1)",
        [principal],
        |row| row.get(0),
    )?;
    if blocked {
        return Err(refused("principal has been revoked"));
    }
    Ok(())
}

pub(crate) fn revoked(transaction: &Transaction<'_>) -> Result<Vec<RevokedCertParams>, Error> {
    let mut statement = transaction.prepare(
        "SELECT serial, revoked FROM rh_ca_certificates WHERE revoked IS NOT NULL ORDER BY serial",
    )?;
    let mut rows = statement.query([])?;
    let mut certificates = Vec::new();
    while let Some(row) = rows.next()? {
        let number: String = row.get(0)?;
        let timestamp: i64 = row.get(1)?;
        certificates.push(RevokedCertParams {
            serial_number: SerialNumber::from_slice(&serial(&number)?.to_bytes_be()),
            revocation_time: OffsetDateTime::from_unix_timestamp(timestamp)?,
            reason_code: None,
            invalidity_date: None,
        });
    }
    Ok(certificates)
}
