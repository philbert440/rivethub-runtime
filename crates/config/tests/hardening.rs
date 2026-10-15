use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt as _;

use config::{ConfigError, Environment, Paths};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn state_paths_require_an_absolute_home() -> TestResult {
    for home in [
        None,
        Some(""),
        Some("relative"),
        Some("/home/user/../other"),
    ] {
        let env = Environment::from_vars(home.map(|home| ("HOME", home)));
        assert!(matches!(Paths::new(&env), Err(ConfigError::Home)));
        assert!(matches!(
            config::resolve_config_path(&env),
            Err(ConfigError::Home)
        ));
        assert!(config::adopt_legacy_state(&env).is_err());
    }
    let paths = Paths::new(&Environment::from_vars([("HOME", "/home/user")]))?;
    assert_eq!(
        paths.state_dir(),
        std::path::Path::new("/home/user/.rivethub")
    );
    Ok(())
}

#[test]
fn substitutions_preserve_unicode_quoting_and_literal_dollars() -> TestResult {
    let env = Environment::from_vars([("VALUE", "é🦀 '$OTHER' > /tmp/out $(date)")]);
    let mut document: serde_yaml_ng::Value =
        serde_yaml_ng::from_str("text: '${VALUE} ${MISSING} ${bad-name} $VALUE'")?;
    config::resolve_env_vars_with(&env, &mut document)?;
    let expected: serde_yaml_ng::Value =
        serde_yaml_ng::from_str("text: \"é🦀 '$OTHER' > /tmp/out $(date)  ${bad-name} $VALUE\"")?;
    assert_eq!(document, expected);
    Ok(())
}

#[test]
fn non_utf8_substitution_is_an_error() -> TestResult {
    let env = Environment::from_vars([(OsString::from("VALUE"), OsString::from_vec(vec![0xff]))]);
    let mut document: serde_yaml_ng::Value = serde_yaml_ng::from_str("text: '${VALUE}'")?;
    assert!(
        matches!(config::resolve_env_vars_with(&env, &mut document), Err(ConfigError::EnvironmentText { name }) if name == "VALUE")
    );
    Ok(())
}

#[test]
fn substituted_required_values_are_validated() {
    let text = "runtime:\n  workspace: '${ABSENT}'\n  default_agent: main\nagents:\n  main:\n    provider: p\nproviders:\n  p:\n    model: m\n    api_key: '${KEY}'\n";
    let env = Environment::from_vars([("KEY".to_owned(), format!("sk-{}", "a".repeat(32)))]);
    let result = config::Config::from_str(&env, text);
    assert!(
        matches!(result, Err(ConfigError::Validation { result }) if result.errors.iter().any(|error| error.path == "runtime.workspace") && result.warnings.iter().all(|warning| !warning.message.contains("hardcoded")))
    );
}
