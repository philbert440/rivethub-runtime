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
        encode_request(&self.id, self.method, &self.params)
    }
}

pub fn encode_request(
    id: &str,
    method: RpcMethod,
    params: &RequestParams,
) -> Result<Vec<u8>, Error> {
    #[derive(Serialize)]
    struct Wire<'a> {
        id: &'a str,
        method: RpcMethod,
        params: &'a RequestParams,
    }
    let mut line =
        serde_json::to_vec(&Wire { id, method, params }).map_err(|error| Error::Rpc {
            method: std::borrow::Cow::Borrowed("encode"),
            message: error.to_string(),
        })?;
    line.push(b'\n');
    Ok(line)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AckResult {
    SubscriptionStarted,
    #[serde(other)]
    Unrecorded,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AckFrame {
    pub id: String,
    #[serde(default)]
    pub result: Option<AckResult>,
    #[serde(default)]
    pub error: Option<wire::Extra>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct EventFrame {
    pub event: HerdrEventKind,
    pub data: wire::Extra,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum InboundFrame {
    Ack(AckFrame),
    Event(EventFrame),
}

pub fn decode_line(line: &[u8]) -> Result<InboundFrame, Error> {
    serde_json::from_slice(line).map_err(|error| Error::Rpc {
        method: std::borrow::Cow::Borrowed("decode"),
        message: error.to_string(),
    })
}

pub fn decode_request(line: &[u8]) -> Result<RequestFrame, Error> {
    serde_json::from_slice(line).map_err(|error| Error::Rpc {
        method: std::borrow::Cow::Borrowed("decode"),
        message: error.to_string(),
    })
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
    use super::AckResult;
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
            String::from_utf8(line).expect("utf8"),
            "{\"id\":\"1\",\"method\":\"workspace.create\",\"params\":{\"cwd\":\"/home/user/AcmeKit\",\"env\":{\"ACME_ROSTER_ENV\":\"roster\"},\"label\":\"acme\"}}\n"
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
            String::from_utf8(line).expect("utf8"),
            "{\"id\":\"2\",\"method\":\"agent.start\",\"params\":{\"name\":\"d0123456789ab\",\"kind\":\"claude\",\"pane_id\":\"w1:p1\",\"args\":[\"--resume\",\"00000000-0000-4000-8000-000000000002\"],\"timeout_ms\":10000}}\n"
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
            String::from_utf8(get.encode().expect("encode")).expect("utf8"),
            "{\"id\":\"3\",\"method\":\"pane.get\",\"params\":{\"pane_id\":\"w1:p1\"}}\n"
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
            String::from_utf8(send.encode().expect("encode")).expect("utf8"),
            "{\"id\":\"4\",\"method\":\"pane.send_text\",\"params\":{\"pane_id\":\"w1:p1\",\"text\":\"bash -l\\n\"}}\n"
        );
    }

    #[test]
    fn subscription_started_ack_decodes() {
        let frame = decode_line(b"{\"id\":\"7\",\"result\":{\"type\":\"subscription_started\"}}")
            .expect("ack");
        match frame {
            InboundFrame::Ack(ack) => {
                assert_eq!(ack.id, "7");
                assert_eq!(ack.result, Some(AckResult::SubscriptionStarted));
                assert!(ack.error.is_none());
            }
            InboundFrame::Event(_) => panic!("an ack must not decode as an event"),
        }
    }

    #[test]
    fn unrecorded_result_types_decode_as_unrecorded() {
        let frame = decode_line(
            b"{\"id\":\"8\",\"result\":{\"type\":\"workspace_created\",\"pane_id\":\"w1:p1\"}}",
        )
        .expect("ack");
        match frame {
            InboundFrame::Ack(ack) => assert_eq!(ack.result, Some(AckResult::Unrecorded)),
            InboundFrame::Event(_) => panic!("an ack must not decode as an event"),
        }
    }

    #[test]
    fn error_acks_carry_the_recorded_error_object() {
        let frame =
            decode_line(b"{\"id\":\"9\",\"error\":{\"code\":\"agent_pane_busy\"}}").expect("ack");
        match frame {
            InboundFrame::Ack(ack) => {
                let error = ack.error.expect("error object");
                assert!(error.get("code").is_some());
            }
            InboundFrame::Event(_) => panic!("an ack must not decode as an event"),
        }
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
