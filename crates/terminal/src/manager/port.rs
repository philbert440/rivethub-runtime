use kernel::terminal::TerminalId;

use crate::manager::registry::Runtime;
use crate::mux::MuxPlan;
use crate::{Error, SpawnSpec, TerminalManager};

impl TerminalManager {
    pub(super) async fn port_spawn(&self, spec: SpawnSpec) -> Result<TerminalId, Error> {
        match &self.inner.plan {
            MuxPlan::Herdr(_) => self.port_herdr(spec).await,
            MuxPlan::Direct => {
                let manager = self.clone();
                tokio::task::spawn_blocking(move || manager.start_pty(&spec))
                    .await
                    .map_err(|error| Error::Spawn(error.to_string()))?
            }
            MuxPlan::Unavailable(unavailable) => Err(unavailable.reason.to_error()),
        }
    }

    async fn port_herdr(&self, spec: SpawnSpec) -> Result<TerminalId, Error> {
        let server = self.start(spec.clone()).await?;
        let session = {
            let state = self.inner.state.lock();
            match state.get(&server) {
                Some(Runtime::Herdr { session, .. }) => session.clone(),
                Some(Runtime::Pty { .. }) | None => {
                    return Err(Error::Spawn("herdr server record missing".into()));
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
        let manager = self.clone();
        let outcome = tokio::task::spawn_blocking(move || manager.start_pty(&attached))
            .await
            .map_err(|error| Error::Spawn(error.to_string()))?;
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
