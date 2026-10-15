use std::sync::Arc;

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
            return Err(Error::HerdrStream);
        };
        if let Some(Err(error)) = exit_code {
            return Err(Error::PtyFailure(Arc::clone(error)));
        }
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
        self.spawn_forwarder(id.clone(), source, out_tx);
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
                    return Err(Error::HerdrStream);
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
        let (result, written) = oneshot::channel();
        writer
            .send(pty::WriteRequest { bytes, result })
            .await
            .map_err(|_| Error::Exited {
                terminal: id.clone(),
            })?;
        written.await??;
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
                    .map_err(|error| Error::PtyBackend(error.into()))?;
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
    ) {
        let detach_tx = self.inner.detach_tx.clone();
        let shutdown = self.inner.shutdown.clone();
        self.inner.tasks.spawn(async move {
            let mut alive = true;
            while alive {
                tokio::select! {
                    chunk = source.recv() => match chunk {
                        Ok(chunk) => {
                            alive = tokio::select! {
                                () = shutdown.cancelled() => break,
                                sent = out.send(chunk.to_vec()) => sent.is_ok(),
                            };
                        },
                        Err(
                            broadcast::error::RecvError::Closed
                            | broadcast::error::RecvError::Lagged(_),
                        ) => break,
                    },
                    () = out.closed() => break,
                    () = shutdown.cancelled() => break,
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
        mut output: mpsc::Receiver<Result<Vec<u8>, Error>>,
        exit: oneshot::Receiver<Result<pty::ExitOutcome, Error>>,
    ) {
        let inner = Arc::clone(&self.inner);
        self.inner.tasks.spawn(async move {
            let mut failure = None;
            while let Some(chunk) = output.recv().await {
                let chunk = match chunk {
                    Ok(chunk) => chunk,
                    Err(error) => {
                        failure = Some(error);
                        break;
                    }
                };
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
            let outcome = match failure {
                Some(error) => Err(error),
                None => match exit.await {
                    Ok(result) => result.map(|outcome| outcome.code),
                    Err(error) => Err(Error::WorkerClosed(error)),
                },
            };
            let terminal_state = match &outcome {
                Ok(code) => TerminalState::Exited { code: *code },
                Err(error) => {
                    tracing::error!(terminal = %id, %error, "terminal worker failed");
                    TerminalState::Failed
                }
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
                *exit_code = Some(outcome.map_err(Arc::new));
                exited_at.get_or_insert_with(Instant::now);
                exit_watch.send_replace(terminal_state);
            }
        });
    }

    pub async fn wait_exited(&self, id: &TerminalId) -> Result<TerminalRecord, Error> {
        let mut exit = {
            let state = self.inner.state.lock();
            match state.get(id) {
                Some(Runtime::Pty { exit_watch, .. }) => exit_watch.subscribe(),
                Some(Runtime::Herdr { .. }) => {
                    return Err(Error::HerdrExit);
                }
                None => {
                    return Err(Error::UnknownTerminal {
                        terminal: id.clone(),
                    });
                }
            }
        };
        while *exit.borrow_and_update() == TerminalState::Running {
            exit.changed().await?;
        }
        if let Some(Runtime::Pty {
            exit_code: Some(Err(error)),
            ..
        }) = self.inner.state.lock().get(id)
        {
            return Err(Error::PtyFailure(Arc::clone(error)));
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
