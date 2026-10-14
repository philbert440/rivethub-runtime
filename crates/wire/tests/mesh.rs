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
