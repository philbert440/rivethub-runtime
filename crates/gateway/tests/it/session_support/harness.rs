use std::collections::BTreeMap;

use async_trait::async_trait;
use kernel::harness::{ApprovalDecision, SteerOutcome};
use kernel::harness::{
    HarnessAdapter, HarnessAdapterError, SessionEvent, SessionOption, SessionSpec, TurnInput,
};
use parking_lot::Mutex;
use tokio::sync::{Semaphore, mpsc, watch};
use wire::session_id::SessionId;

#[derive(Debug)]
struct Session {
    sender: mpsc::Sender<SessionEvent>,
    receiver: Option<mpsc::Receiver<SessionEvent>>,
}

#[derive(Debug)]
pub(crate) struct Harness {
    sessions: Mutex<BTreeMap<SessionId, Session>>,
    offered: Mutex<Vec<kernel::harness::HarnessOffering>>,
    pub(crate) trace: watch::Sender<Vec<String>>,
    pub(crate) finish: Semaphore,
    pub(crate) specs: Mutex<Vec<SessionSpec>>,
    pub(crate) usage: Mutex<Option<wire::stream::LlmUsage>>,
    pub(crate) refusal: Mutex<Option<String>>,
}

impl Default for Harness {
    fn default() -> Self {
        Self {
            sessions: Mutex::new(BTreeMap::new()),
            offered: Mutex::new(Vec::new()),
            trace: watch::channel(Vec::new()).0,
            finish: Semaphore::new(0),
            specs: Mutex::new(Vec::new()),
            usage: Mutex::new(None),
            refusal: Mutex::new(None),
        }
    }
}

impl Harness {
    pub(crate) fn offering(harnesses: &[wire::harness::HarnessId]) -> Self {
        let harness = Self::default();
        *harness.offered.lock() = harnesses.iter().copied().map(offered).collect();
        harness
    }

    fn opened(&self, id: SessionId) {
        let (sender, receiver) = mpsc::channel(16);
        assert!(
            self.sessions
                .lock()
                .insert(
                    id,
                    Session {
                        sender,
                        receiver: Some(receiver)
                    }
                )
                .is_none()
        );
    }

    pub(crate) async fn emit(&self, session: &SessionId, event: SessionEvent) {
        let sender = self
            .sessions
            .lock()
            .get(session)
            .expect("live session")
            .sender
            .clone();
        sender.send(event).await.expect("events");
    }

    pub(crate) async fn traced(&self, wanted: &str) {
        let mut trace = self.trace.subscribe();
        tokio::time::timeout(super::WAIT, async {
            loop {
                if trace
                    .borrow_and_update()
                    .iter()
                    .any(|entry| entry == wanted)
                {
                    return;
                }
                trace.changed().await.expect("trace open");
            }
        })
        .await
        .expect("trace timeout");
    }
}

#[async_trait]
impl HarnessAdapter for Harness {
    async fn new_session(&self, spec: SessionSpec) -> Result<SessionId, HarnessAdapterError> {
        self.opened(spec.session.clone());
        self.specs.lock().push(spec.clone());
        self.trace.send_modify(|trace| trace.push("new".into()));
        Ok(spec.session)
    }

    async fn resume(
        &self,
        session: &SessionId,
        _cwd: Option<&std::path::Path>,
    ) -> Result<(), HarnessAdapterError> {
        self.opened(session.clone());
        self.trace.send_modify(|trace| trace.push("resume".into()));
        Ok(())
    }

    async fn release(&self, session: &SessionId) -> Result<(), HarnessAdapterError> {
        assert!(self.sessions.lock().remove(session).is_some());
        self.trace.send_modify(|trace| trace.push("release".into()));
        Ok(())
    }

    async fn prompt(
        &self,
        session: &SessionId,
        input: TurnInput,
    ) -> Result<(), HarnessAdapterError> {
        let sender = self
            .sessions
            .lock()
            .get(session)
            .expect("live session")
            .sender
            .clone();
        self.trace
            .send_modify(|trace| trace.push(format!("prompt:{}", described(input))));
        self.finish.acquire().await.expect("finish turn").forget();
        let refusal = self.refusal.lock().take();
        if let Some(message) = refusal {
            return Err(HarnessAdapterError::Failed {
                message,
                source: Box::new(std::io::Error::other("refused")),
            });
        }
        sender
            .send(SessionEvent::AgentMessageChunk {
                text: "answer".into(),
            })
            .await
            .expect("events");
        let usage = self.usage.lock().clone();
        if let Some(usage) = usage {
            sender
                .send(SessionEvent::Usage { usage })
                .await
                .expect("usage");
        }
        sender
            .send(SessionEvent::TurnEnded)
            .await
            .expect("turn end");
        self.trace.send_modify(|trace| trace.push("ended".into()));
        Ok(())
    }

    async fn steer(
        &self,
        _session: &SessionId,
        input: TurnInput,
    ) -> Result<SteerOutcome, HarnessAdapterError> {
        self.trace
            .send_modify(|trace| trace.push(format!("steer:{}", described(input))));
        Ok(SteerOutcome::Steered)
    }

    async fn cancel(&self, _session: &SessionId) -> Result<(), HarnessAdapterError> {
        self.finish.add_permits(1);
        Ok(())
    }

    async fn answer_permission(
        &self,
        _session: &SessionId,
        request_id: &str,
        decision: ApprovalDecision,
    ) -> Result<(), HarnessAdapterError> {
        self.trace
            .send_modify(|trace| trace.push(format!("answer:{request_id}:{decision:?}")));
        Ok(())
    }

    async fn set_option(
        &self,
        _session: &SessionId,
        option: SessionOption,
    ) -> Result<(), HarnessAdapterError> {
        self.trace
            .send_modify(|trace| trace.push(format!("{}:{option:?}", option.name())));
        Ok(())
    }

    fn switches(&self, _session: &SessionId) -> kernel::harness::SessionSwitches {
        kernel::harness::SessionSwitches {
            model: true,
            effort: false,
        }
    }

    fn admit(
        &self,
        _session: &SessionId,
        option: &SessionOption,
    ) -> Result<(), HarnessAdapterError> {
        match option {
            SessionOption::Model(_) => Ok(()),
            SessionOption::Mode(_) | SessionOption::Effort(_) => {
                Err(HarnessAdapterError::OptionUnavailable {
                    option: option.name().to_owned(),
                })
            }
        }
    }

    async fn events(
        &self,
        session: &SessionId,
    ) -> Result<mpsc::Receiver<SessionEvent>, HarnessAdapterError> {
        Ok(self
            .sessions
            .lock()
            .get_mut(session)
            .expect("live session")
            .receiver
            .take()
            .expect("one consumer"))
    }

    async fn offerings(
        &self,
    ) -> Result<Vec<kernel::harness::HarnessOffering>, HarnessAdapterError> {
        Ok(self.offered.lock().clone())
    }

    async fn commands(&self) -> Result<Vec<kernel::harness::HarnessCommands>, HarnessAdapterError> {
        Ok(Vec::new())
    }
}

fn described(input: TurnInput) -> String {
    match input {
        TurnInput::Text(text) => text,
        TurnInput::Attached { text, attachments } => {
            let attached: Vec<String> = attachments
                .iter()
                .map(|attachment| {
                    format!(
                        "{}:{}:{}",
                        attachment.name,
                        attachment.media_type,
                        attachment.path.display()
                    )
                })
                .collect();
            format!("{text}|attached:{}", attached.join(","))
        }
    }
}

fn offered(harness_id: wire::harness::HarnessId) -> kernel::harness::HarnessOffering {
    use kernel::harness::{
        HarnessCapabilities, HarnessControlCapabilities, HarnessObservationCapabilities,
    };
    kernel::harness::HarnessOffering {
        harness_id,
        chat: true,
        capabilities: HarnessCapabilities {
            control: HarnessControlCapabilities {
                interrupt: true,
                resume: true,
                approvals: false,
                steering: None,
            },
            observation: HarnessObservationCapabilities {
                live_stream: true,
                list_sessions: false,
                drive: None,
            },
            turn_options: None,
            launch_model: None,
            image_attachments: None,
            models: None,
            models_source: None,
            efforts: None,
            model_flag: None,
            effort_flag: None,
        },
    }
}
