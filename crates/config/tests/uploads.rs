use std::time::Duration;

use config::Environment;

#[test]
fn upload_limits_prefer_the_current_name() {
    let env = Environment::from_vars([
        ("RIVETHUB_UPLOAD_MAX_BYTES", "1024"),
        ("RIVETOS_DEN_UPLOAD_MAX_BYTES", "2048"),
        ("RIVETHUB_UPLOAD_TTL_MS", "5000"),
        ("RIVETOS_DEN_UPLOAD_TTL", "1000"),
    ]);
    let settings = env.upload_settings();
    assert_eq!(settings.max_bytes, 1024);
    assert_eq!(settings.ttl, Duration::from_millis(5000));
    assert_eq!(settings.ignored, Vec::<String>::new());
    assert_eq!(
        env.legacy_notices(),
        Vec::<String>::new(),
        "a set current name suppresses the legacy warning"
    );
}

#[test]
fn legacy_upload_names_apply_once_and_warn() {
    let env = Environment::from_vars([
        ("RIVETOS_DEN_UPLOAD_MAX_BYTES", "4096"),
        ("RIVETOS_DEN_UPLOAD_TTL", "2500"),
    ]);
    let settings = env.upload_settings();
    assert_eq!(settings.max_bytes, 4096);
    assert_eq!(settings.ttl, Duration::from_millis(2500));
    let notices = env.legacy_notices();
    assert_eq!(
        notices
            .iter()
            .filter(|notice| notice.contains("RIVETOS_DEN_UPLOAD_MAX_BYTES"))
            .count(),
        1
    );
    assert_eq!(
        notices
            .iter()
            .filter(|notice| notice.contains("RIVETOS_DEN_UPLOAD_TTL"))
            .count(),
        1
    );
    assert!(
        notices
            .iter()
            .any(|notice| notice.contains("RIVETHUB_UPLOAD_MAX_BYTES"))
    );
    assert!(
        notices
            .iter()
            .any(|notice| notice.contains("RIVETHUB_UPLOAD_TTL_MS"))
    );
}

#[test]
fn unset_zero_and_unreadable_upload_limits_keep_the_defaults() {
    let defaults = Environment::from_vars(Vec::<(&str, &str)>::new()).upload_settings();
    assert_eq!(defaults.max_bytes, config::DEFAULT_UPLOAD_MAX_BYTES);
    assert_eq!(
        defaults.ttl,
        Duration::from_millis(config::DEFAULT_UPLOAD_TTL_MS)
    );
    assert_eq!(defaults.ignored, Vec::<String>::new());

    let env = Environment::from_vars([
        ("RIVETHUB_UPLOAD_MAX_BYTES", "0"),
        ("RIVETHUB_UPLOAD_TTL_MS", "nope"),
    ]);
    let settings = env.upload_settings();
    assert_eq!(settings.max_bytes, config::DEFAULT_UPLOAD_MAX_BYTES);
    assert_eq!(
        settings.ttl,
        Duration::from_millis(config::DEFAULT_UPLOAD_TTL_MS)
    );
    assert_eq!(
        settings.ignored,
        vec![
            "RIVETHUB_UPLOAD_MAX_BYTES=0".to_owned(),
            "RIVETHUB_UPLOAD_TTL_MS=nope".to_owned(),
        ]
    );
}
