use std::collections::BTreeMap;
use std::path::PathBuf;

use config::Config;
use config::Environment;
use config::MemoryBackend;
use config::ServiceSwitch;
use config::model::EmbeddedSection;
use config::model::GatewaySection;
use config::model::MemorySection;
use config::model::PostgresSection;
use config::model::SqliteSection;
use config::model::TransportSection;

fn bare_env() -> Environment {
    Environment::from_vars(std::iter::empty::<(&str, &str)>())
}

fn config_with(memory: Option<MemorySection>, gateway: Option<GatewaySection>) -> Config {
    Config {
        memory,
        gateway,
        ..Config::default()
    }
}

#[test]
fn service_switches_follow_the_configured_flags() {
    let env = bare_env();
    let plain = Config::default();
    assert_eq!(plain.service_switches(&env), vec![ServiceSwitch::Gateway]);

    let disabled = config_with(
        None,
        Some(GatewaySection {
            enabled: Some(false),
            ..GatewaySection::default()
        }),
    );
    assert_eq!(disabled.service_switches(&env), Vec::new());

    let sqlite = config_with(
        Some(MemorySection {
            backend: Some(MemoryBackend::Sqlite),
            sqlite: Some(SqliteSection {
                path: Some(PathBuf::from("/tmp/memory.sqlite")),
                workers: Some(true),
                tagging: Some(true),
                wiki_dir: Some(PathBuf::from("/tmp/wiki")),
                ..SqliteSection::default()
            }),
            ..MemorySection::default()
        }),
        None,
    );
    assert_eq!(
        sqlite.service_switches(&env),
        vec![
            ServiceSwitch::Gateway,
            ServiceSwitch::Workers,
            ServiceSwitch::Tagging,
            ServiceSwitch::Wiki,
        ]
    );

    let mut transports = BTreeMap::new();
    transports.insert(
        "mcp".to_owned(),
        TransportSection {
            host: Some(String::new()),
            ..TransportSection::default()
        },
    );
    let mcp = Config {
        transports: Some(transports),
        ..Config::default()
    };
    assert_eq!(
        mcp.service_switches(&env),
        vec![ServiceSwitch::Gateway, ServiceSwitch::Mcp]
    );

    let mut port_only = BTreeMap::new();
    port_only.insert("mcp".to_owned(), TransportSection::default());
    let quiet = Config {
        transports: Some(port_only),
        ..Config::default()
    };
    assert_eq!(quiet.service_switches(&env), vec![ServiceSwitch::Gateway]);

    let postgres = config_with(
        Some(MemorySection {
            backend: Some(MemoryBackend::Postgres),
            ..MemorySection::default()
        }),
        None,
    );
    assert_eq!(
        postgres.service_switches(&env),
        vec![ServiceSwitch::Gateway]
    );
}

#[test]
fn a_postgres_config_includes_the_agent_and_a_sqlite_config_does_not() {
    let env = bare_env();
    let postgres = config_with(
        Some(MemorySection {
            backend: Some(MemoryBackend::Postgres),
            postgres: Some(PostgresSection {
                connection_string: Some("postgres://127.0.0.1:1/rivethub".to_owned()),
                ..PostgresSection::default()
            }),
            ..MemorySection::default()
        }),
        None,
    );
    assert_eq!(
        postgres.service_switches(&env),
        vec![ServiceSwitch::Gateway, ServiceSwitch::Agent]
    );

    let sqlite = config_with(
        Some(MemorySection {
            backend: Some(MemoryBackend::Sqlite),
            postgres: Some(PostgresSection {
                connection_string: Some("postgres://127.0.0.1:1/rivethub".to_owned()),
                ..PostgresSection::default()
            }),
            sqlite: Some(SqliteSection {
                workers: Some(true),
                ..SqliteSection::default()
            }),
            ..MemorySection::default()
        }),
        None,
    );
    assert_eq!(
        sqlite.service_switches(&env),
        vec![ServiceSwitch::Gateway, ServiceSwitch::Workers,]
    );

    let from_env = Environment::from_vars([(config::POSTGRES_URL_VARIABLE, "postgres://db")]);
    assert_eq!(
        Config::default().service_switches(&from_env),
        vec![ServiceSwitch::Gateway, ServiceSwitch::Agent]
    );

    let pending = config_with(
        Some(MemorySection {
            postgres: Some(PostgresSection {
                connection_string: Some("postgres://127.0.0.1:1/rivethub".to_owned()),
                embedded: Some(EmbeddedSection::default()),
                ..PostgresSection::default()
            }),
            ..MemorySection::default()
        }),
        None,
    );
    assert_eq!(pending.service_switches(&env), vec![ServiceSwitch::Gateway]);
}

#[test]
fn an_explicit_config_path_wins_before_the_home_candidates()
-> Result<(), Box<dyn std::error::Error>> {
    let home =
        std::env::temp_dir().join(format!("rivethub-config-override-{}", std::process::id()));
    std::fs::create_dir_all(home.join(".rivethub"))?;
    std::fs::write(home.join(".rivethub").join("config.yaml"), "runtime: {}\n")?;
    let explicit = home.join("explicit.yaml");
    let home_text = home.to_str().ok_or("temp home is not utf-8")?;
    let explicit_text = explicit.to_str().ok_or("override path is not utf-8")?;
    let env = Environment::from_vars([
        (config::CONFIG_PATH_VARIABLE, explicit_text),
        ("HOME", home_text),
    ]);
    let location = config::resolve_config_path(&env)?.ok_or("override")?;
    assert_eq!(location.path(), explicit.as_path());
    assert!(!location.is_legacy());
    assert!(!explicit.exists());

    let empty = Environment::from_vars([(config::CONFIG_PATH_VARIABLE, ""), ("HOME", home_text)]);
    let location = config::resolve_config_path(&empty)?.ok_or("empty override")?;
    assert_eq!(location.path().as_os_str(), "");

    let bad = Environment::from_vars([
        (config::CONFIG_PATH_VARIABLE, explicit_text),
        ("HOME", "relative"),
    ]);
    assert!(matches!(
        config::resolve_config_path(&bad),
        Err(config::ConfigError::Home)
    ));
    std::fs::remove_dir_all(home)?;
    Ok(())
}

#[test]
fn sqlite_selection_is_the_single_rule_for_every_memory_configuration() {
    for (backend, sqlite, postgres, embedded, expected) in [
        (None, false, false, false, false),
        (None, true, false, false, true),
        (None, false, true, false, false),
        (None, true, true, false, false),
        (None, false, true, true, true),
        (Some(MemoryBackend::Postgres), true, false, false, false),
        (Some(MemoryBackend::Postgres), false, true, true, true),
        (Some(MemoryBackend::Sqlite), false, true, false, true),
        (Some(MemoryBackend::Sqlite), false, false, false, true),
    ] {
        let config = config_with(
            Some(MemorySection {
                backend,
                sqlite: sqlite.then(SqliteSection::default),
                postgres: postgres.then(|| PostgresSection {
                    embedded: embedded.then(EmbeddedSection::default),
                    ..PostgresSection::default()
                }),
                ..MemorySection::default()
            }),
            None,
        );
        assert_eq!(config.sqlite_selected(), expected, "{config:?}");
    }
    assert!(!Config::default().sqlite_selected());
}
