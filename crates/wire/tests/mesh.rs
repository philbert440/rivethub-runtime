use wire::mesh::MeshFile;

#[test]
fn record_format_preserves_operator_and_unknown_fields_as_parsed_values() {
    let text = r#"{
        "version":1,"updatedAt":1760000000000,"operator":"fleet",
        "nodes":{"peer":{
            "id":"peer","name":"Peer","role":"agent","agents":["assistant"],
            "host":"peer.example.test","port":5700,"providers":["inference"],
            "models":["model"],"capabilities":["chat"],"status":"online",
            "lastSeen":1760000000000,"registeredAt":1750000000000,"version":"0.1.0",
            "sshUser":"operator","installRoot":"/srv/RivetHub","platform":"linux",
            "metadata":{"rack":"west"},"future":{"enabled":true,"unset":null}
        }}
    }"#;
    let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
    let typed: MeshFile = serde_json::from_str(text).unwrap();
    assert_eq!(serde_json::to_value(&typed).unwrap(), parsed);
    assert_eq!(typed.updated_at.timestamp_millis(), 1_760_000_000_000);
}

fn file_with_metadata(metadata: &str) -> String {
    format!(
        r#"{{"version":1,"updatedAt":0,"nodes":{{"peer":{{"id":"peer","name":"Peer","agents":[],"host":"peer.example","port":5700,"providers":[],"models":[],"capabilities":[],"status":"offline","lastSeen":0,"registeredAt":0,"version":"0.1.0","metadata":{metadata}}}}}}}"#
    )
}

#[test]
fn gateway_metadata_keeps_a_typed_port_and_rewrites_a_cutover_string() {
    let text = file_with_metadata(
        r#"{"denUrl":"http://peer.example:5174","denPort":"6002","rack":"west"}"#,
    );
    let typed: MeshFile = serde_json::from_str(&text).unwrap();
    let metadata = typed
        .nodes
        .get("peer")
        .and_then(|node| node.metadata.as_ref())
        .unwrap();
    assert_eq!(metadata.gateway_port, Some(6002));
    let endpoint = metadata.gateway_url.as_ref().expect("gateway url");
    assert_eq!(endpoint.port(), Some(5174));
    assert_eq!(
        metadata
            .extra
            .get("rack")
            .and_then(serde_json::Value::as_str),
        Some("west")
    );
    let written = serde_json::to_value(&typed).unwrap();
    let saved = written
        .get("nodes")
        .and_then(|nodes| nodes.get("peer"))
        .and_then(|node| node.get("metadata"))
        .expect("metadata");
    assert_eq!(
        saved.get("denPort").and_then(serde_json::Value::as_u64),
        Some(6002)
    );
    assert_eq!(
        saved.get("rack").and_then(serde_json::Value::as_str),
        Some("west")
    );
    assert_eq!(
        saved.get("denUrl").and_then(serde_json::Value::as_str),
        Some("http://peer.example:5174/")
    );
    assert_eq!(wire::mesh::parse_cutover_gateway_port("6002"), Ok(6002));
    assert_eq!(
        wire::mesh::parse_cutover_gateway_port("nope"),
        Err(wire::mesh::CutoverPortError::NotDecimal)
    );
}

#[test]
fn a_wrong_gateway_port_or_url_type_is_rejected() {
    for metadata in [
        r#"{"denPort":true}"#,
        r#"{"denPort":1.5}"#,
        r#"{"denPort":70000}"#,
        r#"{"denPort":-1}"#,
        r#"{"denPort":"nope"}"#,
        r#"{"denPort":" 6002"}"#,
        r#"{"denPort":"06002"}"#,
        r#"{"denPort":"6002 "}"#,
        r#"{"denUrl":4}"#,
        r#"{"denUrl":"not a url"}"#,
    ] {
        let text = file_with_metadata(metadata);
        assert!(
            serde_json::from_str::<MeshFile>(&text).is_err(),
            "{metadata}"
        );
    }
}

#[test]
fn numeric_spellings_and_wrong_types_are_not_coerced() {
    for text in [
        r#"{"version":"1","nodes":{},"updatedAt":0}"#,
        r#"{"version":1,"nodes":{},"updatedAt":"2026-10-14"}"#,
        r#"{"version":1,"nodes":{},"updatedAt":1.7600000045e12}"#,
        r#"{"version":1,"nodes":[],"updatedAt":0}"#,
        r#"{"version":1.1,"nodes":{},"updatedAt":0}"#,
    ] {
        assert!(serde_json::from_str::<MeshFile>(text).is_err(), "{text}");
    }
}
