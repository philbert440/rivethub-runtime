mod disk;
mod turns;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use chrono::{DateTime, Utc};
use kernel::harness::TurnAttachment;
use parking_lot::RwLock;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use uuid::Uuid;
use wire::gateway::uploads::UploadCreated;
use wire::session_id::SessionId;

pub(crate) use turns::{AttachedTurn, attach};

use crate::tenancy::UserId;

#[derive(Debug, Clone)]
pub struct UploadOptions {
    pub root: PathBuf,
    pub max_bytes: u64,
    pub ttl: Duration,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum UploadError {
    #[error("upload exceeds the {max} byte limit")]
    TooLarge { max: u64 },
    #[error("upload is empty")]
    Empty,
    #[error("upload name is invalid")]
    InvalidName,
    #[error("media type is invalid")]
    InvalidMediaType,
    #[error("invalid upload id")]
    InvalidId,
    #[error("upload ttl must be positive")]
    Ttl,
    #[error("upload limits must be positive")]
    Limits,
    #[error("upload not found")]
    NotFound,
    #[error("upload path escapes the upload root")]
    Escape,
    #[error("upload metadata is corrupt")]
    Corrupt,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("upload task failed")]
    Task(#[from] tokio::task::JoinError),
}

#[derive(Debug)]
pub struct UploadService {
    options: UploadOptions,
    opened: tokio::sync::OnceCell<Arc<Uploads>>,
}

impl UploadService {
    #[must_use]
    pub fn new(options: UploadOptions) -> Self {
        Self {
            options,
            opened: tokio::sync::OnceCell::new(),
        }
    }

    pub(crate) async fn store(&self) -> Result<Arc<Uploads>, UploadError> {
        self.opened
            .get_or_try_init(|| {
                let options = self.options.clone();
                async move {
                    let opened =
                        tokio::task::spawn_blocking(move || Uploads::open(&options)).await??;
                    Ok(Arc::new(opened))
                }
            })
            .await
            .cloned()
    }

    pub(crate) async fn close(&self) {
        if let Some(store) = self.opened.get() {
            store.close().await;
        }
    }
}

#[derive(Debug)]
struct Record {
    owner: UserId,
    name: String,
    media_type: String,
    bytes: u64,
    expires_at: DateTime<Utc>,
    session: Option<String>,
    bin: PathBuf,
    meta: PathBuf,
}

#[derive(Debug)]
pub(crate) struct Uploads {
    root: PathBuf,
    max_bytes: u64,
    ttl: Duration,
    index: RwLock<BTreeMap<String, Record>>,
    shutdown: CancellationToken,
    tasks: TaskTracker,
    wake: Notify,
    started: AtomicBool,
    journal: tokio::sync::Mutex<()>,
}

#[derive(Debug)]
pub(crate) struct NewUpload<'a> {
    pub(crate) owner: &'a UserId,
    pub(crate) name: &'a str,
    pub(crate) media_type: &'a str,
    pub(crate) session: Option<String>,
    pub(crate) bytes: &'a [u8],
    pub(crate) now: DateTime<Utc>,
}

impl Uploads {
    fn open(options: &UploadOptions) -> Result<Self, UploadError> {
        if options.max_bytes == 0 || options.ttl.is_zero() {
            return Err(UploadError::Limits);
        }
        std::fs::create_dir_all(&options.root)?;
        if std::fs::symlink_metadata(&options.root)?
            .file_type()
            .is_symlink()
        {
            return Err(UploadError::Escape);
        }
        let root = std::fs::canonicalize(&options.root)?;
        let now = Utc::now();
        let index = disk::load(&root, now)?;
        Ok(Self {
            root,
            max_bytes: options.max_bytes,
            ttl: options.ttl,
            index: RwLock::new(index),
            shutdown: CancellationToken::new(),
            tasks: TaskTracker::new(),
            wake: Notify::new(),
            started: AtomicBool::new(false),
            journal: tokio::sync::Mutex::new(()),
        })
    }

    pub(crate) fn max_bytes(&self) -> u64 {
        self.max_bytes
    }

    pub(crate) async fn close(&self) {
        self.shutdown.cancel();
        self.wake.notify_waiters();
        self.tasks.close();
        self.tasks.wait().await;
    }

    pub(crate) async fn create(
        self: &Arc<Self>,
        upload: NewUpload<'_>,
    ) -> Result<UploadCreated, UploadError> {
        self.ensure_sweeper();
        validate(&upload, self.max_bytes, self.ttl)?;
        let id = Uuid::new_v4().to_string();
        let ttl = chrono::Duration::from_std(self.ttl).map_err(|_| UploadError::Ttl)?;
        let expires_at = upload.now + ttl;
        let record = Record {
            owner: upload.owner.clone(),
            name: upload.name.to_owned(),
            media_type: upload.media_type.to_owned(),
            bytes: u64::try_from(upload.bytes.len()).map_err(|_| UploadError::TooLarge {
                max: self.max_bytes,
            })?,
            expires_at,
            session: upload.session.clone(),
            bin: PathBuf::new(),
            meta: PathBuf::new(),
        };
        let root = self.root.clone();
        let bytes = upload.bytes.to_vec();
        let stored_id = id.clone();
        let stored = tokio::task::spawn_blocking(move || {
            disk::write_new(&root, &stored_id, &record, &bytes)
        })
        .await??;
        let created = created(&id, &stored);
        self.index.write().insert(id, stored);
        self.wake.notify_one();
        Ok(created)
    }

    pub(crate) async fn list(
        &self,
        owner: &UserId,
        session: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<Vec<UploadCreated>, UploadError> {
        self.purge(now).await?;
        let mut uploads = Vec::new();
        {
            let records = self.index.read();
            for (id, record) in records.iter() {
                if &record.owner == owner
                    && session.is_none_or(|key| record.session.as_deref() == Some(key))
                {
                    uploads.push(created(id, record));
                }
            }
        }
        Ok(uploads)
    }

    pub(crate) async fn read(
        &self,
        owner: &UserId,
        id: &str,
        now: DateTime<Utc>,
    ) -> Result<(String, Vec<u8>), UploadError> {
        let record = self.owned(owner, id, now).await?;
        let root = self.root.clone();
        let media_type = record.media_type.clone();
        let bytes =
            tokio::task::spawn_blocking(move || disk::read_contained(&root, &record.bin)).await??;
        Ok((media_type, bytes))
    }

    pub(crate) async fn delete(
        &self,
        owner: &UserId,
        id: &str,
        now: DateTime<Utc>,
    ) -> Result<(), UploadError> {
        parse_id(id)?;
        self.purge(now).await?;
        let removed = {
            let mut index = self.index.write();
            let caller_owns = matches!(index.get(id), Some(found) if &found.owner == owner);
            if caller_owns { index.remove(id) } else { None }
        };
        let Some(record) = removed else {
            return Err(UploadError::NotFound);
        };
        let restored = clone_record(&record);
        if let Err(error) = self.remove_files(record).await {
            self.index.write().insert(id.to_owned(), restored);
            return Err(error);
        }
        Ok(())
    }

    pub(crate) async fn attachment(
        &self,
        owner: &UserId,
        uri: &str,
        now: DateTime<Utc>,
    ) -> Result<TurnAttachment, UploadError> {
        self.purge(now).await?;
        let records = self.index.read();
        records
            .values()
            .find(|record| &record.owner == owner && record.bin.as_os_str() == uri)
            .map(|record| TurnAttachment {
                path: record.bin.clone(),
                media_type: record.media_type.clone(),
                name: record.name.clone(),
            })
            .ok_or(UploadError::NotFound)
    }

    pub(crate) async fn record_turn(
        &self,
        session: &SessionId,
        turn: AttachedTurn,
    ) -> Result<(), UploadError> {
        let root = self.root.clone();
        let session = session.clone();
        let writing = self.journal.lock().await;
        let appended =
            tokio::task::spawn_blocking(move || turns::append(&root, &session, turn)).await?;
        drop(writing);
        appended
    }

    pub(crate) async fn attached_turns(
        &self,
        session: &SessionId,
    ) -> Result<Vec<AttachedTurn>, UploadError> {
        let root = self.root.clone();
        let session = session.clone();
        tokio::task::spawn_blocking(move || turns::read(&root, &session)).await?
    }

    async fn owned(
        &self,
        owner: &UserId,
        id: &str,
        now: DateTime<Utc>,
    ) -> Result<Record, UploadError> {
        parse_id(id)?;
        self.purge(now).await?;
        let cloned = {
            let records = self.index.read();
            records
                .get(id)
                .filter(|record| &record.owner == owner)
                .map(clone_record)
        };
        cloned.ok_or(UploadError::NotFound)
    }

    async fn purge(&self, now: DateTime<Utc>) -> Result<(), UploadError> {
        let expired = {
            let mut records = self.index.write();
            let ids = records
                .iter()
                .filter(|(_, record)| record.expires_at <= now)
                .map(|(id, _)| id.clone())
                .collect::<Vec<_>>();
            let mut expired = Vec::with_capacity(ids.len());
            for id in ids {
                if let Some(record) = records.remove(&id) {
                    expired.push(record);
                }
            }
            expired
        };
        for record in expired {
            self.remove_files(record).await?;
        }
        Ok(())
    }

    async fn remove_files(&self, record: Record) -> Result<(), UploadError> {
        let root = self.root.clone();
        tokio::task::spawn_blocking(move || {
            disk::remove_contained(&root, &record.bin)?;
            disk::remove_contained(&root, &record.meta)?;
            Ok(())
        })
        .await?
    }

    fn ensure_sweeper(self: &Arc<Self>) {
        if self.started.swap(true, Ordering::AcqRel) {
            return;
        }
        let store = Arc::clone(self);
        self.tasks.spawn(async move { store.sweep().await });
    }

    async fn sweep(self: Arc<Self>) {
        loop {
            let deadline = self
                .index
                .read()
                .values()
                .map(|record| record.expires_at)
                .min();
            let notified = self.wake.notified();
            tokio::pin!(notified);
            tokio::select! {
                () = self.shutdown.cancelled() => break,
                () = notified => {}
                () = wait_until(deadline) => {}
            }
            if self.shutdown.is_cancelled() {
                break;
            }
            if let Err(error) = self.purge(Utc::now()).await {
                tracing::warn!("upload expiry failed: {error}");
            }
        }
    }
}

fn clone_record(record: &Record) -> Record {
    Record {
        owner: record.owner.clone(),
        name: record.name.clone(),
        media_type: record.media_type.clone(),
        bytes: record.bytes,
        expires_at: record.expires_at,
        session: record.session.clone(),
        bin: record.bin.clone(),
        meta: record.meta.clone(),
    }
}

fn created(id: &str, record: &Record) -> UploadCreated {
    UploadCreated {
        id: id.to_owned(),
        uri: record.bin.display().to_string(),
        name: record.name.clone(),
        bytes: record.bytes,
        media_type: record.media_type.clone(),
        expires_at: record.expires_at,
        session: record.session.clone(),
    }
}

fn validate(upload: &NewUpload<'_>, max_bytes: u64, ttl: Duration) -> Result<(), UploadError> {
    if ttl.is_zero() {
        return Err(UploadError::Ttl);
    }
    if !valid_name(upload.name) {
        return Err(UploadError::InvalidName);
    }
    if !valid_media_type(upload.media_type) {
        return Err(UploadError::InvalidMediaType);
    }
    if upload.bytes.is_empty() {
        return Err(UploadError::Empty);
    }
    let len =
        u64::try_from(upload.bytes.len()).map_err(|_| UploadError::TooLarge { max: max_bytes })?;
    if len > max_bytes {
        return Err(UploadError::TooLarge { max: max_bytes });
    }
    Ok(())
}

fn parse_id(id: &str) -> Result<Uuid, UploadError> {
    Uuid::parse_str(id).map_err(|_| UploadError::InvalidId)
}

async fn wait_until(deadline: Option<DateTime<Utc>>) {
    let Some(deadline) = deadline else {
        std::future::pending::<()>().await;
        return;
    };
    let now = Utc::now();
    if deadline <= now {
        return;
    }
    match (deadline - now).to_std() {
        Ok(wait) => tokio::time::sleep(wait).await,
        Err(error) => {
            tracing::warn!("upload expiry wait is not a standard duration ({error})");
            tokio::time::sleep(Duration::from_secs(3_600)).await;
        }
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && name.chars().count() <= 255
        && !name
            .chars()
            .any(|character| character.is_control() || character == '/' || character == '\\')
}

fn valid_media_type(value: &str) -> bool {
    let (base, parameter) = match value.split_once(';') {
        Some((base, parameter)) => (base, Some(parameter.trim())),
        None => (value, None),
    };
    type_pair(base)
        && parameter.is_none_or(media_parameter)
        && axum::http::HeaderValue::from_str(value).is_ok()
}

fn type_pair(value: &str) -> bool {
    let Some((left, right)) = value.split_once('/') else {
        return false;
    };
    media_token(left) && media_token(right) && !value[left.len() + 1..].contains('/')
}

fn media_parameter(value: &str) -> bool {
    let Some((name, argument)) = value.split_once('=') else {
        return false;
    };
    media_token(name.trim())
        && !argument.is_empty()
        && !argument
            .chars()
            .any(|character| character.is_control() || matches!(character, ';' | '"' | '\\'))
}

fn media_token(value: &str) -> bool {
    !value.is_empty()
        && !value.contains("..")
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '+' | '-' | '.')
        })
}
