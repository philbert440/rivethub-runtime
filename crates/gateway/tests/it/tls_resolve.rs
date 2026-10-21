use std::path::Path;

use gateway::server::{BindError, BindSource};
use gateway::tls::resolve_tls;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn config(text: &str) -> Result<config::Config, serde_yaml_ng::Error> {
    serde_yaml_ng::from_str(text)
}

fn environment(pairs: &[(&str, &str)]) -> config::Environment {
    config::Environment::from_vars(pairs.iter().copied())
}

fn resolve(
    text: &str,
    env: &config::Environment,
    node: &str,
    shared: &Path,
) -> Result<BindSource, serde_yaml_ng::Error> {
    Ok(BindSource::resolve(&config(text)?, env, node, shared))
}

fn write(path: &Path, bytes: &[u8]) -> TestResult {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, bytes)?;
    Ok(())
}

#[test]
fn config_paths_beat_environment_and_issued_files() -> TestResult {
    let root = tempfile::tempdir()?;
    let configured_cert = root.path().join("config.crt");
    let configured_key = root.path().join("config.key");
    let configured_ca = root.path().join("config-ca.pem");
    write(&configured_cert, b"config-cert")?;
    write(&configured_key, b"config-key")?;
    write(&configured_ca, b"config-ca")?;
    let shared = root.path().join("shared");
    write(&shared.join("rivet-ca/issued/node.crt"), b"issued-cert")?;
    write(&shared.join("rivet-ca/issued/node.key"), b"issued-key")?;
    let env = environment(&[
        ("RIVETHUB_TLS_CERT", "/env/node.crt"),
        ("RIVETHUB_TLS_KEY", "/env/node.key"),
        ("RIVETHUB_TLS_CA", "/env/ca.pem"),
        ("RIVETOS_DEN_TLS_CERT", "/legacy/node.crt"),
        ("RIVETOS_DEN_TLS_KEY", "/legacy/node.key"),
        ("RIVETOS_DEN_TLS_CA", "/legacy/ca.pem"),
    ]);
    let text = format!(
        "gateway:\n  host: 0.0.0.0\n  tls_cert: {}\n  tls_key: {}\n  tls_ca: {}\n",
        configured_cert.display(),
        configured_key.display(),
        configured_ca.display()
    );
    let source = resolve(&text, &env, "node", &shared)?;
    assert_eq!(source.tls_cert.as_deref(), Some(configured_cert.as_path()));
    assert_eq!(source.tls_key.as_deref(), Some(configured_key.as_path()));
    assert_eq!(source.tls_ca.as_deref(), Some(configured_ca.as_path()));
    assert_eq!(source.notices, Vec::<String>::new());
    Ok(())
}

#[test]
fn current_environment_beats_legacy_names_and_issued_files() -> TestResult {
    let root = tempfile::tempdir()?;
    let cert = root.path().join("env.crt");
    let key = root.path().join("env.key");
    let ca = root.path().join("env-ca.pem");
    write(&cert, b"env-cert")?;
    write(&key, b"env-key")?;
    write(&ca, b"env-ca")?;
    let shared = root.path().join("shared");
    write(&shared.join("rivet-ca/issued/node.crt"), b"issued-cert")?;
    write(&shared.join("rivet-ca/issued/node.key"), b"issued-key")?;
    let cert_text = cert.to_str().ok_or("utf-8 cert")?;
    let key_text = key.to_str().ok_or("utf-8 key")?;
    let ca_text = ca.to_str().ok_or("utf-8 ca")?;
    let env = environment(&[
        ("RIVETHUB_TLS_CERT", cert_text),
        ("RIVETHUB_TLS_KEY", key_text),
        ("RIVETHUB_TLS_CA", ca_text),
        ("RIVETOS_DEN_TLS_CERT", "/legacy/node.crt"),
        ("RIVETOS_DEN_TLS_KEY", "/legacy/node.key"),
        ("RIVETOS_DEN_TLS_CA", "/legacy/ca.pem"),
    ]);
    let source = resolve("gateway:\n  host: 0.0.0.0\n", &env, "node", &shared)?;
    assert_eq!(source.tls_cert.as_deref(), Some(cert.as_path()));
    assert_eq!(source.tls_key.as_deref(), Some(key.as_path()));
    assert_eq!(source.tls_ca.as_deref(), Some(ca.as_path()));
    assert_eq!(source.notices, Vec::<String>::new());
    Ok(())
}

#[test]
fn legacy_environment_is_used_only_when_the_current_name_is_unset() -> TestResult {
    let root = tempfile::tempdir()?;
    let cert = root.path().join("legacy.crt");
    let key = root.path().join("legacy.key");
    write(&cert, b"legacy-cert")?;
    write(&key, b"legacy-key")?;
    let cert_text = cert.to_str().ok_or("utf-8 cert")?;
    let key_text = key.to_str().ok_or("utf-8 key")?;
    let env = environment(&[
        ("RIVETHUB_TLS_CERT", "  "),
        ("RIVETOS_DEN_TLS_CERT", cert_text),
        ("RIVETOS_DEN_TLS_KEY", key_text),
        ("RIVET_CA_CHAIN", "/legacy/ca.pem"),
    ]);
    let source = resolve("gateway:\n  host: 0.0.0.0\n", &env, "node", root.path())?;
    assert_eq!(source.tls_cert.as_deref(), Some(cert.as_path()));
    assert_eq!(source.tls_key.as_deref(), Some(key.as_path()));
    assert_eq!(source.tls_ca.as_deref(), Some(Path::new("/legacy/ca.pem")));
    assert_eq!(
        source.notices,
        vec![
            env.legacy_notice("RIVETHUB_TLS_CERT", "RIVETOS_DEN_TLS_CERT")
                .ok_or("cert notice")?,
            env.legacy_notice("RIVETHUB_TLS_KEY", "RIVETOS_DEN_TLS_KEY")
                .ok_or("key notice")?,
        ]
    );
    Ok(())
}

#[test]
fn issued_pair_is_used_only_when_both_files_stay_inside_the_directory() -> TestResult {
    let root = tempfile::tempdir()?;
    let shared = root.path().join("shared");
    let issued = shared.join("rivet-ca/issued");
    write(&issued.join("node.crt"), b"issued-cert")?;
    write(&issued.join("node.key"), b"issued-key")?;
    let env = environment(&[]);
    let source = resolve("gateway:\n  host: 0.0.0.0\n", &env, "node", &shared)?;
    assert_eq!(
        source.tls_cert.as_deref(),
        Some(issued.join("node.crt").canonicalize()?.as_path())
    );
    assert_eq!(
        source.tls_key.as_deref(),
        Some(issued.join("node.key").canonicalize()?.as_path())
    );
    assert_eq!(
        source.tls_ca.as_deref(),
        Some(shared.join("rivet-ca/intermediate/chain.pem").as_path())
    );

    let cert_only = root.path().join("cert-only");
    write(&cert_only.join("rivet-ca/issued/node.crt"), b"only-cert")?;
    let missing = resolve("gateway:\n  host: 0.0.0.0\n", &env, "node", &cert_only)?;
    assert!(missing.tls_cert.is_none());
    assert!(missing.tls_key.is_none());
    assert!(missing.tls_ca.is_none());

    let outside = root.path().join("outside.crt");
    write(&outside, b"escaped")?;
    let linked = root.path().join("linked");
    let linked_issued = linked.join("rivet-ca/issued");
    std::fs::create_dir_all(&linked_issued)?;
    std::os::unix::fs::symlink(&outside, linked_issued.join("node.crt"))?;
    write(&linked_issued.join("node.key"), b"inside-key")?;
    let escaped = resolve("gateway:\n  host: 0.0.0.0\n", &env, "node", &linked)?;
    assert!(escaped.tls_cert.is_none());
    assert!(escaped.tls_key.is_none());

    write(&issued.join("foo/bar.crt"), b"nested-cert")?;
    write(&issued.join("foo/bar.key"), b"nested-key")?;
    for node in ["..", "foo/bar", "", ".", "node/../node"] {
        let refused = resolve("gateway:\n  host: 0.0.0.0\n", &env, node, &shared)?;
        assert!(
            refused.tls_cert.is_none() && refused.tls_key.is_none(),
            "{node}"
        );
    }
    Ok(())
}

#[test]
fn configured_certificate_can_take_its_key_from_the_issued_pair() -> TestResult {
    let root = tempfile::tempdir()?;
    let shared = root.path().join("shared");
    let issued = shared.join("rivet-ca/issued");
    write(&issued.join("node.crt"), b"issued-cert")?;
    write(&issued.join("node.key"), b"issued-key")?;
    let cert = root.path().join("config.crt");
    write(&cert, b"config-cert")?;
    let text = format!("gateway:\n  tls_cert: {}\n", cert.display());
    let source = resolve(&text, &environment(&[]), "node", &shared)?;
    assert_eq!(source.tls_cert.as_deref(), Some(cert.as_path()));
    assert_eq!(
        source.tls_key.as_deref(),
        Some(issued.join("node.key").canonicalize()?.as_path())
    );
    Ok(())
}

#[test]
fn ca_chain_follows_config_then_environment_then_the_shared_default() -> TestResult {
    let root = tempfile::tempdir()?;
    let cert = root.path().join("node.crt");
    let key = root.path().join("node.key");
    write(&cert, b"cert")?;
    write(&key, b"key")?;
    let material = format!(
        "gateway:\n  host: 0.0.0.0\n  tls_cert: {}\n  tls_key: {}\n",
        cert.display(),
        key.display()
    );
    let configured = root.path().join("from-config.pem");
    let current = root.path().join("from-current.pem");
    let legacy = root.path().join("from-legacy.pem");
    let chain = root.path().join("from-chain.pem");
    let with_config = format!("{material}  tls_ca: {}\n", configured.display());
    let current_text = current.to_str().ok_or("utf-8")?;
    let legacy_text = legacy.to_str().ok_or("utf-8")?;
    let chain_text = chain.to_str().ok_or("utf-8")?;
    let env = environment(&[
        ("RIVETHUB_TLS_CA", current_text),
        ("RIVETOS_DEN_TLS_CA", legacy_text),
        ("RIVET_CA_CHAIN", chain_text),
    ]);
    assert_eq!(
        resolve(&with_config, &env, "node", root.path())?
            .tls_ca
            .as_deref(),
        Some(configured.as_path())
    );
    assert_eq!(
        resolve(&material, &env, "node", root.path())?
            .tls_ca
            .as_deref(),
        Some(current.as_path())
    );
    let legacy_env = environment(&[
        ("RIVETOS_DEN_TLS_CA", legacy_text),
        ("RIVET_CA_CHAIN", chain_text),
    ]);
    let legacy_source = resolve(&material, &legacy_env, "node", root.path())?;
    assert_eq!(legacy_source.tls_ca.as_deref(), Some(legacy.as_path()));
    assert_eq!(
        legacy_source.notices,
        vec![
            legacy_env
                .legacy_notice("RIVETHUB_TLS_CA", "RIVETOS_DEN_TLS_CA")
                .ok_or("ca notice")?
        ]
    );
    let chain_env = environment(&[("RIVET_CA_CHAIN", chain_text)]);
    assert_eq!(
        resolve(&material, &chain_env, "node", root.path())?
            .tls_ca
            .as_deref(),
        Some(chain.as_path())
    );
    let plain = resolve(&material, &environment(&[]), "node", root.path())?;
    let default = root.path().join("rivet-ca/intermediate/chain.pem");
    assert_eq!(plain.tls_ca.as_deref(), Some(default.as_path()));
    let error = resolve_tls(&plain).expect_err("missing chain");
    let message = error.to_string();
    assert!(matches!(error, BindError::CaMissing { path } if path == default));
    assert!(message.contains(&default.display().to_string()));
    Ok(())
}

struct Requirement {
    text: &'static str,
    pairs: &'static [(&'static str, &'static str)],
    required: bool,
}

fn requirements() -> [Requirement; 13] {
    [
        Requirement {
            text: "",
            pairs: &[],
            required: true,
        },
        Requirement {
            text: "gateway:\n  tls_require_client: false\n",
            pairs: &[("RIVETHUB_TLS_REQUIRE_CLIENT", "1")],
            required: false,
        },
        Requirement {
            text: "gateway:\n  tls_require_client: true\n",
            pairs: &[("RIVETHUB_TLS_REQUIRE_CLIENT", "0")],
            required: false,
        },
        Requirement {
            text: "",
            pairs: &[("RIVETHUB_TLS_REQUIRE_CLIENT", " 0 ")],
            required: false,
        },
        Requirement {
            text: "",
            pairs: &[("RIVETHUB_TLS_REQUIRE_CLIENT", " OFF ")],
            required: false,
        },
        Requirement {
            text: "",
            pairs: &[("RIVETHUB_TLS_REQUIRE_CLIENT", "False")],
            required: false,
        },
        Requirement {
            text: "",
            pairs: &[("RIVETHUB_TLS_REQUIRE_CLIENT", " no ")],
            required: false,
        },
        Requirement {
            text: "",
            pairs: &[("RIVETHUB_TLS_REQUIRE_CLIENT", "1")],
            required: true,
        },
        Requirement {
            text: "",
            pairs: &[("RIVETHUB_TLS_REQUIRE_CLIENT", "on")],
            required: true,
        },
        Requirement {
            text: "",
            pairs: &[("RIVETHUB_TLS_REQUIRE_CLIENT", "yes")],
            required: true,
        },
        Requirement {
            text: "",
            pairs: &[("RIVETHUB_TLS_REQUIRE_CLIENT", "garbage")],
            required: true,
        },
        Requirement {
            text: "",
            pairs: &[
                ("RIVETHUB_TLS_REQUIRE_CLIENT", "1"),
                ("RIVETOS_DEN_TLS_REQUIRE_CLIENT", "0"),
            ],
            required: true,
        },
        Requirement {
            text: "",
            pairs: &[("RIVETOS_DEN_TLS_REQUIRE_CLIENT", "0")],
            required: false,
        },
    ]
}

#[test]
fn client_certificates_are_required_unless_config_or_environment_turns_them_off() -> TestResult {
    let root = tempfile::tempdir()?;
    for case in requirements() {
        let env = environment(case.pairs);
        let source = resolve(case.text, &env, "node", root.path())?;
        assert_eq!(source.require_client, case.required, "{}", case.text);
    }
    let legacy = environment(&[("RIVETOS_DEN_TLS_REQUIRE_CLIENT", "0")]);
    let source = resolve("", &legacy, "node", root.path())?;
    assert!(!source.require_client);
    assert_eq!(
        source.notices,
        vec![
            legacy
                .legacy_notice(
                    "RIVETHUB_TLS_REQUIRE_CLIENT",
                    "RIVETOS_DEN_TLS_REQUIRE_CLIENT"
                )
                .ok_or("requirement notice")?
        ]
    );
    let empty_host = resolve(
        "gateway:\n  host: \"\"\n",
        &environment(&[]),
        "node",
        root.path(),
    )?;
    assert_eq!(empty_host.host, "");
    assert!(empty_host.require_client);
    Ok(())
}

#[test]
fn an_absent_gateway_section_keeps_the_loopback_default() -> TestResult {
    let root = tempfile::tempdir()?;
    let source = resolve("", &environment(&[]), "node", root.path())?;
    assert_eq!(source.host, "127.0.0.1");
    assert_eq!(source.port, 5174);
    assert!(source.require_client);
    assert!(source.tls_cert.is_none());
    Ok(())
}
