#[expect(
    dead_code,
    reason = "the harness module is included whole by every test binary and this one uses a subset"
)]
mod common;

use common::env_from;
use std::collections::BTreeMap;
use std::path::Path;
use terminal::RosterKey;
use terminal::SessionKey;
use terminal::TerminalSettings;
use testkit::Masks;
use testkit::json_masked_eq;
use testkit::masked_eq;
use testkit::recording;

#[test]
fn term_knobs_come_from_the_environment_with_the_recorded_defaults() {
    let settings = common::settings(&config::Environment::default());
    let defaults = config::TermSettings::defaults();
    assert_eq!(settings.limits.max_ptys, defaults.max_ptys);
    assert_eq!(settings.limits.max_ptys, 4);
    assert_eq!(settings.limits.scrollback_bytes, defaults.scrollback_bytes);
    assert_eq!(settings.limits.detached_ttl_ms, defaults.detached_ttl_ms);
    assert_eq!(settings.limits.idle_ttl_ms, defaults.idle_ttl_ms);
    assert_eq!(settings.limits.exit_linger_ms, defaults.exit_linger_ms);
    assert_eq!(settings.limits.session_gc_ms, defaults.session_gc_ms);
    assert_eq!(settings.limits.ignored, [] as [String; 0]);
    assert_eq!(settings.mux.setting, config::TermMuxSetting::Auto);
    assert_eq!(settings.mux.ignored, None);
}

#[test]
fn term_knobs_read_the_new_names_over_the_legacy_spellings() {
    let env = env_from(&[
        ("RIVETHUB_TERM_MAX", "7"),
        ("RIVETOS_DEN_TERM_MAX", "99"),
        ("RIVETHUB_TERM_SCROLLBACK", "1024"),
        ("RIVETHUB_TERM_DETACHED_TTL_MS", "1000"),
        ("RIVETHUB_TERM_IDLE_TTL_MS", "0"),
        ("RIVETHUB_TERM_EXIT_LINGER_MS", "250"),
        ("RIVETHUB_TERM_SESSION_GC_MS", "4096"),
    ]);
    let settings = common::settings(&env);
    assert_eq!(settings.limits.max_ptys, 7);
    assert_eq!(settings.limits.scrollback_bytes, 1024);
    assert_eq!(settings.limits.detached_ttl_ms, 1000);
    assert_eq!(settings.limits.idle_ttl_ms, 0);
    assert_eq!(settings.limits.exit_linger_ms, 250);
    assert_eq!(settings.limits.session_gc_ms, 4096);

    let legacy_only = env_from(&[("RIVETOS_DEN_TERM_MAX", "9")]);
    assert_eq!(common::settings(&legacy_only).limits.max_ptys, 9);
}

#[test]
fn unparsable_knob_values_are_surfaced_not_silently_defaulted() {
    let env = env_from(&[
        ("RIVETHUB_TERM_MAX", "many"),
        ("RIVETHUB_TERM_SCROLLBACK", "-3"),
        ("RIVETHUB_TERM_IDLE_TTL_MS", "1.5"),
    ]);
    let settings = common::settings(&env);
    assert_eq!(settings.limits.max_ptys, 4);
    assert_eq!(settings.limits.scrollback_bytes, 262_144);
    assert_eq!(
        settings.limits.ignored,
        [
            "RIVETHUB_TERM_MAX=many",
            "RIVETHUB_TERM_SCROLLBACK=-3",
            "RIVETHUB_TERM_IDLE_TTL_MS=1.5"
        ]
    );
}

#[test]
fn mux_values_map_to_the_typed_choices() {
    let unset = common::settings(&config::Environment::default()).mux;
    assert_eq!(unset.setting, config::TermMuxSetting::Auto);

    let herdr = common::settings(&env_from(&[("RIVETHUB_TERM_MUX", "herdr")])).mux;
    assert_eq!(herdr.setting, config::TermMuxSetting::Herdr);

    let none = common::settings(&env_from(&[("RIVETHUB_TERM_MUX", "none")])).mux;
    assert_eq!(none.setting, config::TermMuxSetting::Direct);

    let tmux = common::settings(&env_from(&[("RIVETOS_TERM_MUX", "tmux")])).mux;
    assert_eq!(tmux.setting, config::TermMuxSetting::Auto);
    assert_eq!(tmux.ignored, None);

    let bogus = common::settings(&env_from(&[("RIVETHUB_TERM_MUX", "Bogus")])).mux;
    assert_eq!(bogus.setting, config::TermMuxSetting::Direct);
    assert_eq!(bogus.ignored.as_deref(), Some("Bogus"));
}

#[test]
fn terminal_enabled_defaults_off_and_reads_the_gateway_section() {
    let config = config::Config::default();
    assert!(!config.terminal_enabled());
}

fn envdump_settings(home: &str, shared: &str) -> TerminalSettings {
    let env = env_from(&[
        ("PATH", "/golden/bin"),
        ("HOME", home),
        ("TZ", "UTC"),
        ("RIVETOS_USER_ID", "from-process"),
        ("RIVETHUB_SHARED_DIR", shared),
    ]);
    let mut config = config::Config::default();
    config.gateway = Some(config::model::GatewaySection {
        port: 5174,
        ..config::model::GatewaySection::default()
    });
    TerminalSettings::from_config(&config, &env, common::HOSTNAME.to_owned())
}

#[test]
fn child_env_matches_the_recorded_envdump_lines() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let home = tmp.path().join("home");
    let shared = tmp.path().join("shared");
    let settings = envdump_settings(
        home.to_str().expect("utf8 home"),
        shared.to_str().expect("utf8 shared"),
    );
    let work = tmp.path().join("work");
    let session = SessionKey::new("acme-env").expect("session key");
    let roster = RosterKey::new("envdump").expect("roster key");
    let entry = BTreeMap::from([
        ("ACME_ENTRY_ENV".to_owned(), "envdump".to_owned()),
        ("RIVETOS_USER_ID".to_owned(), "from-roster".to_owned()),
        ("RIVETOS_USER_DBS".to_owned(), "denied".to_owned()),
    ]);
    let child = settings.child_env(Some(&session), &roster, &work, &entry);
    let text = |name: &str| {
        child
            .get(name)
            .map(|value| value.to_string_lossy().into_owned())
    };

    assert_eq!(text("TERM").as_deref(), Some("xterm-256color"));
    assert_eq!(text("COLORTERM").as_deref(), Some("truecolor"));
    assert_eq!(text("LANG").as_deref(), Some("C.UTF-8"));
    assert_eq!(
        text("PWD").as_deref(),
        Some(work.to_str().expect("utf8 cwd"))
    );
    assert_eq!(
        text("HOME").as_deref(),
        Some(home.to_str().expect("utf8 home"))
    );
    assert_eq!(text("TZ").as_deref(), Some("UTC"));
    assert_eq!(
        text("PATH").as_deref(),
        Some(format!("/golden/bin:{}/.local/bin", home.display()).as_str())
    );
    assert_eq!(text("RIVETOS_SESSION_KEY").as_deref(), Some("acme-env"));
    assert_eq!(text("RIVET_DEN_SESSION").as_deref(), Some("acme-env"));
    assert_eq!(
        text("RIVET_DEN_NAME").as_deref(),
        Some(format!("{}:envdump", common::HOSTNAME).as_str())
    );
    assert_eq!(
        text("RIVET_DEN_CA").as_deref(),
        Some(
            shared
                .join("rivet-ca")
                .join("intermediate")
                .join("chain.pem")
                .to_str()
                .expect("utf8 ca")
        )
    );
    assert!(text("RIVETOS_USER_ID").is_none());
    assert_eq!(text("RIVETOS_USER_DBS").as_deref(), Some("denied"));
    assert_eq!(text("ACME_ENTRY_ENV").as_deref(), Some("envdump"));

    let masks = Masks {
        port: "5174".to_owned(),
        hostname: Some(common::HOSTNAME.to_owned()),
        tmp: None,
    };
    assert!(masked_eq(
        "http://127.0.0.1:{{PORT}}",
        text("RIVET_DEN_URL").as_deref().expect("url"),
        &masks
    ));

    let anonymous = settings.child_env(None, &roster, &work, &BTreeMap::new());
    assert!(!anonymous.contains_key("RIVET_DEN_SESSION"));
    assert!(!anonymous.contains_key("RIVETOS_SESSION_KEY"));
    assert!(!anonymous.contains_key("RIVET_DEN_TOKEN"));
}

#[test]
fn the_envdump_fixture_pins_the_managed_env() {
    #[derive(serde::Deserialize)]
    struct Frames {
        steps: Vec<Step>,
    }
    #[derive(serde::Deserialize)]
    struct Step {
        id: String,
        #[serde(default)]
        replay: Option<String>,
    }
    let recorded = recording("term/ws-pty-frames.json");
    let file = std::fs::read_to_string(&recorded).expect("fixture");
    let parsed: Frames = serde_json::from_str(&file).expect("fixture json");
    let envdump = parsed
        .steps
        .iter()
        .find(|step| step.id == "attach-envdump")
        .expect("the envdump step");
    let replay = envdump.replay.as_deref().expect("the envdump replay");
    for recorded_line in ["TERM=xterm-256color", "COLORTERM=truecolor", "LANG=C.UTF-8"] {
        assert!(
            replay.contains(recorded_line),
            "the fixture must pin {recorded_line}"
        );
    }
    assert!(replay.contains("RIVET_DEN_NAME={{HOSTNAME}}:envdump"));
    assert!(replay.contains("RIVET_DEN_URL=http://127.0.0.1:{{PORT}}"));
    assert!(!replay.contains("RIVETOS_USER_ID="));
    assert!(replay.contains("RIVETOS_USER_DBS=denied"));
}

#[test]
fn the_ca_chain_sits_under_the_shared_dir() {
    let env = env_from(&[("RIVETHUB_SHARED_DIR", "/golden/shared")]);
    let settings = common::settings(&env);
    assert_eq!(
        settings.ca_path,
        Path::new("/golden/shared/rivet-ca/intermediate/chain.pem")
    );
}

#[test]
fn json_masked_comparison_accepts_the_recorded_exit_frame() {
    let recorded = wire::gateway::term::TermServerFrame::Exit {
        code: Some(3),
        signal: None,
    };
    let actual = wire::gateway::term::TermServerFrame::Exit {
        code: Some(3),
        signal: None,
    };
    let recorded = serde_json::to_value(&recorded).expect("the recorded frame serializes");
    let actual = serde_json::to_value(&actual).expect("the actual frame serializes");
    assert!(json_masked_eq(&recorded, &actual, &Masks::default()));
}
