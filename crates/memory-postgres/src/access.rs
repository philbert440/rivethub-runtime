use std::collections::BTreeSet;
use std::sync::Arc;

use parking_lot::Mutex;
use sqlx::PgPool;
use uuid::Uuid;

use crate::Error;

const PENDING_LIMIT: usize = 10_000;

#[derive(Debug, Default)]
pub(crate) struct AccessLog {
    state: Mutex<Pending>,
}

#[derive(Debug, Default)]
struct Pending {
    messages: BTreeSet<Uuid>,
    summaries: BTreeSet<Uuid>,
    dropped: u64,
    writing: bool,
}

impl AccessLog {
    pub(crate) fn record(
        self: &Arc<Self>,
        pool: &PgPool,
        messages: Vec<Uuid>,
        summaries: Vec<Uuid>,
    ) {
        let start = {
            let mut pending = self.state.lock();
            for id in messages {
                pending.add_message(id);
            }
            for id in summaries {
                pending.add_summary(id);
            }
            let idle = !pending.writing && !pending.is_empty();
            if idle {
                pending.writing = true;
            }
            idle
        };
        if start {
            tokio::spawn(Arc::clone(self).drain(pool.clone()));
        }
    }

    async fn drain(self: Arc<Self>, pool: PgPool) {
        loop {
            let (messages, summaries, dropped) = {
                let mut pending = self.state.lock();
                if pending.is_empty() {
                    pending.writing = false;
                    return;
                }
                (
                    std::mem::take(&mut pending.messages),
                    std::mem::take(&mut pending.summaries),
                    std::mem::take(&mut pending.dropped),
                )
            };
            if dropped > 0 {
                tracing::warn!(
                    "search access counts: {dropped} hit(s) dropped while the writer was behind"
                );
            }
            if let Err(error) = write(&pool, &messages, &summaries).await {
                tracing::warn!("search access counts not recorded: {error}");
            }
        }
    }
}

impl Pending {
    fn is_empty(&self) -> bool {
        self.messages.is_empty() && self.summaries.is_empty() && self.dropped == 0
    }

    fn has_capacity(&self) -> bool {
        self.messages.len() + self.summaries.len() < PENDING_LIMIT
    }

    fn add_message(&mut self, id: Uuid) {
        if self.has_capacity() || self.messages.contains(&id) {
            self.messages.insert(id);
        } else {
            self.dropped += 1;
        }
    }

    fn add_summary(&mut self, id: Uuid) {
        if self.has_capacity() || self.summaries.contains(&id) {
            self.summaries.insert(id);
        } else {
            self.dropped += 1;
        }
    }
}

async fn write(
    pool: &PgPool,
    messages: &BTreeSet<Uuid>,
    summaries: &BTreeSet<Uuid>,
) -> Result<(), Error> {
    if !messages.is_empty() {
        let ids: Vec<Uuid> = messages.iter().copied().collect();
        sqlx::query!(
            "update rh_messages set access_count = access_count + 1,
                        last_accessed_at = now() where id = any($1)",
            &ids
        )
        .execute(pool)
        .await
        .map_err(|source| Error::operation("access bump", source))?;
    }
    if !summaries.is_empty() {
        let ids: Vec<Uuid> = summaries.iter().copied().collect();
        sqlx::query!(
            "update rh_summaries set access_count = access_count + 1,
                        last_accessed_at = now() where id = any($1)",
            &ids
        )
        .execute(pool)
        .await
        .map_err(|source| Error::operation("access bump", source))?;
    }
    Ok(())
}
