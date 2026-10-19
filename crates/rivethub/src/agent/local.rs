use std::sync::Arc;

use config::Config;
use config::Environment;
use gateway::Gateway;
use kernel::tasks::TaskWatch;
use memory_sqlite::SqliteStore;
use tokio_util::sync::CancellationToken;
use wire::harness::HarnessId;

use super::AgentError;
use super::ports::Roster;

pub(crate) type LocalEngine = tasks::Engine<SqliteStore, SqliteStore>;

pub(crate) struct LocalTasks {
    engine: Arc<LocalEngine>,
    store: SqliteStore,
    roster: Arc<Roster>,
    directory: tasks::Directory,
    allowed: Option<Vec<HarnessId>>,
}

impl LocalTasks {
    pub(crate) async fn open(
        config: &Config,
        env: &Environment,
        store: &SqliteStore,
        gateway: url::Url,
        activities: &kernel::shutdown::Activities,
    ) -> Result<Option<Self>, AgentError> {
        if config
            .tasks
            .as_ref()
            .is_some_and(|tasks| tasks.enabled == Some(false))
        {
            return Ok(None);
        }
        let standalone = super::assemble::standalone(config, env, gateway).await?;
        let engine = tasks::Engine::new(
            store.clone(),
            store.clone(),
            standalone.directory.clone(),
            standalone.settings,
        )?
        .with_activities(activities.clone());
        Ok(Some(Self {
            engine: Arc::new(engine),
            store: store.clone(),
            roster: Arc::new(standalone.roster),
            directory: standalone.directory,
            allowed: standalone.allowed,
        }))
    }

    pub(crate) fn engine(&self) -> Arc<LocalEngine> {
        Arc::clone(&self.engine)
    }

    pub(crate) fn mount(&self, gateway: Gateway, tools: Vec<String>) -> Gateway {
        let harness = gateway.terminal().and_then(|terminal| terminal.adapter());
        let catalog = Arc::new(tasks::Catalog::new(
            &self.directory,
            self.roster.mesh,
            tools,
            harness,
        ));
        let api = super::ports::api(
            Arc::clone(&self.roster),
            catalog,
            self.allowed.clone(),
            &self.store,
        );
        gateway.with_tasks(api)
    }

    pub(crate) async fn wake_on_changes(
        &self,
        cancel: CancellationToken,
    ) -> Result<(), AgentError> {
        let mut events = TaskWatch::subscribe(&self.store);
        loop {
            tokio::select! {
                biased;
                () = cancel.cancelled() => return Ok(()),
                changed = events.changed() => {
                    changed.map_err(|_| AgentError::WatchClosed)?;
                    self.engine.wake_waiters();
                }
            }
        }
    }
}
