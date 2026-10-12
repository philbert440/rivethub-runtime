use std::path::Path;

use serde::Deserialize;

use crate::Error;
use crate::Mismatch;
use crate::json::Json;
use crate::json::Masks;
use crate::json::json_masked_eq;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Canned {
    pub status: u16,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedRequest {
    pub method: String,
    pub target: String,
    pub body: String,
}

#[derive(Debug, Clone)]
pub struct ProviderTurn {
    pub requests: Vec<CapturedRequest>,
    pub chunks: Vec<Json>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct RequestRecord {
    #[serde(default)]
    pub index: u32,
    #[serde(default)]
    pub method: String,
    #[serde(default)]
    pub target: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub served: ServedRecord,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ServedRecord {
    #[serde(default = "default_served_status")]
    pub status: u16,
    #[serde(rename = "bodyFile", default)]
    pub body_file: String,
}

fn default_served_status() -> u16 {
    200
}

#[derive(Debug, Clone)]
pub struct ProviderScenario {
    pub name: String,
    pub canned: Vec<Canned>,
    pub requests: Vec<RequestRecord>,
    pub chunks: Option<Vec<Json>>,
}

pub fn load_scenario(scenario: &Path) -> Result<ProviderScenario, Error> {
    let name = scenario
        .file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().to_string());
    let requests_path = scenario.join("requests.json");
    let text = crate::read_text(&requests_path)?;
    let requests: Vec<RequestRecord> = crate::parse_json(&requests_path, &text)?;
    let mut canned = Vec::new();
    for request in &requests {
        let body_path = scenario.join(&request.served.body_file);
        let body = crate::read_text(&body_path)?;
        canned.push(Canned {
            status: request.served.status,
            body,
        });
    }
    let chunks_path = scenario.join("chunks.json");
    let chunks = match std::fs::read_to_string(&chunks_path) {
        Ok(text) => Some(crate::parse_json(&chunks_path, &text)?),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => None,
        Err(source) => return Err(Error::io(&chunks_path, source)),
    };
    Ok(ProviderScenario {
        name,
        canned,
        requests,
        chunks,
    })
}

pub fn replay(
    scenario: &ProviderScenario,
    run: impl FnOnce(Vec<Canned>) -> ProviderTurn,
) -> Result<(), Mismatch> {
    let turn = run(scenario.canned.clone());
    if turn.requests.len() != scenario.requests.len() {
        return Err(Mismatch::new(
            &scenario.name,
            format!(
                "{} request(s), recorded {}",
                turn.requests.len(),
                scenario.requests.len()
            ),
        ));
    }
    let masks = Masks::default();
    for (want, got) in scenario.requests.iter().zip(&turn.requests) {
        let id = format!("{}/request-{}", scenario.name, want.index);
        compare_request(&id, want, got, &masks)?;
    }
    compare_chunks(scenario, &turn.chunks, &masks)
}

fn compare_request(
    id: &str,
    want: &RequestRecord,
    got: &CapturedRequest,
    masks: &Masks,
) -> Result<(), Mismatch> {
    if want.method != got.method {
        return Err(Mismatch::new(
            id,
            format!("method {} != recorded {}", got.method, want.method),
        ));
    }
    if want.target != got.target {
        return Err(Mismatch::new(
            id,
            format!("target {} != recorded {}", got.target, want.target),
        ));
    }
    if want.body.is_empty() {
        if !got.body.is_empty() {
            return Err(Mismatch::new(
                id,
                "request body mismatch\n    recorded: <none>\n    actual:   non-empty",
            ));
        }
        return Ok(());
    }
    let Ok(want_body) = serde_json::from_str(&want.body) else {
        return Err(Mismatch::new(id, "recorded request body unreadable"));
    };
    let Ok(got_body) = serde_json::from_str(&got.body) else {
        return Err(Mismatch::new(id, "actual request body is not JSON"));
    };
    if !json_masked_eq(&want_body, &got_body, masks) {
        return Err(Mismatch::new(
            id,
            format!("request body mismatch\n    recorded: {want_body}\n    actual:   {got_body}"),
        ));
    }
    Ok(())
}

fn compare_chunks(
    scenario: &ProviderScenario,
    actual: &[Json],
    masks: &Masks,
) -> Result<(), Mismatch> {
    let Some(recorded) = &scenario.chunks else {
        return Ok(());
    };
    if recorded.len() != actual.len() {
        return Err(Mismatch::new(
            &scenario.name,
            format!("{} chunk(s), recorded {}", actual.len(), recorded.len()),
        ));
    }
    for (index, (want, got)) in recorded.iter().zip(actual).enumerate() {
        if !json_masked_eq(want, got, masks) {
            return Err(Mismatch::new(
                format!("{}/chunk-{index}", scenario.name),
                format!("chunk mismatch\n    recorded: {want}\n    actual:   {got}"),
            ));
        }
    }
    Ok(())
}
