use kernel::terminal::TerminalId;
use tokio::sync::broadcast;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::sync::watch;
use tokio::time::Instant;

use crate::error::Error;
use crate::herdr::HerdrSession;
use crate::manager::Backing;
use crate::manager::CHUNK_QUEUE;
use crate::manager::SpawnSpec;
use crate::manager::TerminalManager;
use crate::manager::TerminalState;
use crate::manager::registry::LivePty;
use crate::manager::registry::RecordFields;
use crate::manager::registry::Runtime;
use crate::mux::HerdrSetup;
use crate::pty;
use crate::scrollback::Scrollback;

const WRITER_QUEUE: usize = 64;
const BROADCAST_CAPACITY: usize = 256;

impl TerminalManager {
    pub(super) fn start_pty(&self, spec: &SpawnSpec) -> Result<TerminalId, Error> {
        let env = self.inner.settings.child_env(
            spec.session.as_ref(),
            &spec.roster_key,
            &spec.cwd,
            &spec.entry_env,
        );
        let now = Instant::now();
        let id = self.fresh_id();
        let fields = RecordFields::new(spec, id.clone(), Backing::Pty);
        match pty::spawn(&spec.argv, &spec.cwd, spec.size.into(), &env)? {
            pty::SpawnOutcome::MissingBinary => {
                let mut scrollback = Scrollback::new(self.inner.settings.limits.scrollback_bytes);
                scrollback.push(pty::MISSING_BINARY_OUTPUT);
                let (exit_watch, _) = watch::channel(TerminalState::Exited {
                    code: pty::MISSING_BINARY_EXIT,
                });
                self.inner.state.lock().insert(
                    id.clone(),
                    Runtime::Pty {
                        scrollback,
                        fields,
                        live: None,
                        exit_code: Some(pty::MISSING_BINARY_EXIT),
                        exited_at: Some(now),
                        exit_watch,
                    },
                );
                self.bind_session(&id, spec.session.as_ref());
                Ok(id)
            }
            pty::SpawnOutcome::Live(spawned) => {
                let (writer_tx, writer_rx) = mpsc::channel(WRITER_QUEUE);
                let (output_tx, _) = broadcast::channel(BROADCAST_CAPACITY);
                let (reader_tx, reader_rx) = mpsc::channel(CHUNK_QUEUE);
                let (exit_tx, exit_rx) = oneshot::channel();
                let (exit_watch, _) = watch::channel(TerminalState::Running);
                let master_reader = spawned
                    .master
                    .try_clone_reader()
                    .map_err(|error| Error::Spawn(error.to_string()))?;
                let master_writer = spawned
                    .master
                    .take_writer()
                    .map_err(|error| Error::Spawn(error.to_string()))?;
                pty::spawn_reader(master_reader, reader_tx);
                pty::spawn_writer(master_writer, writer_rx);
                pty::spawn_exit_watcher(spawned.child, exit_tx);
                let live = LivePty {
                    master: spawned.master,
                    pid: spawned.pid,
                    writer: writer_tx,
                    output: output_tx,
                    viewers: 0,
                    detached_since: now,
                    last_activity: now,
                    last_output: None,
                    hangup_sent: false,
                };
                self.inner.state.lock().insert(
                    id.clone(),
                    Runtime::Pty {
                        scrollback: Scrollback::new(self.inner.settings.limits.scrollback_bytes),
                        fields,
                        live: Some(live),
                        exit_code: None,
                        exited_at: None,
                        exit_watch,
                    },
                );
                self.bind_session(&id, spec.session.as_ref());
                self.stream_until_exit(id.clone(), reader_rx, exit_rx);
                Ok(id)
            }
        }
    }

    pub(super) async fn start_herdr(
        &self,
        setup: &HerdrSetup,
        spec: SpawnSpec,
    ) -> Result<TerminalId, Error> {
        let Some(session) = &spec.session else {
            return Err(Error::Spawn(
                "herdr terminals are keyed by session; the gateway mints the key".to_owned(),
            ));
        };
        let herdr_session = HerdrSession::ensure(
            setup,
            session,
            &spec.argv,
            &spec.cwd,
            &spec.entry_env,
            spec.roster_key.as_str(),
        )
        .await?;
        let id = self.fresh_id();
        let fields = RecordFields::new(&spec, id.clone(), Backing::Herdr);
        self.inner.state.lock().insert(
            id.clone(),
            Runtime::Herdr {
                fields,
                session: herdr_session,
            },
        );
        self.bind_session(&id, spec.session.as_ref());
        Ok(id)
    }
}
