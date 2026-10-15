use std::collections::BTreeSet;
use std::os::unix::fs::symlink;
use std::path::Path;
use std::path::PathBuf;

use terminal::Error;
use terminal::MuxPlan;
use terminal::TerminalManager;
use terminal::TerminalSettings;
use terminal::herdr::namespace;
use terminal::herdr::namespace::HostRoots;
use terminal::herdr::namespace::SessionStamp;
use terminal::herdr::namespace::SkipRule;
use terminal::herdr::paths;
use terminal::mux::HerdrSetup;
use terminal::session::RosterKey;
use terminal::session::SessionKey;

#[derive(Debug, serde::Serialize)]
struct ForeignMarker {
    command: String,
    user: String,
    #[serde(rename = "paneId")]
    pane_id: String,
    created: i64,
    #[serde(rename = "denKey")]
    recorded_key: String,
    pid: u32,
    #[serde(rename = "agentPane")]
    agent_pane: String,
}

fn settings() -> TerminalSettings {
    TerminalSettings::from_config(
        &config::Config::default(),
        &config::Environment::from_vars([("HOME", "/tmp")]),
        "isolation-test".to_owned(),
    )
}

fn stamp(
    namespace_id: &str,
    key: &SessionKey,
    user: &str,
) -> Result<SessionStamp, Box<dyn std::error::Error>> {
    Ok(SessionStamp {
        namespace: namespace_id.to_owned(),
        session_key: key.clone(),
        user: user.to_owned(),
        roster_key: RosterKey::new("shell")?,
        argv: vec!["/bin/true".to_owned()],
    })
}

fn session_dir(config_home: &Path, key: &SessionKey) -> PathBuf {
    paths::sessions_dir(config_home).join(paths::session_dir_name(key))
}

fn plant_stamp(
    config_home: &Path,
    namespace_id: &str,
    key: &str,
    user: &str,
) -> Result<(SessionKey, PathBuf), Box<dyn std::error::Error>> {
    let key = SessionKey::new(key)?;
    let dir = session_dir(config_home, &key);
    std::fs::create_dir_all(&dir)?;
    namespace::write_stamp(&dir, &stamp(namespace_id, &key, user)?)?;
    Ok((key, dir))
}

fn foreign_marker() -> ForeignMarker {
    ForeignMarker {
        command: "grok".to_owned(),
        user: "coco".to_owned(),
        pane_id: "w1:p1".to_owned(),
        created: 1_700_000_000,
        recorded_key: "00000000-0000-4000-8000-000000000099".to_owned(),
        pid: 4242_u32,
        agent_pane: "0".to_owned(),
    }
}

fn write_foreign(dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all(dir)?;
    let bytes = serde_json::to_vec(&foreign_marker())?;
    std::fs::write(dir.join("rivet.json"), bytes)?;
    std::fs::write(dir.join("herdr.sock"), [])?;
    Ok(())
}

struct Neighbours {
    runtime: PathBuf,
    temp: PathBuf,
    runtime_sock: PathBuf,
    temp_sock: PathBuf,
    sibling_sock: PathBuf,
    kept: PathBuf,
}

fn plant_neighbours(
    root: &Path,
    config_home: &Path,
) -> Result<Neighbours, Box<dyn std::error::Error>> {
    let outside = root.join("outside");
    std::fs::create_dir_all(&outside)?;
    let kept = outside.join("kept");
    std::fs::write(&kept, b"keep")?;
    symlink(&outside, paths::sessions_dir(config_home).join("linked"))?;
    let home_name = config_home.file_name().ok_or("config home name")?;
    let sibling = config_home.with_file_name(format!("{}-extra", home_name.to_string_lossy()));
    let sibling_sock = sibling
        .join("herdr")
        .join("sessions")
        .join("d0123456789ab")
        .join("herdr.sock");
    std::fs::create_dir_all(sibling_sock.parent().ok_or("sibling parent")?)?;
    std::fs::write(&sibling_sock, [])?;
    let runtime = root.join("run-user");
    let host_temp = root.join("host-tmp");
    let runtime_sock = runtime
        .join("1000")
        .join(home_name)
        .join("herdr")
        .join("sessions")
        .join("done")
        .join("herdr.sock");
    let temp_sock = host_temp
        .join(home_name)
        .join("herdr")
        .join("sessions")
        .join("dtwo")
        .join("herdr.sock");
    std::fs::create_dir_all(runtime_sock.parent().ok_or("runtime parent")?)?;
    std::fs::create_dir_all(temp_sock.parent().ok_or("temp parent")?)?;
    std::fs::write(&runtime_sock, [])?;
    std::fs::write(&temp_sock, [])?;
    Ok(Neighbours {
        runtime,
        temp: host_temp,
        runtime_sock,
        temp_sock,
        sibling_sock,
        kept,
    })
}

#[test]
fn containment_refuses_sibling_prefix_parent_and_keeps_a_real_child() {
    let root = Path::new("/var/state/run/rivethub-0123abcd/herdr/sessions");
    let child = root.join("d0123456789ab").join("herdr.sock");
    let sibling =
        Path::new("/var/state/run/rivethub-0123abcd-extra/herdr/sessions/d0123456789ab/herdr.sock");
    let parent = Path::new(
        "/var/state/run/rivethub-0123abcd/herdr/sessions/../sessions/d0123456789ab/herdr.sock",
    );
    assert!(namespace::path_is_inside(root, &child));
    assert!(!namespace::path_is_inside(root, sibling));
    assert!(!namespace::path_is_inside(root, parent));
    assert!(!namespace::path_is_inside(root, root));
}

#[test]
fn foreign_sessions_are_skipped_once_and_own_sessions_are_adopted() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let state = tmp.path().join("state");
    std::fs::create_dir_all(&state).expect("state");
    let config_home = paths::resolve_config_home(&state, 5974);
    assert_eq!(
        config_home,
        state
            .join("run")
            .join(paths::config_home_name(&state, 5974))
    );
    assert!(config_home.starts_with(&state));
    let namespace_id = namespace::namespace_id(&config_home).expect("namespace");
    let (owned_key, owned_dir) =
        plant_stamp(&config_home, &namespace_id, "owned-session", "owner").expect("owned stamp");
    std::fs::write(owned_dir.join("herdr.sock"), []).expect("own socket");
    let (_idle_key, idle_dir) =
        plant_stamp(&config_home, &namespace_id, "idle-session", "owner").expect("idle stamp");
    let (_other_key, other_dir) =
        plant_stamp(&config_home, &namespace_id, "other-user", "coco").expect("other stamp");
    std::fs::write(other_dir.join("herdr.sock"), []).expect("other socket");
    write_foreign(&paths::sessions_dir(&config_home).join("dforeignmarker1")).expect("marker");
    write_foreign(&paths::sessions_dir(&config_home).join("dforeignmarker2")).expect("marker");
    let neighbours = plant_neighbours(tmp.path(), &config_home).expect("neighbours");
    let scanned = namespace::scan(
        &state,
        &config_home,
        "owner",
        &HostRoots {
            runtime: &neighbours.runtime,
            temp: &neighbours.temp,
            uid: 1000,
        },
    )
    .expect("scan");
    assert_eq!(
        scanned.skipped,
        BTreeSet::from([
            SkipRule::ForeignHerdrNamespace,
            SkipRule::ForeignSessionOwner
        ])
    );
    assert_eq!(scanned.owned.len(), 1);
    let owned = scanned.owned.first().expect("owned");
    assert_eq!(owned.session, owned_key);
    assert_eq!(scanned.inactive.len(), 1);
    let idle = scanned.inactive.first().expect("idle");
    assert_eq!(idle.as_str(), "idle-session");
    assert!(owned_dir.join("herdr.sock").is_file());
    assert!(other_dir.join("herdr.sock").is_file());
    assert!(neighbours.runtime_sock.is_file());
    assert!(neighbours.temp_sock.is_file());
    assert!(neighbours.sibling_sock.is_file());
    assert_eq!(
        std::fs::read(&neighbours.kept).expect("kept bytes"),
        b"keep"
    );
    assert!(idle_dir.join("rivethub.json").is_file());
}

#[test]
fn a_corrupt_own_stamp_fails_closed_and_leaves_the_directory() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let state = tmp.path().join("state");
    let config_home = paths::resolve_config_home(&state, 5974);
    let dir = paths::sessions_dir(&config_home).join("dcorruptstamp1");
    std::fs::create_dir_all(&dir).expect("dir");
    std::fs::write(dir.join("rivethub.json"), b"{").expect("stamp");
    let error = namespace::scan(
        &state,
        &config_home,
        "owner",
        &HostRoots {
            runtime: tmp.path().join("run").as_path(),
            temp: tmp.path().join("tmp-root").as_path(),
            uid: 1000,
        },
    )
    .expect_err("corrupt stamp");
    assert!(matches!(error, Error::SessionStamp { .. }), "{error}");
    assert_eq!(
        std::fs::read(dir.join("rivethub.json")).expect("bytes"),
        b"{"
    );
}

#[tokio::test]
async fn the_manager_adopts_only_its_namespace_and_a_foreign_socket_survives_kill() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let state = tmp.path().join("state");
    std::fs::create_dir_all(&state).expect("state");
    let config_home = paths::resolve_config_home(&state, 5974);
    let namespace_id = namespace::namespace_id(&config_home).expect("namespace");
    let (_owned_key, owned_dir) =
        plant_stamp(&config_home, &namespace_id, "owned-session", "owner").expect("owned stamp");
    let owned_sock = owned_dir.join("herdr.sock");
    std::fs::write(&owned_sock, []).expect("socket");
    let (_other_key, other_dir) =
        plant_stamp(&config_home, &namespace_id, "owned-other", "owner").expect("other stamp");
    std::fs::write(other_dir.join("herdr.sock"), []).expect("other socket");
    let foreign = tmp.path().join("foreign-socket");
    std::fs::write(&foreign, b"foreign").expect("foreign");
    let manager = TerminalManager::new(
        settings(),
        MuxPlan::Herdr(HerdrSetup {
            binary: PathBuf::from("herdr"),
            config_home: config_home.clone(),
        }),
    );
    let report = manager
        .adopt_namespace(&state, "owner")
        .await
        .expect("adopt");
    assert_eq!(report.adopted.len(), 2);
    assert!(
        report
            .adopted
            .iter()
            .any(|session| session.session.as_str() == "owned-session")
    );
    let listed = manager.list();
    assert_eq!(listed.len(), 2);
    assert!(listed.iter().all(|record| {
        record
            .session
            .as_ref()
            .is_some_and(|session| session.as_str() != "other-user")
    }));
    let released = report
        .adopted
        .iter()
        .find(|session| session.session.as_str() == "owned-other")
        .expect("second")
        .id
        .clone();
    manager.release(&released);
    assert!(other_dir.join("herdr.sock").is_file());
    assert_eq!(manager.list().len(), 1);
    std::fs::remove_file(&owned_sock).expect("remove socket");
    symlink(&foreign, &owned_sock).expect("escape symlink");
    let kept = report
        .adopted
        .iter()
        .find(|session| session.session.as_str() == "owned-session")
        .expect("owned")
        .id
        .clone();
    let error = manager.kill(&kept).await.expect_err("refused");
    assert!(matches!(error, Error::NamespaceEscape { .. }), "{error}");
    assert_eq!(std::fs::read(&foreign).expect("foreign bytes"), b"foreign");
    let missing = manager
        .kill(&kernel::terminal::TerminalId::new("pty-missing"))
        .await
        .expect_err("missing");
    assert!(matches!(missing, Error::UnknownTerminal { .. }));
    assert_eq!(std::fs::read(&foreign).expect("foreign bytes"), b"foreign");
}

#[test]
fn an_ancestor_symlink_cannot_redirect_the_namespace_outside_state()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let state = temp.path().join("state");
    let outside = temp.path().join("outside");
    std::fs::create_dir_all(&state)?;
    std::fs::create_dir_all(&outside)?;
    symlink(&outside, state.join("run"))?;
    let config_home = paths::resolve_config_home(&state, 5174);
    std::fs::create_dir_all(paths::sessions_dir(&config_home))?;
    let hosts = HostRoots {
        runtime: temp.path(),
        temp: temp.path(),
        uid: 1,
    };
    assert!(matches!(
        namespace::scan(&state, &config_home, "owner", &hosts),
        Err(Error::NamespaceEscape { .. })
    ));
    assert!(matches!(
        namespace::scan(&state, &config_home, "", &hosts),
        Err(Error::OwnerUnset)
    ));
    Ok(())
}

#[test]
fn a_stamp_symlink_is_neither_read_nor_overwritten() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let state = temp.path().join("state");
    let home = paths::resolve_config_home(&state, 5174);
    let namespace_id = namespace::namespace_id(&home)?;
    let key = SessionKey::new("claude-code:linked-stamp")?;
    let dir = session_dir(&home, &key);
    std::fs::create_dir_all(&dir)?;
    let outside = temp.path().join("outside.json");
    let stamp = stamp(&namespace_id, &key, "owner")?;
    let bytes = serde_json::to_vec(&stamp)?;
    std::fs::write(&outside, &bytes)?;
    symlink(&outside, dir.join("rivethub.json"))?;
    assert!(matches!(
        namespace::write_stamp(&dir, &stamp),
        Err(Error::Io(_))
    ));
    let hosts = HostRoots {
        runtime: temp.path(),
        temp: temp.path(),
        uid: 1,
    };
    assert!(matches!(
        namespace::scan(&state, &home, "owner", &hosts),
        Err(Error::Io(_))
    ));
    assert_eq!(std::fs::read(&outside)?, bytes);
    Ok(())
}
