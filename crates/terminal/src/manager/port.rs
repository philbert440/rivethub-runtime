use kernel::terminal::TerminalId;
use kernel::terminal::TerminalMuxError;
use tokio::sync::mpsc;

use crate::manager::registry::Runtime;
use crate::mux::MuxPlan;
use crate::{Error, SpawnSpec, TerminalManager};

impl TerminalManager {
    pub(super) async fn port_spawn(&self, spec: SpawnSpec) -> Result<TerminalId, Error> {
        match &self.inner.plan {
            MuxPlan::Herdr(_) => self.port_herdr(spec).await,
            MuxPlan::Direct => {
                let manager = self.clone();
                self.inner
                    .tasks
                    .spawn_blocking(move || manager.start_pty(&spec))
                    .await
                    .map_err(Error::Worker)?
            }
            MuxPlan::Unavailable(unavailable) => Err(unavailable.reason.to_error()),
        }
    }

    pub(super) fn port_turn_ends(
        &self,
        id: &TerminalId,
    ) -> Result<mpsc::Receiver<()>, TerminalMuxError> {
        let server = self.inner.port_backends.lock().get(id).cloned();
        let events = {
            let state = self.inner.state.lock();
            match server.as_ref().and_then(|server| state.get(server)) {
                Some(Runtime::Herdr { session, .. }) => session.client.subscribe_events(),
                Some(Runtime::Pty { .. }) | None => {
                    return Err(TerminalMuxError::Unsupported("turn_ends"));
                }
            }
        };
        let (ends, receiver) = mpsc::channel(4);
        let shutdown = self.inner.shutdown.clone();
        self.inner.tasks.spawn(async move {
            tokio::select! {
                () = shutdown.cancelled() => {}
                () = crate::herdr::turns::follow(events, ends) => {}
            }
        });
        Ok(receiver)
    }

    async fn port_herdr(&self, spec: SpawnSpec) -> Result<TerminalId, Error> {
        let server = self.start(spec.clone()).await?;
        let session = {
            let state = self.inner.state.lock();
            match state.get(&server) {
                Some(Runtime::Herdr { session, .. }) => session.clone(),
                Some(Runtime::Pty { .. }) | None => {
                    return Err(Error::MissingServer);
                }
            }
        };
        let mut attached = spec;
        attached.argv = vec![session.cli.binary.to_string_lossy().into_owned()];
        attached.argv.extend(session.cli.attach());
        attached.entry_env.insert(
            crate::herdr::CONFIG_HOME_ENV.into(),
            session.config_home.to_string_lossy().into_owned(),
        );
        attached.entry_env.insert(
            crate::herdr::RUNTIME_DIR_ENV.into(),
            session.config_home.to_string_lossy().into_owned(),
        );
        let manager = self.clone();
        let outcome = self
            .inner
            .tasks
            .spawn_blocking(move || manager.start_pty(&attached))
            .await
            .map_err(Error::Worker)?;
        match outcome {
            Ok(id) => {
                self.inner.port_backends.lock().insert(id.clone(), server);
                Ok(id)
            }
            Err(error) => {
                self.kill(&server).await?;
                Err(error)
            }
        }
    }
}
