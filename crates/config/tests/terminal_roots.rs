use std::path::PathBuf;

use config::{Config, Environment, Profile, validate_document};

#[test]
fn terminal_roots_default_to_files_home_and_shared_unless_listed() {
    let env = Environment::from_vars([("HOME", "/home/u"), ("RIVETHUB_SHARED_DIR", "/srv/shared")]);
    let config: Config =
        serde_yaml_ng::from_str("gateway:\n  files_root: /srv/shared\n").expect("config");
    assert_eq!(
        config.terminal_roots(&env),
        [PathBuf::from("/srv/shared"), PathBuf::from("/home/u")]
    );
    let config: Config =
        serde_yaml_ng::from_str("gateway:\n  terminal:\n    roots: [/srv/work]\n").expect("config");
    assert_eq!(config.terminal_roots(&env), [PathBuf::from("/srv/work")]);
}

#[test]
fn terminal_roots_must_be_absolute() {
    let document = serde_yaml_ng::from_str("gateway:\n  terminal:\n    roots: [/srv/work, work]\n")
        .expect("document");
    let result = validate_document(&document, Profile::Node);
    assert!(
        result
            .errors
            .iter()
            .any(|finding| finding.path == "gateway.terminal.roots"),
        "{:?}",
        result.errors
    );
}
