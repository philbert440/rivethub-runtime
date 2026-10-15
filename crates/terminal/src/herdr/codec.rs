use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;
use strum::Display;

use crate::error::Error;
use crate::herdr::AgentKind;
use crate::herdr::HerdrEventKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display)]
pub enum RpcMethod {
    #[serde(rename = "workspace.create")]
    #[strum(serialize = "workspace.create")]
    WorkspaceCreate,
    #[serde(rename = "pane.get")]
    #[strum(serialize = "pane.get")]
    PaneGet,
    #[serde(rename = "agent.start")]
    #[strum(serialize = "agent.start")]
    AgentStart,
    #[serde(rename = "pane.send_text")]
    #[strum(serialize = "pane.send_text")]
    PaneSendText,
    #[serde(rename = "events.subscribe")]
    #[strum(serialize = "events.subscribe")]
    EventsSubscribe,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceCreateParams {
    pub cwd: PathBuf,
    pub env: BTreeMap<String, String>,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PaneGetParams {
    pub pane_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentStartParams {
    pub name: String,
    pub kind: AgentKind,
    pub pane_id: String,
    pub args: Vec<String>,
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PaneSendTextParams {
    pub pane_id: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscriptionParams {
    #[serde(rename = "type")]
    pub event: HerdrEventKind,
    pub pane_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventsSubscribeParams {
    pub subscriptions: Vec<SubscriptionParams>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestParams {
    WorkspaceCreate(WorkspaceCreateParams),
    PaneGet(PaneGetParams),
    AgentStart(AgentStartParams),
    PaneSendText(PaneSendTextParams),
    EventsSubscribe(EventsSubscribeParams),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestFrame {
    pub id: String,
    pub method: RpcMethod,
    pub params: RequestParams,
}

impl RequestFrame {
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        let mut line = serde_json::to_vec(self)?;
        line.push(b'\n');
        Ok(line)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AckResult {
    #[serde(rename = "type")]
    pub kind: AckResultKind,
    #[serde(flatten)]
    pub fields: wire::JsonObject,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AckResultKind {
    SubscriptionStarted,
    WorkspaceCreated,
    Ok,
    #[serde(untagged)]
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Display)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum AckErrorCode {
    AgentPaneBusy,
    #[serde(untagged)]
    #[strum(to_string = "{0}")]
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, thiserror::Error)]
#[error("code {code:?}, message {message:?}")]
pub struct AckError {
    pub code: Option<AckErrorCode>,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AckFrame {
    pub id: String,
    #[serde(default)]
    pub result: Option<AckResult>,
    #[serde(default)]
    pub error: Option<AckError>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct EventFrame {
    pub event: HerdrEventKind,
    pub data: wire::JsonObject,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum InboundFrame {
    Ack(AckFrame),
    Event(EventFrame),
}

pub fn decode_line(line: &[u8]) -> Result<InboundFrame, Error> {
    Ok(serde_json::from_slice(line)?)
}

#[cfg(test)]
fn decode_request(line: &[u8]) -> Result<RequestFrame, Error> {
    Ok(serde_json::from_slice(line)?)
}

pub fn shell_join(argv: &[String]) -> String {
    argv.iter()
        .map(|word| {
            if !word.is_empty()
                && word.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric()
                        || matches!(
                            byte,
                            b'_' | b'-' | b'.' | b'/' | b':' | b'=' | b',' | b'+' | b'%' | b'@'
                        )
                })
            {
                word.clone()
            } else {
                let quoted: String = "'".to_owned() + &word.replace('\'', "'\\''") + "'";
                quoted
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::AckErrorCode;
    use super::AckResultKind;
    use super::AgentStartParams;
    use super::EventFrame;
    use super::InboundFrame;
    use super::PaneGetParams;
    use super::PaneSendTextParams;
    use super::RequestFrame;
    use super::RequestParams;
    use super::RpcMethod;
    use super::WorkspaceCreateParams;
    use super::decode_line;
    use super::decode_request;
    use super::shell_join;
    use crate::herdr::AgentKind;
    use crate::herdr::DEFAULT_PANE;
    use crate::herdr::HerdrEventKind;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    #[test]
    fn workspace_create_encodes_the_recorded_shape() {
        let frame = RequestFrame {
            id: "1".to_owned(),
            method: RpcMethod::WorkspaceCreate,
            params: RequestParams::WorkspaceCreate(WorkspaceCreateParams {
                cwd: PathBuf::from("/home/user/AcmeKit"),
                env: BTreeMap::from([("ACME_ROSTER_ENV".to_owned(), "roster".to_owned())]),
                label: "acme".to_owned(),
            }),
        };
        let line = frame.encode().expect("encode");
        assert_eq!(
            decode_request(&line).expect("request"),
            decode_request("{\"id\":\"1\",\"method\":\"workspace.create\",\"params\":{\"cwd\":\"/home/user/AcmeKit\",\"env\":{\"ACME_ROSTER_ENV\":\"roster\"},\"label\":\"acme\"}}\n".as_bytes()).expect("recorded request")
        );
    }

    #[test]
    fn agent_start_encodes_the_recorded_shape() {
        let frame = RequestFrame {
            id: "2".to_owned(),
            method: RpcMethod::AgentStart,
            params: RequestParams::AgentStart(AgentStartParams {
                name: "d0123456789ab".to_owned(),
                kind: AgentKind::Claude,
                pane_id: DEFAULT_PANE.to_owned(),
                args: vec![
                    "--resume".to_owned(),
                    "00000000-0000-4000-8000-000000000002".to_owned(),
                ],
                timeout_ms: 10_000,
            }),
        };
        let line = frame.encode().expect("encode");
        assert_eq!(
            decode_request(&line).expect("request"),
            decode_request("{\"id\":\"2\",\"method\":\"agent.start\",\"params\":{\"name\":\"d0123456789ab\",\"kind\":\"claude\",\"pane_id\":\"w1:p1\",\"args\":[\"--resume\",\"00000000-0000-4000-8000-000000000002\"],\"timeout_ms\":10000}}\n".as_bytes()).expect("recorded request")
        );
    }

    #[test]
    fn pane_frames_encode_the_recorded_shapes() {
        let get = RequestFrame {
            id: "3".to_owned(),
            method: RpcMethod::PaneGet,
            params: RequestParams::PaneGet(PaneGetParams {
                pane_id: "w1:p1".to_owned(),
            }),
        };
        assert_eq!(
            decode_request(&get.encode().expect("encode")).expect("request"),
            decode_request(
                "{\"id\":\"3\",\"method\":\"pane.get\",\"params\":{\"pane_id\":\"w1:p1\"}}\n"
                    .as_bytes()
            )
            .expect("recorded request")
        );
        let send = RequestFrame {
            id: "4".to_owned(),
            method: RpcMethod::PaneSendText,
            params: RequestParams::PaneSendText(PaneSendTextParams {
                pane_id: "w1:p1".to_owned(),
                text: "bash -l\n".to_owned(),
            }),
        };
        assert_eq!(
            decode_request(&send.encode().expect("encode")).expect("request"),
            decode_request("{\"id\":\"4\",\"method\":\"pane.send_text\",\"params\":{\"pane_id\":\"w1:p1\",\"text\":\"bash -l\\n\"}}\n".as_bytes()).expect("recorded request")
        );
    }

    #[test]
    fn subscription_started_ack_decodes() {
        let frame = decode_line(b"{\"id\":\"7\",\"result\":{\"type\":\"subscription_started\"}}")
            .expect("ack");
        match frame {
            InboundFrame::Ack(ack) => {
                assert_eq!(ack.id, "7");
                assert_eq!(
                    ack.result.expect("result").kind,
                    AckResultKind::SubscriptionStarted
                );
                assert!(ack.error.is_none());
            }
            InboundFrame::Event(_) => panic!("an ack must not decode as an event"),
        }
    }

    #[test]
    fn workspace_created_result_decodes() {
        let frame = decode_line(
            b"{\"id\":\"8\",\"result\":{\"type\":\"workspace_created\",\"pane_id\":\"w1:p1\"}}",
        )
        .expect("ack");
        match frame {
            InboundFrame::Ack(ack) => assert_eq!(
                ack.result.expect("result").kind,
                AckResultKind::WorkspaceCreated
            ),
            InboundFrame::Event(_) => panic!("an ack must not decode as an event"),
        }
    }

    #[test]
    fn error_acks_carry_a_typed_code_and_optional_message() {
        let busy = decode_line(
            b"{\"id\":\"9\",\"error\":{\"code\":\"agent_pane_busy\",\"message\":\"pane w1:p1 is busy\"}}",
        )
        .expect("ack");
        let InboundFrame::Ack(ack) = busy else {
            panic!("an ack must not decode as an event")
        };
        let error = ack.error.expect("error object");
        assert_eq!(error.code, Some(AckErrorCode::AgentPaneBusy));
        assert_eq!(error.message.as_deref(), Some("pane w1:p1 is busy"));
        assert!(error.to_string().contains("pane w1:p1 is busy"));
        let other =
            decode_line(b"{\"id\":\"10\",\"error\":{\"code\":\"pane_not_found\"}}").expect("ack");
        let InboundFrame::Ack(ack) = other else {
            panic!("an ack must not decode as an event")
        };
        let error = ack.error.expect("error object");
        assert_eq!(
            error.code,
            Some(AckErrorCode::Other("pane_not_found".to_owned()))
        );
        assert_eq!(error.message, None);
        assert!(error.to_string().contains("pane_not_found"));
    }

    #[test]
    fn event_frames_decode_through_both_recorded_aliases() {
        let dotted = decode_line(
            b"{\"event\":\"pane.agent_status_changed\",\"data\":{\"pane_id\":\"w1:p1\"}}",
        )
        .expect("event");
        let flat = decode_line(
            b"{\"event\":\"pane_agent_status_changed\",\"data\":{\"pane_id\":\"w1:p1\"}}",
        )
        .expect("event");
        for frame in [dotted, flat] {
            match frame {
                InboundFrame::Event(EventFrame { event, data }) => {
                    assert_eq!(event, HerdrEventKind::PaneAgentStatusChanged);
                    assert_eq!(
                        data.get("pane_id").and_then(|found| found.as_str()),
                        Some("w1:p1")
                    );
                }
                InboundFrame::Ack(_) => panic!("an event must not decode as an ack"),
            }
        }
        let detected =
            decode_line(b"{\"event\":\"pane_agent_detected\",\"data\":{}}").expect("event");
        match detected {
            InboundFrame::Event(EventFrame { event, .. }) => {
                assert_eq!(event, HerdrEventKind::PaneAgentDetected);
            }
            InboundFrame::Ack(_) => panic!("an event must not decode as an ack"),
        }
    }

    #[test]
    fn unknown_event_names_do_not_decode() {
        let decoded = decode_line(b"{\"event\":\"pane.exited\",\"data\":{}}");
        assert!(decoded.is_err());
    }

    #[test]
    fn shell_join_quotes_only_unsafe_words() {
        assert_eq!(shell_join(&["bash".to_owned(), "-l".to_owned()]), "bash -l");
        assert_eq!(
            shell_join(&[
                "/home/user/AcmeKit/run.sh".to_owned(),
                "a b".to_owned(),
                "it's".to_owned()
            ]),
            "/home/user/AcmeKit/run.sh 'a b' 'it'\\''s'"
        );
    }
}
