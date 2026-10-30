use std::sync::Arc;

use tokio::sync::broadcast;
use tokio::sync::mpsc;

use crate::Error;
use crate::herdr::AgentKind;
use crate::herdr::HerdrEventKind;
use crate::herdr::codec::EventFrame;

#[derive(Debug, Default)]
pub struct TurnWatch {
    running: bool,
}

impl TurnWatch {
    pub fn ended(&mut self, frame: &EventFrame) -> bool {
        if frame.event != HerdrEventKind::PaneAgentStatusChanged {
            return false;
        }
        let reports = frame
            .data
            .get("agent")
            .and_then(|agent| agent.as_str())
            .is_some_and(|agent| {
                agent
                    .parse::<AgentKind>()
                    .is_ok_and(AgentKind::reports_turns)
            });
        let status = frame
            .data
            .get("agent_status")
            .and_then(|status| status.as_str());
        match status.filter(|_| reports) {
            Some("working" | "blocked") => {
                self.running = true;
                false
            }
            Some("idle" | "done") => std::mem::take(&mut self.running),
            Some(_) | None => {
                self.running = false;
                false
            }
        }
    }
}

pub(crate) async fn follow(
    mut events: broadcast::Receiver<Result<EventFrame, Arc<Error>>>,
    ends: mpsc::Sender<()>,
) {
    let mut watch = TurnWatch::default();
    loop {
        let frame = tokio::select! {
            () = ends.closed() => return,
            frame = events.recv() => frame,
        };
        match frame {
            Ok(Ok(frame)) => {
                if watch.ended(&frame) && ends.send(()).await.is_err() {
                    return;
                }
            }
            Ok(Err(error)) => {
                tracing::debug!(%error, "herdr stopped reporting the pane's agent status");
                return;
            }
            Err(broadcast::error::RecvError::Lagged(missed)) => {
                tracing::debug!(missed, "herdr agent status events lagged");
            }
            Err(broadcast::error::RecvError::Closed) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TurnWatch;
    use crate::herdr::codec::InboundFrame;
    use crate::herdr::codec::decode_line;

    fn status(agent: &str, status: &str) -> super::EventFrame {
        let line = format!(
            "{{\"event\":\"pane.agent_status_changed\",\"data\":{{\"agent\":\"{agent}\",\"agent_status\":\"{status}\",\"pane_id\":\"w1:p1\",\"workspace_id\":\"w1\"}}}}"
        );
        match decode_line(line.as_bytes()).expect("event") {
            InboundFrame::Event(frame) => frame,
            InboundFrame::Ack(_) => panic!("a status change must decode as an event"),
        }
    }

    #[test]
    fn a_turn_ends_when_a_proven_agent_settles_after_working() {
        let mut watch = TurnWatch::default();
        let seen: Vec<bool> = [
            status("codex", "idle"),
            status("codex", "working"),
            status("codex", "blocked"),
            status("codex", "working"),
            status("codex", "idle"),
            status("codex", "idle"),
            status("opencode", "working"),
            status("opencode", "done"),
            status("claude", "working"),
            status("claude", "idle"),
            status("omp", "working"),
            status("omp", "idle"),
        ]
        .iter()
        .map(|frame| watch.ended(frame))
        .collect();
        assert_eq!(
            seen,
            [
                false, false, false, false, true, false, false, true, false, false, false, false
            ]
        );
    }
}
