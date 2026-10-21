use std::sync::Arc;
use std::time::Duration;

use tokio::io::AsyncBufRead;
use tokio::io::AsyncBufReadExt;
use tokio::sync::Mutex;
use tokio::sync::Notify;

pub(super) struct Logs {
    lines: Arc<Mutex<Vec<String>>>,
    notify: Arc<Notify>,
}

impl Logs {
    pub(super) fn capture(reader: impl AsyncBufRead + Unpin + Send + 'static) -> Self {
        let lines = Arc::new(Mutex::new(Vec::new()));
        let notify = Arc::new(Notify::new());
        let stored = Arc::clone(&lines);
        let wake = Arc::clone(&notify);
        drop(tokio::spawn(async move {
            let mut incoming = reader.lines();
            while let Some(line) = incoming.next_line().await? {
                stored.lock().await.push(line);
                wake.notify_one();
            }
            Ok::<(), std::io::Error>(())
        }));
        Self { lines, notify }
    }

    pub(super) async fn wait_for(&self, needle: &str) -> Result<(), String> {
        let waited = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let notified = self.notify.notified();
                if self.contains(needle).await {
                    return;
                }
                notified.await;
            }
        })
        .await;
        match waited {
            Ok(()) => Ok(()),
            Err(_elapsed) => Err(format!(
                "timed out waiting for {needle}\n{}",
                self.text().await
            )),
        }
    }

    pub(super) async fn count(&self, needle: &str) -> usize {
        self.lines
            .lock()
            .await
            .iter()
            .filter(|line| line.contains(needle))
            .count()
    }

    pub(super) async fn text(&self) -> String {
        self.lines.lock().await.join("\n")
    }

    async fn contains(&self, needle: &str) -> bool {
        self.lines
            .lock()
            .await
            .iter()
            .any(|line| line.contains(needle))
    }
}
