use std::sync::Arc;
use std::time::Duration;

use kernel::terminal::TermSize;
use kernel::terminal::TerminalId;
use tokio::sync::broadcast;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::time::Instant;

use crate::error::Error;
use crate::manager::AttachStream;
use crate::manager::CHUNK_QUEUE;
use crate::manager::TerminalManager;
use crate::manager::TerminalRecord;
use crate::manager::TerminalState;
use crate::manager::registry::Runtime;
use crate::pty;

const DETACH_POLL: Duration = Duration::from_millis(1_000);

impl TerminalManager {
    pub fn attach_stream(&self, id: &TerminalId) -> Result<AttachStream, Error> {
        let mut state = self.inner.state.lock();
        let Some(runtime) = state.get_mut(id) else {
            return Err(Error::UnknownTerminal {
                terminal: id.clone(),
            });
        };
        let Runtime::Pty {
            scrollback,
            fields,
            live,
            exit_code,
            ..
        } = runtime
        else {
            return Err(Error::MuxUnavailable(
                "herdr terminals stream through the herdr attach client — the gateway owns the /term byte path"
                    .to_owned(),
            ));
        };
        let record = Self::pty_record(fields, live.as_ref(), exit_code.as_ref());
        let replay = scrollback.snapshot();
        let Some(live) = live else {
            return Ok(AttachStream {
                record,
                replay,
                live: None,
            });
        };
        let source = live.output.subscribe();
        live.viewers += 1;
        drop(state);
        let (out_tx, out_rx) = mpsc::channel(CHUNK_QUEUE);
        self.spawn_forwarder(id.clone(), source, out_tx, replay.clone());
        Ok(AttachStream {
            record,
            replay,
            live: Some(out_rx),
        })
    }

    pub async fn input(&self, id: &TerminalId, bytes: Vec<u8>) -> Result<(), Error> {
        let writer = {
            let mut state = self.inner.state.lock();
            match state.get_mut(id) {
                None => {
                    return Err(Error::UnknownTerminal {
                        terminal: id.clone(),
                    });
                }
                Some(Runtime::Herdr { .. }) => {
                    return Err(Error::MuxUnavailable(
                        "herdr terminals stream through the herdr attach client — the gateway owns the /term byte path"
                            .to_owned(),
                    ));
                }
                Some(Runtime::Pty {
                    live: Some(live),
                    exit_code: None,
                    ..
                }) => live.writer.clone(),
                Some(Runtime::Pty { .. }) => {
                    return Err(Error::Exited {
                        terminal: id.clone(),
                    });
                }
            }
        };
        writer.send(bytes).await.map_err(|_| Error::Exited {
            terminal: id.clone(),
        })?;
        {
            let mut state = self.inner.state.lock();
            if let Some(Runtime::Pty {
                live: Some(live), ..
            }) = state.get_mut(id)
            {
                live.last_activity = Instant::now();
            }
        }
        Ok(())
    }

    pub fn resize(&self, id: &TerminalId, size: TermSize) -> Result<(), Error> {
        let mut state = self.inner.state.lock();
        match state.get_mut(id) {
            None => Err(Error::UnknownTerminal {
                terminal: id.clone(),
            }),
            Some(Runtime::Herdr { .. }) => Ok(()),
            Some(Runtime::Pty {
                fields,
                live: Some(live),
                exit_code: None,
                ..
            }) => {
                live.master
                    .resize(portable_pty::PtySize {
                        cols: size.cols,
                        rows: size.rows,
                        pixel_width: 0,
                        pixel_height: 0,
                    })
                    .map_err(|error| Error::Spawn(error.to_string()))?;
                fields.size = size;
                Ok(())
            }
            Some(Runtime::Pty { .. }) => Err(Error::Exited {
                terminal: id.clone(),
            }),
        }
    }

    fn spawn_forwarder(
        &self,
        id: TerminalId,
        mut source: broadcast::Receiver<Arc<[u8]>>,
        out: mpsc::Sender<Vec<u8>>,
        replay: Vec<u8>,
    ) {
        let detach_tx = self.inner.detach_tx.clone();
        tokio::spawn(async move {
            let mut alive = out.send(replay).await.is_ok();
            while alive {
                tokio::select! {
                    chunk = source.recv() => match chunk {
                        Ok(chunk) => alive = out.send(chunk.to_vec()).await.is_ok(),
                        Err(
                            broadcast::error::RecvError::Closed
                            | broadcast::error::RecvError::Lagged(_),
                        ) => break,
                    },
                    () = tokio::time::sleep(DETACH_POLL) => {
                        if out.is_closed() {
                            break;
                        }
                    }
                }
            }
            if detach_tx.send(id.clone()).is_err() {
                tracing::warn!(
                    "terminal {id} detach notice dropped — the manager tasks are not running"
                );
            }
        });
    }

    pub(super) fn stream_until_exit(
        &self,
        id: TerminalId,
        mut output: mpsc::Receiver<Vec<u8>>,
        exit: oneshot::Receiver<pty::ExitOutcome>,
    ) {
        let inner = Arc::clone(&self.inner);
        tokio::spawn(async move {
            while let Some(chunk) = output.recv().await {
                let mut state = inner.state.lock();
                let Some(Runtime::Pty {
                    scrollback,
                    live: Some(live),
                    ..
                }) = state.get_mut(&id)
                else {
                    break;
                };
                scrollback.push(&chunk);
                live.last_output = Some(Instant::now());
                live.last_activity = Instant::now();
                let sent = live.output.send(Arc::from(chunk));
                drop(state);
                drop(sent);
            }
            let code = if let Ok(outcome) = exit.await {
                outcome.code
            } else {
                tracing::warn!("terminal {id} exit watch dropped — recorded as exit 0");
                0
            };
            let mut state = inner.state.lock();
            if let Some(Runtime::Pty {
                live,
                exit_code,
                exited_at,
                exit_watch,
                ..
            }) = state.get_mut(&id)
            {
                live.take();
                exit_code.get_or_insert(code);
                exited_at.get_or_insert_with(Instant::now);
                exit_watch.send_replace(TerminalState::Exited { code });
            }
        });
    }

    pub async fn wait_exited(&self, id: &TerminalId) -> Result<TerminalRecord, Error> {
        let mut exit = {
            let state = self.inner.state.lock();
            match state.get(id) {
                Some(Runtime::Pty { exit_watch, .. }) => exit_watch.subscribe(),
                Some(Runtime::Herdr { .. }) => {
                    return Err(Error::MuxUnavailable(
                        "herdr terminals have no manager-visible exit — the gateway owns the /term byte path"
                            .to_owned(),
                    ));
                }
                None => {
                    return Err(Error::UnknownTerminal {
                        terminal: id.clone(),
                    });
                }
            }
        };
        while *exit.borrow_and_update() == TerminalState::Running {
            if exit.changed().await.is_err() {
                break;
            }
        }
        self.list()
            .into_iter()
            .find(|record| record.id == *id)
            .ok_or_else(|| Error::UnknownTerminal {
                terminal: id.clone(),
            })
    }

    pub(super) fn mark_detached(&self, id: &TerminalId) {
        let mut state = self.inner.state.lock();
        if let Some(Runtime::Pty {
            live: Some(live), ..
        }) = state.get_mut(id)
        {
            live.viewers = live.viewers.saturating_sub(1);
            if live.viewers == 0 {
                live.detached_since = Instant::now();
            }
        }
    }
}
