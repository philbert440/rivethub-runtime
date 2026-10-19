use std::path::PathBuf;

use config::{Config, Environment};

fn workspace(env: &Environment, value: &str) -> Result<Option<PathBuf>, config::ConfigError> {
    let text = format!(
        "runtime:\n  workspace: {value}\n  default_agent: main\nagents:\n  main:\n    provider: p\nproviders:\n  p:\n    model: m\n"
    );
    Ok(Config::from_str(env, &text)?
        .config
        .runtime
        .and_then(|runtime| runtime.workspace))
}

fn home() -> Environment {
    Environment::from_vars([("HOME", "/home/user")])
}

#[test]
fn a_tilde_workspace_resolves_against_home() -> Result<(), config::ConfigError> {
    assert_eq!(
        workspace(&home(), "~/.rivetos/workspace")?,
        Some(PathBuf::from("/home/user/.rivetos/workspace"))
    );
    Ok(())
}

#[test]
fn a_bare_tilde_workspace_is_home() -> Result<(), config::ConfigError> {
    assert_eq!(
        workspace(&home(), "\"~\"")?,
        Some(PathBuf::from("/home/user"))
    );
    Ok(())
}

#[test]
fn an_absolute_workspace_is_unchanged() -> Result<(), config::ConfigError> {
    assert_eq!(
        workspace(&home(), "/srv/work")?,
        Some(PathBuf::from("/srv/work"))
    );
    Ok(())
}

#[test]
fn another_users_tilde_is_not_expanded() -> Result<(), config::ConfigError> {
    assert_eq!(
        workspace(&home(), "~other/work")?,
        Some(PathBuf::from("~other/work"))
    );
    Ok(())
}

#[test]
fn without_home_the_tilde_is_kept() -> Result<(), config::ConfigError> {
    assert_eq!(
        workspace(&Environment::default(), "~/work")?,
        Some(PathBuf::from("~/work"))
    );
    Ok(())
}
