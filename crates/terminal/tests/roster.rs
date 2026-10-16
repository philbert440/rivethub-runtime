#[expect(
    dead_code,
    reason = "the harness module is included whole by every test binary and this one uses a subset"
)]
mod common;

use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;

use kernel::terminal::TerminalMux;
use wire::gateway::roster::RosterEntry;
use wire::gateway::roster::RosterFile;
use wire::gateway::term::TermConfig;
use wire::harness::HarnessId;

const RECORDED_DISABLED: &str = concat!(
    r#"{"enabled":false,"default":"claude","maxPtys":4,"active":0,"commands":["#,
    r#"{"id":"claude","label":"Claude Code","room":true},"#,
    r#"{"id":"grok","label":"Grok Build","room":true},"#,
    r#"{"id":"hermes","label":"Hermes","room":true},"#,
    r#"{"id":"kimi","label":"Kimi Code","room":true},"#,
    r#"{"id":"codex","label":"Codex","room":true},"#,
    r#"{"id":"opencode","label":"OpenCode","room":true},"#,
    r#"{"id":"pi","label":"Pi","room":true},"#,
    r#"{"id":"qwen","label":"Qwen Code","room":true},"#,
    r#"{"id":"cursor","label":"Cursor","room":true},"#,
    r#"{"id":"shell","label":"Shell","room":false}]}"#
);

const OUT_OF_SLICE: &[(&str, &str)] = &[
    (
        "proto-key",
        "JavaScript prototype keys are ordinary roster ids; filtering them as inherited properties is a TypeScript artefact",
    ),
    (
        "only-proto",
        "a roster whose only key is __proto__ is a JavaScript prototype artefact, not an empty command map",
    ),
    (
        "proto-default",
        "a default of toString names a JavaScript prototype method, not a roster command",
    ),
    (
        "malformed-entry",
        "an empty argv is a typed startup error; substituting the built-in roster would hide a broken file",
    ),
    (
        "not-json",
        "a roster that is not JSON is a typed startup error; substituting the built-in roster would hide a broken file",
    ),
    (
        "opencode-legacy-cwd",
        "the legacy opencode working directory under the previous state home is not a roster rule",
    ),
    (
        "lazy-mtime-reload",
        "the roster is read when terminals start; reload on mtime is outside this slice",
    ),
];

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/roster")
        .join(name)
}

async fn load_fixture(name: &str) -> Result<terminal::LoadedRoster, terminal::Error> {
    terminal::load_roster(&fixture(name), Path::new("/roster-fixture/fallback")).await
}

fn command<'a>(
    loaded: &'a terminal::LoadedRoster,
    id: &str,
) -> TestResult<&'a terminal::ResolvedCommand> {
    loaded
        .commands
        .iter()
        .find(|command| command.id == id)
        .ok_or_else(|| format!("missing command {id}").into())
}

fn env_value<'a>(command: &'a terminal::ResolvedCommand, name: &str) -> Option<&'a str> {
    command.env.get(name).map(String::as_str)
}

#[test]
fn disabled_config_keeps_the_recorded_builtin_body() {
    let body = serde_json::to_string(&TermConfig::disabled(4)).expect("json");
    assert_eq!(body, RECORDED_DISABLED);
}

#[test]
fn out_of_slice_roster_recordings_are_named_and_reasoned() {
    let mut seen = std::collections::BTreeSet::new();
    for (name, reason) in OUT_OF_SLICE {
        assert!(seen.insert(*name), "duplicate skip {name}");
        assert!(reason.len() > 20, "{name} needs a reason");
    }
    assert_eq!(seen.len(), OUT_OF_SLICE.len());
}

#[test]
fn roster_path_prefers_the_file_then_the_current_name_then_the_state_file() {
    let state = Path::new("/state");
    let yaml = Path::new("/yaml/commands.json");
    let current = config::Environment::from_vars([
        ("RIVETHUB_TERM_CONFIG", "/current/den-term.json"),
        ("RIVETOS_DEN_TERM_CONFIG", "/legacy/den-term.json"),
    ]);
    assert_eq!(
        terminal::resolve_roster_path(Some(yaml), &current, state),
        yaml
    );
    assert_eq!(
        terminal::resolve_roster_path(None, &current, state),
        PathBuf::from("/current/den-term.json")
    );
    assert!(
        current
            .legacy_notice("RIVETHUB_TERM_CONFIG", "RIVETOS_DEN_TERM_CONFIG")
            .is_none()
    );
    let legacy =
        config::Environment::from_vars([("RIVETOS_DEN_TERM_CONFIG", "/legacy/den-term.json")]);
    assert_eq!(
        terminal::resolve_roster_path(None, &legacy, state),
        PathBuf::from("/legacy/den-term.json")
    );
    let notices: Vec<_> = legacy
        .legacy_notices()
        .into_iter()
        .filter(|notice| notice.contains("RIVETOS_DEN_TERM_CONFIG"))
        .collect();
    assert_eq!(notices.len(), 1);
    let notice = notices.first().expect("legacy notice");
    assert!(notice.contains("RIVETHUB_TERM_CONFIG"), "{notice}");
    let blank = config::Environment::from_vars([
        ("RIVETHUB_TERM_CONFIG", "   "),
        ("RIVETOS_DEN_TERM_CONFIG", "/legacy/den-term.json"),
    ]);
    assert_eq!(
        terminal::resolve_roster_path(None, &blank, state),
        PathBuf::from("/legacy/den-term.json")
    );
    let absent = config::Environment::from_vars([("HOME", "/home/acme")]);
    assert_eq!(
        terminal::resolve_roster_path(None, &absent, state),
        state.join("den-term.json")
    );
}

#[tokio::test]
async fn mesh_roster_uses_the_top_level_cwd_env_and_key_harness() -> TestResult {
    let loaded = load_fixture("mesh.json").await?;
    assert_eq!(loaded.default_id, "pi");
    let pi = command(&loaded, "pi")?;
    assert_eq!(pi.label, "Pi");
    assert_eq!(pi.argv, ["/home/rivet/.local/bin/pi"]);
    assert_eq!(pi.cwd, Path::new("/roster-fixture/fallback"));
    assert_eq!(pi.harness, Some(HarnessId::Pi));
    assert!(pi.session);
    assert_eq!(
        env_value(pi, "PATH"),
        Some("/usr/bin:/home/rivet/.local/bin")
    );
    let shell = command(&loaded, "shell")?;
    assert_eq!(shell.argv, ["bash", "-l"]);
    assert_eq!(shell.cwd, Path::new("/home/rivet"));
    assert_eq!(shell.harness, None);
    assert!(!shell.session);
    assert_eq!(shell.env, pi.env);
    Ok(())
}

#[tokio::test]
async fn layered_roster_overlays_entry_env_and_cwd() -> TestResult {
    let loaded = load_fixture("layered.json").await?;
    assert_eq!(loaded.default_id, "echo");
    let echo = command(&loaded, "echo")?;
    assert_eq!(echo.cwd, Path::new("/roster-fixture/fallback"));
    assert_eq!(echo.harness, None);
    assert!(echo.session);
    assert_eq!(env_value(echo, "ACME_ROSTER_ENV"), Some("roster"));
    assert_eq!(env_value(echo, "SHARED"), Some("top"));
    let shell = command(&loaded, "shell")?;
    assert!(!shell.session);
    assert_eq!(shell.harness, None);
    assert_eq!(shell.cwd, Path::new("/roster-fixture/top"));
    assert_eq!(env_value(shell, "ACME_ROSTER_ENV"), Some("roster"));
    assert_eq!(env_value(shell, "ACME_ENTRY_ENV"), Some("shell"));
    assert_eq!(env_value(shell, "SHARED"), Some("top"));
    let envdump = command(&loaded, "envdump")?;
    assert!(!envdump.session);
    assert_eq!(envdump.cwd, Path::new("/roster-fixture/entry"));
    assert_eq!(env_value(envdump, "ACME_ROSTER_ENV"), Some("roster"));
    assert_eq!(env_value(envdump, "ACME_ENTRY_ENV"), Some("envdump"));
    assert_eq!(env_value(envdump, "SHARED"), Some("entry"));
    assert_eq!(env_value(envdump, "RIVETOS_USER_ID"), Some("from-roster"));
    assert_eq!(env_value(envdump, "RIVETOS_USER_DBS"), Some("denied"));
    assert_eq!(env_value(envdump, "PATH"), Some("/usr/bin"));
    let claude = command(&loaded, "claude")?;
    assert_eq!(claude.argv, ["node", "claude.js"]);
    assert_eq!(claude.harness, Some(HarnessId::ClaudeCode));
    assert!(claude.session);
    assert_eq!(claude.cwd, Path::new("/roster-fixture/fallback"));
    let silent = command(&loaded, "silent")?;
    assert!(!silent.session);
    assert_eq!(silent.harness, None);
    assert_eq!(silent.cwd, Path::new("/roster-fixture/top"));
    assert_eq!(silent.env, echo.env);
    Ok(())
}

#[tokio::test]
async fn a_prototype_key_is_an_ordinary_command() -> TestResult {
    let loaded = load_fixture("proto.json").await?;
    assert_eq!(loaded.default_id, "__proto__");
    let command = command(&loaded, "__proto__")?;
    assert_eq!(command.label, "Proto");
    assert!(command.session);
    assert_eq!(command.harness, None);
    assert_eq!(command.argv, ["/bin/true"]);
    Ok(())
}

#[tokio::test]
async fn broken_roster_files_refuse_startup() -> TestResult {
    let empty = load_fixture("empty-argv.json").await.expect_err("empty");
    assert!(matches!(
        empty,
        terminal::Error::Roster(terminal::RosterError::EmptyArgv { .. })
    ));
    let relative = load_fixture("relative-cwd.json")
        .await
        .expect_err("relative");
    assert!(matches!(
        relative,
        terminal::Error::Roster(terminal::RosterError::RelativeCwd(_))
    ));
    let entry = load_fixture("entry-relative.json")
        .await
        .expect_err("entry");
    assert!(matches!(
        entry,
        terminal::Error::Roster(terminal::RosterError::RelativeCwd(_))
    ));
    let unknown = load_fixture("unknown-default.json")
        .await
        .expect_err("default");
    assert!(matches!(
        unknown,
        terminal::Error::Roster(terminal::RosterError::UnknownDefault { .. })
    ));
    let key = load_fixture("bad-key.json").await.expect_err("key");
    assert!(matches!(
        key,
        terminal::Error::Roster(terminal::RosterError::InvalidKey(_))
    ));
    for name in [
        "not-json.json",
        "missing-default.json",
        "invented-schema.json",
    ] {
        let error = load_fixture(name).await.expect_err(name);
        assert!(
            matches!(
                error,
                terminal::Error::Roster(terminal::RosterError::Json { .. })
            ),
            "{name}: {error}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn a_missing_file_is_the_builtin_roster() -> TestResult {
    let root = tempfile::tempdir()?;
    let loaded = terminal::load_roster(&root.path().join("den-term.json"), root.path()).await?;
    let disabled = TermConfig::disabled(4);
    assert_eq!(loaded.default_id, disabled.default);
    assert_eq!(loaded.commands.len(), disabled.commands.len());
    for (loaded_command, recorded) in loaded.commands.iter().zip(&disabled.commands) {
        assert_eq!(loaded_command.id, recorded.id);
        assert_eq!(loaded_command.label, recorded.label);
        assert_eq!(loaded_command.session, recorded.session);
        assert_eq!(loaded_command.cwd, root.path());
        assert!(loaded_command.env.is_empty());
    }
    assert_eq!(
        command(&loaded, "grok")?.argv,
        ["grok", "--permission-mode", "bypassPermissions"]
    );
    assert_eq!(
        command(&loaded, "hermes")?.argv,
        ["hermes", "--yolo", "--accept-hooks"]
    );
    assert_eq!(command(&loaded, "kimi")?.argv, ["kimi", "--yolo"]);
    assert_eq!(
        command(&loaded, "qwen")?.argv,
        ["qwen", "--approval-mode", "yolo"]
    );
    assert_eq!(
        command(&loaded, "cursor")?.argv,
        ["agent", "--force", "--trust"]
    );
    assert_eq!(command(&loaded, "shell")?.argv, ["bash", "-l"]);
    assert_eq!(
        command(&loaded, "claude")?.harness,
        Some(HarnessId::ClaudeCode)
    );
    assert_eq!(command(&loaded, "shell")?.harness, None);
    let relative = terminal::load_roster(&root.path().join("absent.json"), Path::new("relative"))
        .await
        .expect_err("relative fallback");
    assert!(matches!(
        relative,
        terminal::Error::Roster(terminal::RosterError::RelativeCwd(_))
    ));
    Ok(())
}

#[tokio::test]
async fn a_directory_and_a_dangling_link_are_not_a_quiet_builtin() -> TestResult {
    let root = tempfile::tempdir()?;
    let directory = root.path().join("den-term.json");
    std::fs::create_dir(&directory)?;
    let not_file = terminal::load_roster(&directory, root.path())
        .await
        .expect_err("directory");
    assert!(matches!(
        not_file,
        terminal::Error::Roster(terminal::RosterError::NotAFile(_))
    ));
    let link = root.path().join("link.json");
    std::os::unix::fs::symlink(root.path().join("missing.json"), &link)?;
    let dangling = terminal::load_roster(&link, root.path())
        .await
        .expect_err("dangling");
    assert!(matches!(dangling, terminal::Error::Io(_)), "{dangling}");
    Ok(())
}

#[tokio::test]
async fn the_default_path_follows_the_state_move() -> TestResult {
    let home = tempfile::tempdir()?;
    let work = home.path().join("work");
    std::fs::create_dir_all(&work)?;
    let legacy = home.path().join(".rivetos");
    std::fs::create_dir_all(&legacy)?;
    write_shell_roster(&legacy.join("den-term.json"), &work)?;
    let home_text = home.path().to_str().ok_or("home utf-8")?;
    let env = config::Environment::from_vars([("HOME", home_text)]);
    let moved = config::adopt_legacy_state(&env)?;
    assert!(moved.is_some(), "the legacy state directory moves once");
    let state = config::Paths::new(&env)?.state_dir();
    assert!(state.ends_with(".rivethub"));
    assert!(!legacy.exists());
    let path = terminal::resolve_roster_path(None, &env, &state);
    assert_eq!(path, state.join("den-term.json"));
    assert!(path.is_file());
    let loaded = terminal::load_roster(&path, &work).await?;
    assert_eq!(loaded.default_id, "shell");
    let shell = command(&loaded, "shell")?;
    assert_eq!(shell.cwd, work);
    assert_eq!(shell.argv, ["/bin/true"]);
    assert!(!shell.session);
    Ok(())
}

fn write_shell_roster(path: &Path, cwd: &Path) -> TestResult {
    let file = RosterFile {
        default: "shell".to_owned(),
        cwd: Some(cwd.to_path_buf()),
        env: BTreeMap::new(),
        commands: BTreeMap::from([(
            "shell".to_owned(),
            RosterEntry {
                label: "Shell".to_owned(),
                argv: vec!["/bin/true".to_owned()],
                session: Some(false),
                cwd: None,
                env: BTreeMap::new(),
            },
        )]),
    };
    std::fs::write(path, serde_json::to_vec(&file)?)?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn roster_env_reaches_the_spawned_pty() -> TestResult {
    let home = tempfile::tempdir()?;
    let work = home.path().join("work");
    std::fs::create_dir_all(&work)?;
    let roster_path = home.path().join("den-term.json");
    write_env_roster(&roster_path, &work)?;
    let loaded = terminal::load_roster(&roster_path, &work).await?;
    let envdump = command(&loaded, "envdump")?;
    assert_eq!(envdump.cwd, work);
    assert_eq!(env_value(envdump, "SHARED"), Some("entry"));
    assert_eq!(env_value(envdump, "ACME_ROSTER_ENV"), Some("roster"));
    let echo = command(&loaded, "echo")?;
    assert_eq!(echo.cwd, work);
    assert_eq!(env_value(echo, "SHARED"), Some("top"));
    let home_text = home.path().to_str().ok_or("home utf-8")?;
    let text = spawn_envdump(&loaded, home_text).await?;
    assert_pty_line(&text, "ACME_ROSTER_ENV=roster");
    assert_pty_line(&text, "ACME_ENTRY_ENV=envdump");
    assert_pty_line(&text, "SHARED=entry");
    assert_pty_line(&text, "RIVETOS_USER_ID=");
    assert_pty_line(&text, "RIVETOS_USER_DBS=denied");
    assert_pty_line(&text, &format!("PATH=/usr/bin:{home_text}/.local/bin"));
    assert!(!text.contains("from-roster"), "{text}");
    assert!(!text.contains("from-process"), "{text}");
    Ok(())
}

fn assert_pty_line(text: &str, line: &str) {
    let expected = format!("{line}\r\n");
    assert!(text.contains(&expected), "missing {expected:?} in {text:?}");
}

async fn spawn_envdump(loaded: &terminal::LoadedRoster, home: &str) -> TestResult<String> {
    let envdump = command(loaded, "envdump")?;
    let manager = common::direct_manager(&common::env_from(&[
        ("HOME", home),
        ("PATH", "/process/bin"),
        ("RIVETOS_USER_ID", "from-process"),
    ]));
    let session = wire::session_id::SessionId::new(HarnessId::Pi, "envdump");
    let id = manager
        .spawn(
            &session,
            envdump.argv.clone(),
            &envdump.cwd,
            &envdump.env,
            kernel::terminal::TermSize { cols: 80, rows: 24 },
        )
        .await?;
    let bytes = common::until_output(manager.attach_stream(&id)?, b"/.local/bin\r\n").await?;
    Ok(String::from_utf8(bytes)?)
}

fn write_env_roster(path: &Path, cwd: &Path) -> TestResult {
    let script = "printf 'ACME_ROSTER_ENV=%s\nACME_ENTRY_ENV=%s\nSHARED=%s\nRIVETOS_USER_ID=%s\nRIVETOS_USER_DBS=%s\nPATH=%s\n' \"$ACME_ROSTER_ENV\" \"$ACME_ENTRY_ENV\" \"$SHARED\" \"$RIVETOS_USER_ID\" \"$RIVETOS_USER_DBS\" \"$PATH\"";
    let mut top = BTreeMap::new();
    top.insert("ACME_ROSTER_ENV".to_owned(), "roster".to_owned());
    top.insert("SHARED".to_owned(), "top".to_owned());
    top.insert("PATH".to_owned(), "/usr/bin".to_owned());
    let mut entry = BTreeMap::new();
    entry.insert("ACME_ENTRY_ENV".to_owned(), "envdump".to_owned());
    entry.insert("SHARED".to_owned(), "entry".to_owned());
    entry.insert("RIVETOS_USER_ID".to_owned(), "from-roster".to_owned());
    entry.insert("RIVETOS_USER_DBS".to_owned(), "denied".to_owned());
    let file = RosterFile {
        default: "envdump".to_owned(),
        cwd: Some(cwd.to_path_buf()),
        env: top,
        commands: BTreeMap::from([
            (
                "echo".to_owned(),
                RosterEntry {
                    label: "Echo".to_owned(),
                    argv: vec!["/bin/true".to_owned()],
                    session: Some(true),
                    cwd: None,
                    env: BTreeMap::new(),
                },
            ),
            (
                "envdump".to_owned(),
                RosterEntry {
                    label: "Env".to_owned(),
                    argv: vec!["/bin/sh".to_owned(), "-c".to_owned(), script.to_owned()],
                    session: Some(false),
                    cwd: None,
                    env: entry,
                },
            ),
        ]),
    };
    std::fs::write(path, serde_json::to_vec(&file)?)?;
    Ok(())
}

#[tokio::test]
async fn harness_entries_start_in_home_except_opencode_with_its_own_cwd() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("den-term.json");
    std::fs::write(
        &path,
        r#"{"default":"claude","cwd":"/roster-fixture/top","commands":{
            "claude":{"label":"Claude","cmd":["claude"],"room":true,"cwd":"/ignored"},
            "opencode":{"label":"OpenCode","cmd":["opencode"],"room":true,"cwd":"/roster-fixture/project"},
            "shell":{"label":"Shell","cmd":["bash","-l"],"room":false}}}"#,
    )?;
    let loaded = terminal::load_roster(&path, Path::new("/roster-fixture/home")).await?;
    assert_eq!(
        command(&loaded, "claude")?.cwd,
        Path::new("/roster-fixture/home")
    );
    assert_eq!(
        command(&loaded, "opencode")?.cwd,
        Path::new("/roster-fixture/project")
    );
    assert_eq!(
        command(&loaded, "shell")?.cwd,
        Path::new("/roster-fixture/top")
    );
    Ok(())
}
