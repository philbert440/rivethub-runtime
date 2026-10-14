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
pub struct Harness {
    sessions: Mutex<BTreeMap<SessionId, Session>>,
    pub trace: watch::Sender<Vec<String>>,
    pub finish: Semaphore,
}

impl Default for Harness {
    fn default() -> Self {
        Self {
            sessions: Mutex::new(BTreeMap::new()),
            trace: watch::channel(Vec::new()).0,
            finish: Semaphore::new(0),
        }
    }
}

impl Harness {
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

    pub async fn traced(&self, wanted: &str) {
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
        let TurnInput::Text(text) = input;
        self.trace
            .send_modify(|trace| trace.push(format!("prompt:{text}")));
        self.finish.acquire().await.expect("finish turn").forget();
        sender
            .send(SessionEvent::AgentMessageChunk {
                text: "answer".into(),
            })
            .await
            .expect("events");
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
        _request_id: &str,
        _decision: ApprovalDecision,
    ) -> Result<(), HarnessAdapterError> {
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
}
