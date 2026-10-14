use config::{Config, Environment, LogLevel};

fn env_from(pairs: &[(&str, &str)]) -> Environment {
    Environment::from_vars(pairs.iter().map(|(key, value)| (*key, *value)))
}

#[test]
fn log_level_defaults_to_info_when_unset() {
    let setting = env_from(&[]).log_level();
    assert_eq!(setting.level, LogLevel::Info);
    assert_eq!(setting.ignored, None);
}

fn alias_warns(env: &Environment, current: &str, legacy: &str) {
    let notices = env.legacy_notices();
    let matched: Vec<&String> = notices
        .iter()
        .filter(|notice| notice.contains(legacy))
        .collect();
    assert_eq!(matched.len(), 1, "{matched:?}");
    assert!(
        matched.iter().any(|notice| notice.contains(current)),
        "{matched:?}"
    );
}

#[test]
fn log_level_parses_the_recorded_names() {
    for (value, want) in [
        ("error", LogLevel::Error),
        ("warn", LogLevel::Warn),
        ("info", LogLevel::Info),
        ("debug", LogLevel::Debug),
    ] {
        let current = env_from(&[("RIVETHUB_LOG_LEVEL", value)]);
        assert_eq!(
            current.log_level().level,
            want,
            "RIVETHUB_LOG_LEVEL={value}"
        );
        assert_eq!(current.log_level().ignored, None);
        assert_eq!(current.legacy_notices(), [] as [String; 0]);

        let legacy = env_from(&[("RIVETOS_LOG_LEVEL", value)]);
        assert_eq!(legacy.log_level().level, want, "RIVETOS_LOG_LEVEL={value}");
        assert_eq!(legacy.log_level().ignored, None);
        alias_warns(&legacy, "RIVETHUB_LOG_LEVEL", "RIVETOS_LOG_LEVEL");
    }
}

#[test]
fn blank_log_level_is_unset() {
    let setting = env_from(&[("RIVETOS_LOG_LEVEL", "   ")]).log_level();
    assert_eq!(setting.level, LogLevel::Info);
    assert_eq!(setting.ignored, None);
    alias_warns(
        &env_from(&[("RIVETOS_LOG_LEVEL", "   ")]),
        "RIVETHUB_LOG_LEVEL",
        "RIVETOS_LOG_LEVEL",
    );
}

#[test]
fn unknown_log_level_keeps_the_default_and_names_the_value() {
    let setting = env_from(&[("RIVETHUB_LOG_LEVEL", " verbose ")]).log_level();
    assert_eq!(setting.level, LogLevel::Info);
    assert_eq!(setting.ignored.as_deref(), Some("verbose"));

    let legacy = env_from(&[("RIVETOS_LOG_LEVEL", " verbose ")]);
    assert_eq!(legacy.log_level().ignored.as_deref(), Some("verbose"));
    alias_warns(&legacy, "RIVETHUB_LOG_LEVEL", "RIVETOS_LOG_LEVEL");
}

#[test]
fn node_name_walks_the_recorded_chain() {
    let current = env_from(&[("RIVETHUB_NODE_NAME", " golden-node ")]);
    assert_eq!(current.node_name("box"), "golden-node");
    assert_eq!(current.legacy_notices(), [] as [String; 0]);

    let legacy = env_from(&[("RIVETOS_DEN_NODE_NAME", " golden-node ")]);
    assert_eq!(legacy.node_name("box"), "golden-node");
    alias_warns(&legacy, "RIVETHUB_NODE_NAME", "RIVETOS_DEN_NODE_NAME");

    let env = env_from(&[
        ("RIVETHUB_NODE_NAME", "   "),
        ("RIVETOS_DEN_NODE_ID", "node-7"),
    ]);
    assert_eq!(env.node_name("box"), "node-7");
    alias_warns(&env, "RIVETHUB_NODE_ID", "RIVETOS_DEN_NODE_ID");

    let env = env_from(&[
        ("RIVETOS_DEN_NODE_NAME", "   "),
        ("RIVETHUB_NODE_ID", "node-7"),
    ]);
    assert_eq!(env.node_name("box"), "node-7");
    alias_warns(&env, "RIVETHUB_NODE_NAME", "RIVETOS_DEN_NODE_NAME");
    assert!(
        env.legacy_notices()
            .iter()
            .all(|notice| !notice.contains("RIVETHUB_NODE_ID")),
        "{:?}",
        env.legacy_notices()
    );

    let env = env_from(&[("RIVETOS_DEN_NODE_ID", "node-7")]);
    assert_eq!(env.node_name("box"), "node-7");
    alias_warns(&env, "RIVETHUB_NODE_ID", "RIVETOS_DEN_NODE_ID");

    let both = env_from(&[
        ("RIVETHUB_NODE_NAME", "primary"),
        ("RIVETOS_DEN_NODE_NAME", "legacy"),
        ("RIVETOS_DEN_NODE_ID", "node-7"),
    ]);
    assert_eq!(both.node_name("box"), "primary");
    alias_warns(&both, "RIVETHUB_NODE_ID", "RIVETOS_DEN_NODE_ID");
    assert!(
        both.legacy_notices()
            .iter()
            .all(|notice| !notice.contains("RIVETHUB_NODE_NAME")),
        "{:?}",
        both.legacy_notices()
    );

    assert_eq!(env_from(&[]).node_name(" box "), "box");
    assert_eq!(env_from(&[]).node_name(""), "local");
    assert_eq!(env_from(&[]).node_name("   "), "local");
}

fn config_with_connection_string(text: &str) -> Result<Config, Box<dyn std::error::Error>> {
    let loaded = Config::from_str(&Environment::default(), text)?;
    Ok(loaded.config)
}

#[test]
fn memory_postgres_url_prefers_the_config_over_the_env() -> Result<(), Box<dyn std::error::Error>> {
    let config = config_with_connection_string(
        "runtime:\n  workspace: /home/user/work\n  default_agent: main\nagents:\n  main:\n    provider: p\nproviders:\n  p:\n    model: m\nmemory:\n  postgres:\n    connection_string: postgres://config-db/rivet\n",
    )?;
    let env = env_from(&[
        ("RIVETHUB_PG_URL", "postgres://new-db/rivet"),
        ("RIVETOS_PG_URL", "postgres://env-db/rivet"),
    ]);
    assert_eq!(
        config.memory_postgres_url(&env).as_deref(),
        Some("postgres://config-db/rivet")
    );
    assert_eq!(env.legacy_notices(), [] as [String; 0]);
    Ok(())
}

#[test]
fn memory_postgres_url_falls_back_to_the_env() -> Result<(), Box<dyn std::error::Error>> {
    let config = config_with_connection_string(
        "runtime:\n  workspace: /home/user/work\n  default_agent: main\nagents:\n  main:\n    provider: p\nproviders:\n  p:\n    model: m\n",
    )?;
    let current = env_from(&[("RIVETHUB_PG_URL", " postgres://env-db/rivet ")]);
    assert_eq!(
        config.memory_postgres_url(&current).as_deref(),
        Some("postgres://env-db/rivet")
    );
    assert_eq!(current.legacy_notices(), [] as [String; 0]);

    let legacy = env_from(&[("RIVETOS_PG_URL", " postgres://env-db/rivet ")]);
    assert_eq!(
        config.memory_postgres_url(&legacy).as_deref(),
        Some("postgres://env-db/rivet")
    );
    alias_warns(&legacy, "RIVETHUB_PG_URL", "RIVETOS_PG_URL");
    assert_eq!(config.memory_postgres_url(&env_from(&[])), None);
    Ok(())
}

#[test]
fn blank_connection_strings_fall_through() -> Result<(), Box<dyn std::error::Error>> {
    let config = config_with_connection_string(
        "runtime:\n  workspace: /home/user/work\n  default_agent: main\nagents:\n  main:\n    provider: p\nproviders:\n  p:\n    model: m\nmemory:\n  postgres:\n    connection_string: \"   \"\n",
    )?;
    let env = env_from(&[
        ("RIVETHUB_PG_URL", "   "),
        ("RIVETOS_PG_URL", "postgres://env-db/rivet"),
    ]);
    assert_eq!(
        config.memory_postgres_url(&env).as_deref(),
        Some("postgres://env-db/rivet")
    );
    alias_warns(&env, "RIVETHUB_PG_URL", "RIVETOS_PG_URL");

    let env = env_from(&[("RIVETOS_PG_URL", "   ")]);
    assert_eq!(config.memory_postgres_url(&env), None);
    alias_warns(&env, "RIVETHUB_PG_URL", "RIVETOS_PG_URL");
    Ok(())
}
