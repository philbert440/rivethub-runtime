#![cfg(test)]

use std::path::PathBuf;

use config::{Config, Environment};

#[test]
fn wiki_directory_uses_the_existing_key_then_contract_environment_then_shared() {
    let config = Config::default();
    let env = Environment::from_vars([("RIVETHUB_SHARED_DIR", "/srv/shared")]);
    assert_eq!(
        config.wiki_directory(&env),
        PathBuf::from("/srv/shared/wiki")
    );
    let env = env.with_override("WIKI_DIR", "/srv/pages");
    assert_eq!(config.wiki_directory(&env), PathBuf::from("/srv/pages"));
    let config: Config =
        serde_yaml_ng::from_str("memory:\n  sqlite:\n    wiki_dir: /srv/configured\n")
            .expect("config");
    assert_eq!(
        config.wiki_directory(&env),
        PathBuf::from("/srv/configured")
    );
}
