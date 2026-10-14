use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Skipped {
    name: String,
    reason: String,
}

#[test]
fn out_of_slice_scenarios_are_named_and_explained() -> Result<(), Box<dyn std::error::Error>> {
    let entries: Vec<Skipped> = serde_json::from_str(include_str!("fixtures/skipped.json"))?;
    let expected = [
        "gateway_ca_v1_sign_chain_crl_routes",
        "gateway_verified_principal_and_legacy_device_mapping",
        "rh_ca_commands_and_offline_root_ceremony",
        "rh_cert_renew_holder_loop",
    ];
    assert_eq!(entries.len(), expected.len());
    for (entry, name) in entries.iter().zip(expected) {
        assert_eq!(entry.name, name);
        assert!(entry.reason.len() >= 60);
    }
    Ok(())
}
