use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::Utc;
use tokio::io::AsyncWriteExt as _;
use tokio::sync::mpsc;
use wire::plugin::PluginId;
use wire::plugin::host::{PluginLogLine, PluginLogStream};

const ROTATE_AT: u64 = 5 * 1024 * 1024;
const LOG_FILE: &str = "plugin.log";
const PREVIOUS_LOG_FILE: &str = "plugin.log.1";
pub(super) const DEFAULT_TAIL: usize = 200;
pub(super) const MAX_TAIL: usize = 2000;

#[derive(Debug, Clone)]
pub(super) struct LogSink {
    lines: mpsc::UnboundedSender<(PathBuf, PluginLogLine)>,
}

impl LogSink {
    pub(super) fn open(
        tasks: &tokio_util::task::TaskTracker,
        shutdown: tokio_util::sync::CancellationToken,
    ) -> Self {
        let (lines, received) = mpsc::unbounded_channel();
        tasks.spawn(write_lines(received, shutdown));
        Self { lines }
    }

    pub(super) fn write(
        &self,
        folder: &Path,
        plugin: &PluginId,
        stream: PluginLogStream,
        text: String,
    ) {
        if stream == PluginLogStream::Host {
            tracing::info!(plugin = plugin.as_ref(), "{text}");
        }
        let line = PluginLogLine {
            at: Utc::now(),
            stream,
            text,
        };
        if self.lines.send((folder.to_path_buf(), line)).is_err() {
            tracing::debug!(plugin = plugin.as_ref(), "plug-in log writer has stopped");
        }
    }
}

struct OpenLog {
    file: tokio::fs::File,
    size: u64,
}

async fn write_lines(
    mut received: mpsc::UnboundedReceiver<(PathBuf, PluginLogLine)>,
    shutdown: tokio_util::sync::CancellationToken,
) {
    let mut open: BTreeMap<PathBuf, OpenLog> = BTreeMap::new();
    loop {
        let next = tokio::select! {
            biased;
            next = received.recv() => next,
            () = shutdown.cancelled() => {
                received.close();
                received.recv().await
            }
        };
        let Some((folder, line)) = next else {
            return;
        };
        if let Err(error) = append(&mut open, &folder, &line).await {
            tracing::warn!(%error, folder = %folder.display(), "plug-in log line not written");
            open.remove(&folder);
        }
    }
}

async fn append(
    open: &mut BTreeMap<PathBuf, OpenLog>,
    folder: &Path,
    line: &PluginLogLine,
) -> Result<(), std::io::Error> {
    let mut bytes = serde_json::to_vec(line)?;
    bytes.push(b'\n');
    let rotate = open
        .get(folder)
        .is_some_and(|log| log.size + bytes.len() as u64 > ROTATE_AT);
    if rotate {
        open.remove(folder);
        tokio::fs::rename(folder.join(LOG_FILE), folder.join(PREVIOUS_LOG_FILE)).await?;
    }
    let log = match open.entry(folder.to_path_buf()) {
        std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
        std::collections::btree_map::Entry::Vacant(entry) => {
            tokio::fs::create_dir_all(folder).await?;
            let file = tokio::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(folder.join(LOG_FILE))
                .await?;
            let size = file.metadata().await?.len();
            entry.insert(OpenLog { file, size })
        }
    };
    log.file.write_all(&bytes).await?;
    log.file.flush().await?;
    log.size += bytes.len() as u64;
    Ok(())
}

pub(super) async fn tail(
    folder: &Path,
    count: usize,
) -> Result<Vec<PluginLogLine>, std::io::Error> {
    let mut lines = Vec::new();
    for name in [PREVIOUS_LOG_FILE, LOG_FILE] {
        match tokio::fs::read_to_string(folder.join(name)).await {
            Ok(text) => {
                for raw in text.lines() {
                    match serde_json::from_str::<PluginLogLine>(raw) {
                        Ok(line) => lines.push(line),
                        Err(error) => {
                            tracing::debug!(%error, "unreadable plug-in log line skipped");
                        }
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    let skip = lines.len().saturating_sub(count);
    Ok(lines.split_off(skip))
}
