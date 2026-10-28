use testkit::http::{HttpCase, Response};
use testkit::json::{Masks, substitute};

use super::Error;
use super::recording::Case;

pub(super) fn compare(id: &str, case: &Case, actual: Response, masks: &Masks) -> Result<(), Error> {
    let expected = case
        .response
        .body
        .as_ref()
        .ok_or_else(|| Error::Fixture(format!("missing response body: {id}")))?;
    let json = case.response.headers.iter().any(|(name, value)| {
        name.eq_ignore_ascii_case("content-type") && value == "application/json"
    });
    if json && !expected.is_empty() {
        let expected = substitute(expected, masks)
            .replace("{{VERSION}}", gateway::VERSION)
            .replace("{{REVISION}}", gateway::REVISION.unwrap_or_default());
        let expected: wire::JsonObject = serde_json::from_str(&expected)?;
        let body: wire::JsonObject = serde_json::from_slice(&actual.body)?;
        if expected != body {
            return Err(Error::Mismatch(format!(
                "{id}: body {body:?}, expected {expected:?}"
            )));
        }
    }
    let recorded = HttpCase {
        id: id.to_owned(),
        request: case.request.http.clone(),
        response: case.response.clone(),
    };
    testkit::http::replay(&recorded, masks, |_| actual)
        .map_err(|error| Error::Mismatch(format!("{error:?}")))
}

pub(super) async fn send(
    fixture: &super::fixture::Fixture,
    port: u16,
    case: &Case,
    masks: &Masks,
) -> Result<Response, Error> {
    let address = fixture.address(case.request.via, port);
    let client = fixture.client(case.request.certificate, address.ip())?;
    let head = testkit::http::parse_head(&substitute(&case.request.http.head, masks))
        .ok_or_else(|| Error::Fixture(format!("unreadable request: {}", case.id)))?;
    let method = reqwest::Method::from_bytes(head.method.as_bytes())?;
    let mut request = client.request(method, format!("https://{address}{}", head.target));
    for (name, value) in head.headers {
        request = request.header(name, value);
    }
    if let Some(body) = &case.request.http.body {
        request = request.body(substitute(body, masks));
    }
    let response = request.send().await?;
    let peer = response
        .remote_addr()
        .ok_or_else(|| Error::Fixture("missing peer address".into()))?;
    assert_eq!(peer, address);
    let status = response.status().as_u16();
    let headers = response
        .headers()
        .iter()
        .map(|(name, value)| Ok((name.to_string(), value.to_str()?.to_owned())))
        .collect::<Result<Vec<_>, axum::http::header::ToStrError>>()?;
    let body = response.bytes().await?.to_vec();
    Ok(Response {
        status,
        headers,
        body,
    })
}
