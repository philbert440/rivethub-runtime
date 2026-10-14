use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::RwLock;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use uuid::Uuid;
use wire::gateway::v1::Conversation;
use wire::stream::openai::Message;

use super::Error;

#[derive(Debug)]
pub(super) struct Store {
    directory: PathBuf,
    entries: Arc<RwLock<BTreeMap<Uuid, Arc<Entry>>>>,
}

#[derive(Debug)]
pub(super) struct Entry {
    snapshot: Arc<RwLock<Option<Conversation>>>,
    sender: mpsc::Sender<Command>,
    busy: AtomicBool,
}

#[derive(Debug)]
pub(super) struct Lease(Arc<Entry>);

impl Drop for Lease {
    fn drop(&mut self) {
        self.0.busy.store(false, Ordering::Release);
    }
}

#[derive(Debug)]
enum Change {
    Title(String),
    Messages(Vec<Message>),
    Delete,
}

#[derive(Debug)]
struct Command {
    change: Change,
    now: i64,
    reply: oneshot::Sender<Result<Option<Conversation>, Error>>,
}

impl Store {
    pub(super) async fn open(
        directory: &Path,
        tasks: &TaskTracker,
        cancel: &CancellationToken,
    ) -> Result<Self, Error> {
        let directory = directory.to_path_buf();
        let (directory, records) = tokio::task::spawn_blocking(move || load(&directory)).await??;
        let entries = records
            .into_iter()
            .map(|(id, record)| {
                let entry = start(directory.join(format!("{id}.json")), record, tasks, cancel);
                (id, entry)
            })
            .collect();
        Ok(Self {
            directory,
            entries: Arc::new(RwLock::new(entries)),
        })
    }

    pub(super) fn list(&self) -> Vec<Conversation> {
        #[expect(
            clippy::needless_collect,
            reason = "release the registry lock before taking individual conversation locks"
        )]
        let entries: Vec<_> = self.entries.read().values().cloned().collect();
        let mut records: Vec<_> = entries
            .into_iter()
            .filter_map(|entry| entry.snapshot.read().clone())
            .collect();
        records.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        records
    }

    pub(super) fn entry(&self, id: Uuid) -> Result<Arc<Entry>, Error> {
        self.entries.read().get(&id).cloned().ok_or(Error::NotFound)
    }

    pub(super) async fn create(
        &self,
        title: String,
        now: i64,
        tasks: &TaskTracker,
        cancel: &CancellationToken,
    ) -> Result<Conversation, Error> {
        if cancel.is_cancelled() {
            return Err(Error::Unavailable);
        }
        let id = Uuid::new_v4();
        let record = Conversation {
            id: id.to_string(),
            title,
            created_at: now,
            updated_at: now,
            messages: Vec::new(),
        };
        let path = self.directory.join(format!("{id}.json"));
        let entries = Arc::clone(&self.entries);
        let cancel = cancel.clone();
        let tracker = tasks.clone();
        let (reply, receiver) = oneshot::channel();
        tasks.spawn(async move {
            let result = async {
                persist(path.clone(), Some(record.clone()), true).await?;
                let entry = start(path, record.clone(), &tracker, &cancel);
                entries.write().insert(id, entry);
                Ok::<_, Error>(record)
            }
            .await;
            if let Err(result) = reply.send(result)
                && let Err(error) = result
            {
                tracing::warn!(%error, "conversation creation failed after request disconnected");
            }
        });
        receiver.await.map_err(|_| Error::Unavailable)?
    }

    pub(super) async fn delete(&self, id: Uuid, now: i64) -> Result<(), Error> {
        let entry = self.entry(id)?;
        let _lease = entry.lease()?;
        entry.change(Change::Delete, now).await?;
        self.entries.write().remove(&id);
        Ok(())
    }
}

impl Entry {
    pub(super) fn snapshot(&self) -> Result<Conversation, Error> {
        self.snapshot.read().clone().ok_or(Error::NotFound)
    }

    pub(super) fn lease(self: &Arc<Self>) -> Result<Lease, Error> {
        self.busy
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .map_err(|_| Error::Busy)?;
        Ok(Lease(Arc::clone(self)))
    }

    pub(super) async fn rename(
        self: &Arc<Self>,
        title: String,
        now: i64,
    ) -> Result<Conversation, Error> {
        let _lease = self.lease()?;
        self.change(Change::Title(title), now)
            .await?
            .ok_or(Error::NotFound)
    }

    async fn change(&self, change: Change, now: i64) -> Result<Option<Conversation>, Error> {
        let (reply, receiver) = oneshot::channel();
        self.sender
            .send(Command { change, now, reply })
            .await
            .map_err(|_| Error::Unavailable)?;
        receiver.await.map_err(|_| Error::Unavailable)?
    }
}

impl Lease {
    pub(super) async fn save(&self, messages: Vec<Message>, now: i64) -> Result<(), Error> {
        self.0
            .change(Change::Messages(messages), now)
            .await?
            .ok_or(Error::NotFound)?;
        Ok(())
    }
}

fn start(
    path: PathBuf,
    record: Conversation,
    tasks: &TaskTracker,
    cancel: &CancellationToken,
) -> Arc<Entry> {
    let snapshot = Arc::new(RwLock::new(Some(record)));
    let (sender, receiver) = mpsc::channel(16);
    tasks.spawn(serve(
        path,
        Arc::clone(&snapshot),
        receiver,
        cancel.child_token(),
    ));
    Arc::new(Entry {
        snapshot,
        sender,
        busy: AtomicBool::new(false),
    })
}

async fn serve(
    path: PathBuf,
    snapshot: Arc<RwLock<Option<Conversation>>>,
    mut receiver: mpsc::Receiver<Command>,
    cancel: CancellationToken,
) {
    loop {
        let command = tokio::select! {
            biased;
            () = cancel.cancelled() => return,
            command = receiver.recv() => match command { Some(command) => command, None => return },
        };
        let current = snapshot.read().clone();
        let result = apply(path.clone(), current, command.change, command.now).await;
        if let Ok(record) = &result {
            snapshot.write().clone_from(record);
        }
        let deleted = matches!(&result, Ok(None));
        if let Err(result) = command.reply.send(result)
            && let Err(error) = result
        {
            tracing::warn!(%error, "conversation write failed after request disconnected");
        }
        if deleted {
            return;
        }
    }
}

async fn apply(
    path: PathBuf,
    current: Option<Conversation>,
    change: Change,
    now: i64,
) -> Result<Option<Conversation>, Error> {
    let mut record = current.ok_or(Error::NotFound)?;
    match change {
        Change::Title(title) => record.title = title,
        Change::Messages(messages) => record.messages = messages,
        Change::Delete => {
            persist(path, None, false).await?;
            return Ok(None);
        }
    }
    record.updated_at = now.max(record.updated_at);
    persist(path, Some(record.clone()), false).await?;
    Ok(Some(record))
}

fn load(directory: &Path) -> Result<(PathBuf, BTreeMap<Uuid, Conversation>), Error> {
    if directory
        .components()
        .any(|part| part == std::path::Component::ParentDir)
    {
        return Err(Error::Invalid(
            "parent traversal in conversation directory".to_owned(),
        ));
    }
    std::fs::create_dir_all(directory)?;
    if std::fs::symlink_metadata(directory)?
        .file_type()
        .is_symlink()
    {
        return Err(Error::Invalid(
            "conversation directory cannot be a symlink".to_owned(),
        ));
    }
    let root = std::fs::canonicalize(directory)?;
    let mut records = BTreeMap::new();
    for item in std::fs::read_dir(&root)? {
        let item = item?;
        let path = item.path();
        if path.extension() != Some(std::ffi::OsStr::new("json")) {
            continue;
        }
        contained_file(&root, &path)?;
        let record: Conversation = serde_json::from_slice(&std::fs::read(&path)?)?;
        let id = Uuid::parse_str(&record.id).map_err(|error| Error::Invalid(error.to_string()))?;
        if path != root.join(format!("{id}.json")) || records.insert(id, record).is_some() {
            return Err(Error::Invalid("invalid conversation filename".to_owned()));
        }
    }
    Ok((root, records))
}

fn contained_file(root: &Path, path: &Path) -> Result<(), Error> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || !std::fs::canonicalize(path)?.starts_with(root)
    {
        return Err(Error::Invalid(
            "conversation path is not a contained file".to_owned(),
        ));
    }
    Ok(())
}

async fn persist(path: PathBuf, record: Option<Conversation>, create: bool) -> Result<(), Error> {
    tokio::task::spawn_blocking(move || {
        let root = path
            .parent()
            .ok_or_else(|| Error::Invalid("conversation directory missing".to_owned()))?;
        if std::fs::canonicalize(root)? != root {
            return Err(Error::Invalid("conversation directory changed".to_owned()));
        }
        if !create {
            contained_file(root, &path)?;
        }
        if let Some(record) = record {
            let mut file = tempfile::NamedTempFile::new_in(root)?;
            serde_json::to_writer(&mut file, &record)?;
            file.flush()?;
            file.as_file().sync_all()?;
            if create {
                file.persist_noclobber(&path)
            } else {
                file.persist(&path)
            }
            .map_err(|error| Error::Io(error.error))?;
        } else {
            std::fs::remove_file(&path)?;
        }
        std::fs::File::open(root)?.sync_all()?;
        Ok::<_, Error>(())
    })
    .await?
}
