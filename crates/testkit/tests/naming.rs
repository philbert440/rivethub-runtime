use testkit::translate_recorded_product_names;

#[test]
fn product_translation_covers_operator_text_environment_and_generated_identifiers() {
    for (recorded, expected) in [
        (
            "[den-server] refusing RIVETOS_DEN_TLS_CERT + RIVETOS_DEN_TLS_KEY; bind den.host",
            "[rivethub-gateway] refusing RIVETHUB_TLS_CERT + RIVETHUB_TLS_KEY; bind the gateway host",
        ),
        (
            "add it to den.allowed_hosts / add it to den.allowed_origins",
            "add it to the gateway allowed_hosts setting / add it to the gateway allowed_origins setting",
        ),
        (
            "RIVETOS_SESSION_KEY RIVET_DEN_SESSION RIVET_DEN_URL RIVET_DEN_TOKEN RIVET_DEN_CA RIVET_DEN_NAME",
            "RIVETHUB_SESSION_KEY RIVETHUB_SESSION RIVETHUB_URL RIVETHUB_TOKEN RIVETHUB_CA RIVETHUB_NAME",
        ),
        (
            "herdr (den pins 0.9 / protocol 20)",
            "herdr (RivetHub pins 0.9 / protocol 20)",
        ),
        (
            "/run/user/1000/rivet-den-0123abcd/herdr",
            "/run/user/1000/rivethub-0123abcd/herdr",
        ),
        (
            "den-00000000-0000-4000-8000-000000000001",
            "rivethub-00000000-0000-4000-8000-000000000001",
        ),
        (
            "golden-node golden.host golden.allowed_hosts denSession den: denied room RIVETOS_MCP_ENABLE_MEMORY_WRITE user supplied prose",
            "golden-node golden.host golden.allowed_hosts denSession den: denied room RIVETOS_MCP_ENABLE_MEMORY_WRITE user supplied prose",
        ),
        (
            "RivetHub rh ~/.rivethub rivethub-abc RIVETHUB_TERM_MAX",
            "RivetHub rh ~/.rivethub rivethub-abc RIVETHUB_TERM_MAX",
        ),
    ] {
        assert_eq!(translate_recorded_product_names(recorded), expected);
    }
}
