use kernel::memory::{SummaryKind, SummaryWatch, WrittenSummary};
use serde::Deserialize;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::PostgresMemory;
use crate::error::Error;
use crate::listen::{Listener, Stopped};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Announced {
    id: Uuid,
    session_key: String,
    kind: SummaryKind,
    depth: i32,
}

impl PostgresMemory {
    pub async fn spawn_summary_watch(
        &self,
        cancel: CancellationToken,
    ) -> Result<JoinHandle<()>, Error> {
        let mut listener = Listener::connect_resilient(
            self.pool.clone(),
            &["rh_summary_written"],
            "summary watch",
            cancel,
        )
        .await
        .map_err(Error::Connection)?;
        let written = self.summaries_written.clone();
        Ok(tokio::spawn(async move {
            loop {
                match listener.next().await {
                    Ok(Some(payload)) => announce(&written, &payload),
                    Ok(None) => {}
                    Err(Stopped) => return,
                }
            }
        }))
    }
}

fn announce(written: &broadcast::Sender<WrittenSummary>, payload: &str) {
    match serde_json::from_str::<Announced>(payload) {
        Ok(announced) => {
            let told = written.send(WrittenSummary {
                id: announced.id,
                session_key: announced.session_key,
                kind: announced.kind,
                depth: announced.depth,
            });
            if told.is_err() {
                tracing::trace!("written summary has no listener");
            }
        }
        Err(error) => tracing::debug!(%error, "summary notification unreadable"),
    }
}

impl SummaryWatch for PostgresMemory {
    fn written(&self) -> broadcast::Receiver<WrittenSummary> {
        self.summaries_written.subscribe()
    }
}
