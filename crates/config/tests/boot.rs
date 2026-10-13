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

#[test]
fn log_level_parses_the_recorded_names() {
    for (value, want) in [
        ("error", LogLevel::Error),
        ("warn", LogLevel::Warn),
        ("info", LogLevel::Info),
        ("debug", LogLevel::Debug),
    ] {
        let setting = env_from(&[("RIVETOS_LOG_LEVEL", value)]).log_level();
        assert_eq!(setting.level, want, "RIVETOS_LOG_LEVEL={value}");
        assert_eq!(setting.ignored, None);
    }
}

#[test]
fn blank_log_level_is_unset() {
    let setting = env_from(&[("RIVETOS_LOG_LEVEL", "   ")]).log_level();
    assert_eq!(setting.level, LogLevel::Info);
    assert_eq!(setting.ignored, None);
}

#[test]
fn unknown_log_level_keeps_the_default_and_names_the_value() {
    let setting = env_from(&[("RIVETOS_LOG_LEVEL", " verbose ")]).log_level();
    assert_eq!(setting.level, LogLevel::Info);
    assert_eq!(setting.ignored.as_deref(), Some("verbose"));
}

#[test]
fn node_name_walks_the_recorded_chain() {
    let env = env_from(&[("RIVETOS_DEN_NODE_NAME", " golden-node ")]);
    assert_eq!(env.node_name("box"), "golden-node");

    let env = env_from(&[
        ("RIVETOS_DEN_NODE_NAME", "   "),
        ("RIVETOS_DEN_NODE_ID", "node-7"),
    ]);
    assert_eq!(env.node_name("box"), "node-7");

    let env = env_from(&[("RIVETOS_DEN_NODE_ID", "node-7")]);
    assert_eq!(env.node_name("box"), "node-7");

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
    let env = env_from(&[("RIVETOS_PG_URL", "postgres://env-db/rivet")]);
    assert_eq!(
        config.memory_postgres_url(&env).as_deref(),
        Some("postgres://config-db/rivet")
    );
    Ok(())
}

#[test]
fn memory_postgres_url_falls_back_to_the_env() -> Result<(), Box<dyn std::error::Error>> {
    let config = config_with_connection_string(
        "runtime:\n  workspace: /home/user/work\n  default_agent: main\nagents:\n  main:\n    provider: p\nproviders:\n  p:\n    model: m\n",
    )?;
    let env = env_from(&[("RIVETOS_PG_URL", " postgres://env-db/rivet ")]);
    assert_eq!(
        config.memory_postgres_url(&env).as_deref(),
        Some("postgres://env-db/rivet")
    );
    assert_eq!(config.memory_postgres_url(&env_from(&[])), None);
    Ok(())
}

#[test]
fn blank_connection_strings_fall_through() -> Result<(), Box<dyn std::error::Error>> {
    let config = config_with_connection_string(
        "runtime:\n  workspace: /home/user/work\n  default_agent: main\nagents:\n  main:\n    provider: p\nproviders:\n  p:\n    model: m\nmemory:\n  postgres:\n    connection_string: \"   \"\n",
    )?;
    let env = env_from(&[("RIVETOS_PG_URL", "postgres://env-db/rivet")]);
    assert_eq!(
        config.memory_postgres_url(&env).as_deref(),
        Some("postgres://env-db/rivet")
    );

    let env = env_from(&[("RIVETOS_PG_URL", "   ")]);
    assert_eq!(config.memory_postgres_url(&env), None);
    Ok(())
}
