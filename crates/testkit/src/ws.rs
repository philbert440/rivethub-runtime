use std::collections::BTreeMap;
use std::path::Path;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::Deserialize;

use crate::Error;
use crate::Mismatch;
use crate::http::ClientCloseRecord;
use crate::http::Handshake;
use crate::http::HandshakeRecord;
use crate::http::HeaderPair;
use crate::http::Request;
use crate::http::RequestRecord;
use crate::http::Response;
use crate::http::ResponseRecord;
use crate::http::parse_head;
use crate::http::status_of;
use crate::json::Masks;
use crate::json::json_masked_eq;
use crate::json::masked_eq;
use crate::json::substitute;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    Text(String),
    Binary { data: Vec<u8>, replay: bool },
    Close { code: Option<u16>, reason: String },
    TcpClosed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WsRequest {
    pub client: String,
    pub request: Request,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WsSent {
    pub client: String,
    pub frame: Frame,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WsCall {
    Upgrade(WsRequest),
    Http(Request),
    Sent(WsSent),
    ClientClose { client: String, code: Option<u16> },
    Take { client: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WsReply {
    Handshake(Handshake),
    Http(Response),
    Frames(Vec<Frame>),
    Done,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Recording {
    #[serde(default)]
    pub runs: BTreeMap<String, RunRecord>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct RunRecord {
    #[serde(default)]
    pub steps: Vec<StepRecord>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct StepRecord {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub connect: Option<ConnectRecord>,
    #[serde(default)]
    pub http: Option<HttpRecord>,
    #[serde(default)]
    pub sent: Vec<SentRecord>,
    #[serde(rename = "sentClose", default)]
    pub sent_close: Option<ClientCloseRecord>,
    #[serde(default)]
    pub frames: BTreeMap<String, Vec<FrameRecord>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ConnectRecord {
    #[serde(default)]
    pub client: String,
    #[serde(default)]
    pub request: RequestRecord,
    #[serde(default)]
    pub handshake: HandshakeRecord,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct HttpRecord {
    #[serde(default)]
    pub request: RequestRecord,
    #[serde(default)]
    pub response: ResponseRecord,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum FrameRecord {
    Text {
        text: String,
    },
    Binary {
        #[serde(default)]
        text: Option<String>,
        #[serde(default)]
        base64: Option<String>,
        #[serde(default)]
        replay: bool,
    },
    Close {
        #[serde(default)]
        code: Option<u16>,
        #[serde(default)]
        reason: String,
    },
    TcpClosed,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SentPayload {
    Text { text: String },
    Binary { hex: String },
}

#[derive(Debug, Clone, Deserialize)]
pub struct SentRecord {
    #[serde(default)]
    pub client: String,
    #[serde(flatten)]
    pub payload: SentPayload,
}

#[derive(Debug, Clone)]
pub struct WsStep {
    pub id: String,
    pub record: StepRecord,
}

pub fn steps(recording: &Path) -> Result<Vec<WsStep>, Error> {
    let text = crate::read_text(recording)?;
    let doc: Recording = crate::parse_json(recording, &text)?;
    let mut steps = Vec::new();
    for (run, entry) in doc.runs {
        for step in entry.steps {
            let id = step.id.clone();
            steps.push(WsStep {
                id: format!("{run}/{id}"),
                record: step,
            });
        }
    }
    Ok(steps)
}

pub fn replay(
    step: &WsStep,
    masks: &Masks,
    mut harness: impl FnMut(WsCall) -> WsReply,
) -> Result<(), Mismatch> {
    if let Some(connect) = &step.record.connect {
        step_connect(step, connect, masks, &mut harness)?;
    }
    if let Some(http) = &step.record.http {
        step_http(step, http, masks, &mut harness)?;
    }
    for sent in &step.record.sent {
        step_sent(step, sent, &mut harness)?;
    }
    if let Some(close) = &step.record.sent_close {
        harness(WsCall::ClientClose {
            client: close.client.clone(),
            code: close.code,
        });
    }
    for (client, recorded) in &step.record.frames {
        compare_frames(step, client, recorded, masks, &mut harness)?;
    }
    Ok(())
}

fn build_request(recorded: &RequestRecord, masks: &Masks) -> Option<Request> {
    let head = parse_head(&substitute(&recorded.head, masks))?;
    let body = recorded
        .body
        .as_deref()
        .map_or_else(Vec::new, |text| substitute(text, masks).into_bytes());
    Some(Request {
        method: head.method,
        target: head.target,
        headers: head.headers,
        body,
        recipe: None,
        via: recorded.via.clone(),
    })
}

fn step_connect(
    step: &WsStep,
    connect: &ConnectRecord,
    masks: &Masks,
    harness: &mut dyn FnMut(WsCall) -> WsReply,
) -> Result<(), Mismatch> {
    let Some(request) = build_request(&connect.request, masks) else {
        return Err(Mismatch::new(&step.id, "connect request unreadable"));
    };
    match harness(WsCall::Upgrade(WsRequest {
        client: connect.client.clone(),
        request,
    })) {
        WsReply::Handshake(actual) => compare_handshake(step, &connect.handshake, &actual, masks),
        WsReply::Http(_) | WsReply::Frames(_) | WsReply::Done => Err(Mismatch::new(
            &step.id,
            "harness returned a non-handshake answer",
        )),
    }
}

fn compare_handshake(
    step: &WsStep,
    recorded: &HandshakeRecord,
    actual: &Handshake,
    masks: &Masks,
) -> Result<(), Mismatch> {
    let id = &step.id;
    if recorded.no_response {
        if actual.status.is_some() {
            return Err(Mismatch::new(
                id,
                "recording destroyed the socket; actual wrote an answer",
            ));
        }
        return Ok(());
    }
    let Some(actual_status) = actual.status else {
        return Err(Mismatch::new(
            id,
            "recording has an answer; actual wrote nothing",
        ));
    };
    let want = status_of(&recorded.status_line);
    if want != actual_status {
        return Err(Mismatch::new(
            id,
            format!("handshake status {actual_status} != recorded {want}"),
        ));
    }
    compare_handshake_headers(id, &recorded.headers, &actual.headers, masks)?;
    if let Some(text) = &recorded.body
        && text.as_bytes() != actual.body.as_slice()
    {
        return Err(Mismatch::new(id, "handshake body mismatch"));
    }
    Ok(())
}

fn compare_handshake_headers(
    id: &str,
    recorded: &[HeaderPair],
    actual: &[(String, String)],
    masks: &Masks,
) -> Result<(), Mismatch> {
    let mut used = vec![false; actual.len()];
    for (name, value) in recorded {
        let matched = actual
            .iter()
            .zip(&mut used)
            .find(|((actual_name, _), taken)| !**taken && actual_name.eq_ignore_ascii_case(name))
            .map(|((_, actual_value), taken)| {
                *taken = true;
                masked_eq(value, actual_value, masks)
            });
        if matched != Some(true) {
            return Err(Mismatch::new(
                id,
                format!("handshake header {name} mismatch"),
            ));
        }
    }
    Ok(())
}

fn step_http(
    step: &WsStep,
    http: &HttpRecord,
    masks: &Masks,
    harness: &mut dyn FnMut(WsCall) -> WsReply,
) -> Result<(), Mismatch> {
    let Some(request) = build_request(&http.request, masks) else {
        return Err(Mismatch::new(&step.id, "http request unreadable"));
    };
    match harness(WsCall::Http(request)) {
        WsReply::Http(actual) => compare_http_response(step, &http.response, &actual, masks),
        WsReply::Handshake(_) | WsReply::Frames(_) | WsReply::Done => Err(Mismatch::new(
            &step.id,
            "harness returned a non-http answer",
        )),
    }
}

fn compare_http_response(
    step: &WsStep,
    recorded: &ResponseRecord,
    actual: &Response,
    masks: &Masks,
) -> Result<(), Mismatch> {
    let want_status = status_of(&recorded.status_line);
    if want_status != actual.status {
        return Err(Mismatch::new(
            &step.id,
            format!("http status {} != recorded {want_status}", actual.status),
        ));
    }
    let Some(want_body) = response_body(recorded) else {
        return Err(Mismatch::new(&step.id, "recorded body unreadable"));
    };
    let want_text = String::from_utf8_lossy(&want_body);
    let act_text = String::from_utf8_lossy(&actual.body);
    let both_json = serde_json::from_str(&want_text)
        .ok()
        .zip(serde_json::from_str(&act_text).ok());
    let equal = match both_json {
        Some((want, got)) => json_masked_eq(&want, &got, masks),
        None => masked_eq(&want_text, &act_text, masks),
    };
    if equal {
        Ok(())
    } else {
        Err(Mismatch::new(
            &step.id,
            format!("body mismatch\n    recorded: {want_text}\n    actual:   {act_text}"),
        ))
    }
}

fn response_body(recorded: &ResponseRecord) -> Option<Vec<u8>> {
    if let Some(text) = recorded.body.as_deref() {
        return Some(text.as_bytes().to_vec());
    }
    recorded
        .body_base64
        .as_deref()
        .and_then(|encoded| BASE64.decode(encoded).ok())
}

fn step_sent(
    step: &WsStep,
    sent: &SentRecord,
    harness: &mut dyn FnMut(WsCall) -> WsReply,
) -> Result<(), Mismatch> {
    let frame = match &sent.payload {
        SentPayload::Text { text } => Frame::Text(text.clone()),
        SentPayload::Binary { hex } => match hex::decode(hex) {
            Ok(data) => Frame::Binary {
                data,
                replay: false,
            },
            Err(_) => {
                return Err(Mismatch::new(&step.id, "sent frame unreadable"));
            }
        },
    };
    harness(WsCall::Sent(WsSent {
        client: sent.client.clone(),
        frame,
    }));
    Ok(())
}

fn merge_output_frames(actual: &[Frame]) -> Vec<Frame> {
    let mut merged: Vec<Frame> = Vec::new();
    for frame in actual {
        if let Some(Frame::Binary {
            data: last,
            replay: last_replay,
        }) = merged.last_mut()
            && let Frame::Binary { data, replay } = frame
            && *last_replay == *replay
        {
            last.extend_from_slice(data);
            continue;
        }
        merged.push(frame.clone());
    }
    merged
}

fn frame_kind(frame: &Frame) -> &'static str {
    match frame {
        Frame::Text(_) => "text",
        Frame::Binary { .. } => "binary",
        Frame::Close { .. } => "close",
        Frame::TcpClosed => "tcp-closed",
    }
}

fn compare_frames(
    step: &WsStep,
    client: &str,
    recorded: &[FrameRecord],
    masks: &Masks,
    harness: &mut dyn FnMut(WsCall) -> WsReply,
) -> Result<(), Mismatch> {
    let actual = match harness(WsCall::Take {
        client: client.to_string(),
    }) {
        WsReply::Frames(actual) => actual,
        WsReply::Handshake(_) | WsReply::Http(_) | WsReply::Done => {
            return Err(Mismatch::new(
                &step.id,
                format!("client {client}: harness returned no frames"),
            ));
        }
    };
    let merged = merge_output_frames(&actual);
    if merged.len() != recorded.len() {
        return Err(Mismatch::new(
            &step.id,
            format!(
                "client {client}: {} frame(s), recorded {}",
                merged.len(),
                recorded.len()
            ),
        ));
    }
    for (index, (want, got)) in recorded.iter().zip(&merged).enumerate() {
        let Some(want_frame) = recorded_frame(want) else {
            return Err(Mismatch::new(
                &step.id,
                format!("client {client} frame {index}: unreadable recording"),
            ));
        };
        compare_frame_pair(
            &step.id,
            &format!("client {client} frame {index}"),
            &want_frame,
            got,
            masks,
        )?;
    }
    Ok(())
}

fn recorded_frame(recorded: &FrameRecord) -> Option<Frame> {
    match recorded {
        FrameRecord::Text { text } => Some(Frame::Text(text.clone())),
        FrameRecord::Binary {
            text,
            base64,
            replay,
        } => {
            let data = if let Some(text) = text {
                Some(text.as_bytes().to_vec())
            } else {
                base64
                    .as_deref()
                    .and_then(|encoded| BASE64.decode(encoded).ok())
            };
            data.map(|data| Frame::Binary {
                data,
                replay: *replay,
            })
        }
        FrameRecord::Close { code, reason } => Some(Frame::Close {
            code: *code,
            reason: reason.clone(),
        }),
        FrameRecord::TcpClosed => Some(Frame::TcpClosed),
    }
}

fn compare_frame_pair(
    id: &str,
    detail: &str,
    want: &Frame,
    got: &Frame,
    masks: &Masks,
) -> Result<(), Mismatch> {
    match (want, got) {
        (Frame::Text(want_text), Frame::Text(actual_text)) => {
            let both_json = serde_json::from_str(want_text)
                .ok()
                .zip(serde_json::from_str(actual_text).ok());
            let equal = match both_json {
                Some((w, a)) => json_masked_eq(&w, &a, masks),
                None => masked_eq(want_text, actual_text, masks),
            };
            if !equal {
                return Err(Mismatch::new(id, format!("{detail}: payload mismatch")));
            }
        }
        (
            Frame::Binary {
                data: want_data,
                replay: want_replay,
            },
            Frame::Binary {
                data: actual_data,
                replay: actual_replay,
            },
        ) => {
            let equal = match (
                std::str::from_utf8(want_data),
                std::str::from_utf8(actual_data),
            ) {
                (Ok(want_text), Ok(actual_text)) => masked_eq(want_text, actual_text, masks),
                _ => want_data == actual_data,
            };
            if !equal || want_replay != actual_replay {
                return Err(Mismatch::new(
                    id,
                    format!("{detail}: binary payload mismatch"),
                ));
            }
        }
        (
            Frame::Close {
                code: want_code,
                reason: want_reason,
            },
            Frame::Close {
                code: actual_code,
                reason: actual_reason,
            },
        ) => {
            if want_code != actual_code || !masked_eq(want_reason, actual_reason, masks) {
                return Err(Mismatch::new(id, format!("{detail}: close mismatch")));
            }
        }
        (Frame::TcpClosed, Frame::TcpClosed) => {}
        _ => {
            return Err(Mismatch::new(
                id,
                format!(
                    "{detail}: frame kind {} != {}",
                    frame_kind(want),
                    frame_kind(got)
                ),
            ));
        }
    }
    Ok(())
}
