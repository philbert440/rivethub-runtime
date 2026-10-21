use std::collections::BTreeMap;

use async_trait::async_trait;
use kernel::harness::{
    HarnessAdapter, HarnessAdapterError, SessionEvent, SessionOption, SessionSpec, TurnInput,
};
use parking_lot::Mutex;
use tokio::sync::{Semaphore, mpsc, watch};
use wire::harness::ApprovalDecision;
use wire::session_id::SessionId;

#[derive(Debug)]
struct Session {
    sender: mpsc::Sender<SessionEvent>,
    receiver: Option<mpsc::Receiver<SessionEvent>>,
}

#[derive(Debug)]
pub(crate) struct Harness {
    sessions: Mutex<BTreeMap<SessionId, Session>>,
    offered: Mutex<Vec<wire::harness::HarnessOffering>>,
    pub(crate) trace: watch::Sender<Vec<String>>,
    pub(crate) finish: Semaphore,
    pub(crate) specs: Mutex<Vec<SessionSpec>>,
    pub(crate) usage: Mutex<Option<wire::stream::LlmUsage>>,
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

    async fn resume(&self, session: &SessionId) -> Result<(), HarnessAdapterError> {
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
        let traced = match input {
            TurnInput::Text(text) => format!("prompt:{text}"),
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
                format!("prompt:{text}|attached:{}", attached.join(","))
            }
        };
        self.trace.send_modify(|trace| trace.push(traced));
        self.finish.acquire().await.expect("finish turn").forget();
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
        _option: SessionOption,
    ) -> Result<(), HarnessAdapterError> {
        Ok(())
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

    async fn offerings(&self) -> Result<Vec<wire::harness::HarnessOffering>, HarnessAdapterError> {
        Ok(self.offered.lock().clone())
    }

    async fn commands(&self) -> Result<Vec<kernel::harness::HarnessCommands>, HarnessAdapterError> {
        Ok(Vec::new())
    }
}

fn offered(harness_id: wire::harness::HarnessId) -> wire::harness::HarnessOffering {
    use wire::harness::{
        HarnessCapabilities, HarnessControlCapabilities, HarnessObservationCapabilities,
    };
    wire::harness::HarnessOffering {
        harness_id,
        chat: true,
        capabilities: HarnessCapabilities {
            control: HarnessControlCapabilities {
                interrupt: true,
                resume: true,
                approvals: false,
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
