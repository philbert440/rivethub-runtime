use std::fs;
use std::path::{Path, PathBuf};

use config::{
    Config, ConfigError, ConfigLocation, Environment, Profile, ValidationResult,
    standalone_config_yaml,
};

fn temp_home(label: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "rivethub-standalone-{label}-{}-{nanos}",
        std::process::id()
    ));
    fs::create_dir_all(&path)?;
    Ok(path)
}

fn env_for(home: &Path) -> Result<Environment, Box<dyn std::error::Error>> {
    let Some(text) = home.to_str() else {
        return Err("home is not utf-8".into());
    };
    Ok(Environment::from_vars([("HOME", text)]))
}

#[test]
fn standalone_yaml_omits_operator_sections() -> Result<(), Box<dyn std::error::Error>> {
    let home = temp_home("yaml")?;
    let outcome = standalone_round_trip(&home);
    fs::remove_dir_all(&home)?;
    outcome
}

fn standalone_round_trip(home: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let database = home.join("memory.sqlite");
    let yaml = standalone_config_yaml(&database, "0.0.0.0", 5174)?;
    assert!(yaml.contains("backend: sqlite"), "{yaml}");
    assert!(yaml.contains("0.0.0.0"), "{yaml}");
    assert!(!yaml.contains("runtime"), "{yaml}");
    assert!(!yaml.contains("agents"), "{yaml}");
    assert!(!yaml.contains("providers"), "{yaml}");
    assert!(!yaml.contains("base_url"), "{yaml}");
    let loaded = load_text(home, "config.yaml", &yaml, Profile::Standalone)?;
    assert!(loaded.config.runtime.is_none());
    assert!(loaded.config.agents.is_none());
    assert!(loaded.config.providers.is_none());
    assert!(loaded.config.sqlite_selected());
    let node = load_outcome(home, "node.yaml", &yaml, Profile::Node)?;
    assert_eq!(
        findings(&validation(node)?),
        [
            ("runtime", "Missing required section \"runtime\""),
            (
                "agents",
                "Missing required section \"agents\" — define at least one agent",
            ),
            (
                "providers",
                "Missing required section \"providers\" — define at least one provider",
            ),
        ]
    );
    let broken = sqlite_document(&database, "den:\n  host: 127.0.0.1\n  port: 0\n");
    let port = load_outcome(home, "port.yaml", &broken, Profile::Standalone)?;
    assert_eq!(
        findings(&validation(port)?),
        [(
            "den.port",
            "\"den.port\" must be an integer between 1 and 65535",
        )]
    );
    let empty = sqlite_document(&database, "agents: {}\n");
    let agents = load_outcome(home, "agents.yaml", &empty, Profile::Standalone)?;
    assert_eq!(
        findings(&validation(agents)?),
        [("agents", "\"agents\" is empty — define at least one agent")]
    );
    let runtime = sqlite_document(&database, "runtime:\n  workspace: /tmp\n");
    let partial = load_outcome(home, "runtime.yaml", &runtime, Profile::Standalone)?;
    assert_eq!(
        findings(&validation(partial)?),
        [(
            "runtime.default_agent",
            "Missing required field \"runtime.default_agent\"",
        )]
    );
    let mistyped = "den:\n  port: \"no\"\n";
    let typed = validation(load_outcome(
        home,
        "type.yaml",
        mistyped,
        Profile::Standalone,
    )?)?;
    assert!(
        typed
            .errors
            .iter()
            .any(|finding| finding.message.contains("wrong type")),
        "{typed:?}"
    );
    Ok(())
}

fn sqlite_document(database: &Path, extra: &str) -> String {
    format!(
        "memory:\n  backend: sqlite\n  sqlite:\n    path: \"{}\"\n{extra}",
        database.display()
    )
}

fn load_text(
    home: &Path,
    name: &str,
    yaml: &str,
    profile: Profile,
) -> Result<config::Loaded, Box<dyn std::error::Error>> {
    match load_outcome(home, name, yaml, profile)? {
        Ok(loaded) => Ok(loaded),
        Err(error) => Err(error.into()),
    }
}

fn load_outcome(
    home: &Path,
    name: &str,
    yaml: &str,
    profile: Profile,
) -> Result<Result<config::Loaded, ConfigError>, Box<dyn std::error::Error>> {
    let path = home.join(name);
    fs::write(&path, yaml)?;
    let env = env_for(home)?;
    Ok(Config::load(&env, &ConfigLocation::Current(path), profile))
}

fn validation(
    outcome: Result<config::Loaded, ConfigError>,
) -> Result<ValidationResult, Box<dyn std::error::Error>> {
    match outcome {
        Err(ConfigError::Validation { result }) => Ok(result),
        Err(error) => Err(error.into()),
        Ok(_) => Err("document loaded".into()),
    }
}

fn findings(result: &ValidationResult) -> Vec<(&str, &str)> {
    result
        .errors
        .iter()
        .map(|finding| (finding.path.as_str(), finding.message.as_str()))
        .collect()
}
