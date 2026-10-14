use std::fs;
use std::path::{Path, PathBuf};

use config::Environment;

fn temp_home(label: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "rivethub-state-{label}-{}-{nanos}",
        std::process::id()
    ));
    fs::create_dir_all(&path)?;
    Ok(path)
}

fn env_for(home: &Path) -> Result<Environment, Box<dyn std::error::Error>> {
    let text = home.to_str().ok_or("temp home is not utf-8")?;
    Ok(Environment::from_vars([(
        "HOME".to_owned(),
        text.to_owned(),
    )]))
}

#[test]
fn legacy_state_moves_once_and_later_reads_use_the_new_home()
-> Result<(), Box<dyn std::error::Error>> {
    let home = temp_home("move")?;
    let outcome = legacy_state_moves(&home);
    fs::remove_dir_all(&home)?;
    outcome
}

fn legacy_state_moves(home: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let legacy = home.join(".rivetos");
    fs::create_dir_all(legacy.join("nested"))?;
    fs::write(legacy.join("config.yaml"), "runtime: {}\n")?;
    fs::write(legacy.join("nested").join("marker"), "kept")?;

    let moved = config::adopt_legacy_state(&env_for(home)?)?;
    let notice = moved.ok_or("the first start moves the directory")?;
    assert!(notice.contains(".rivetos"), "{notice}");
    assert!(notice.contains(".rivethub"), "{notice}");
    assert!(home.join(".rivethub/config.yaml").is_file());
    assert_eq!(
        fs::read_to_string(home.join(".rivethub/nested/marker"))?,
        "kept"
    );
    assert!(!legacy.exists());

    let again = config::adopt_legacy_state(&env_for(home)?)?;
    assert!(again.is_none(), "the move happens once: {again:?}");

    let location = config::resolve_config_path(&env_for(home)?).ok_or("config")?;
    assert!(location.path().ends_with(".rivethub/config.yaml"));
    assert!(!location.is_legacy());
    Ok(())
}

#[test]
fn an_existing_rivethub_directory_is_left_in_place() -> Result<(), Box<dyn std::error::Error>> {
    let home = temp_home("both")?;
    let outcome = both_directories_stay(&home);
    fs::remove_dir_all(&home)?;
    outcome
}

fn both_directories_stay(home: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let legacy = home.join(".rivetos");
    let hub = home.join(".rivethub");
    fs::create_dir_all(&legacy)?;
    fs::create_dir_all(&hub)?;
    fs::write(legacy.join("old"), "old")?;
    fs::write(hub.join("new"), "new")?;

    let moved = config::adopt_legacy_state(&env_for(home)?)?;
    assert!(moved.is_none());
    assert_eq!(fs::read_to_string(legacy.join("old"))?, "old");
    assert_eq!(fs::read_to_string(hub.join("new"))?, "new");
    Ok(())
}

#[test]
fn a_legacy_file_is_not_a_state_directory() -> Result<(), Box<dyn std::error::Error>> {
    let home = temp_home("file")?;
    let outcome = legacy_file_stays(&home);
    fs::remove_dir_all(&home)?;
    outcome
}

fn legacy_file_stays(home: &Path) -> Result<(), Box<dyn std::error::Error>> {
    fs::write(home.join(".rivetos"), "not a directory")?;
    let moved = config::adopt_legacy_state(&env_for(home)?)?;
    assert!(moved.is_none());
    assert!(home.join(".rivetos").is_file());
    assert!(!home.join(".rivethub").exists());
    Ok(())
}

#[test]
fn a_missing_legacy_directory_stays_missing() -> Result<(), Box<dyn std::error::Error>> {
    let home = temp_home("absent")?;
    let outcome = missing_legacy_stays(&home);
    fs::remove_dir_all(&home)?;
    outcome
}

fn missing_legacy_stays(home: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let moved = config::adopt_legacy_state(&env_for(home)?)?;
    assert!(moved.is_none());
    assert!(!home.join(".rivethub").exists());
    assert!(!home.join(".rivetos").exists());
    Ok(())
}

#[test]
fn a_live_legacy_owner_refuses_cutover_without_renaming() -> Result<(), Box<dyn std::error::Error>>
{
    let home = temp_home("live")?;
    let legacy = home.join(".rivetos");
    fs::create_dir_all(&legacy)?;
    let pid = std::process::id();
    fs::write(legacy.join("rivetos.pid"), pid.to_string())?;
    fs::write(legacy.join("marker"), "kept")?;
    let result = config::adopt_legacy_state(&env_for(&home)?);
    assert!(
        matches!(result, Err(config::StateError::LiveOwner { pid: found, .. }) if found == pid)
    );
    assert!(!home.join(".rivethub").exists());
    assert_eq!(fs::read_to_string(legacy.join("marker"))?, "kept");
    fs::remove_dir_all(home)?;
    Ok(())
}

#[test]
fn a_held_owner_lock_refuses_cutover() -> Result<(), Box<dyn std::error::Error>> {
    let home = temp_home("locked")?;
    let legacy = home.join(".rivetos");
    fs::create_dir_all(&legacy)?;
    let lock = fs::File::create(legacy.join("rivetos-owner.lock"))?;
    lock.lock()?;
    let result = config::adopt_legacy_state(&env_for(&home)?);
    assert!(matches!(result, Err(config::StateError::Locked { .. })));
    assert!(legacy.is_dir());
    assert!(!home.join(".rivethub").exists());
    drop(lock);
    config::adopt_legacy_state(&env_for(&home)?)?;
    assert!(home.join(".rivethub").is_dir());
    fs::remove_dir_all(home)?;
    Ok(())
}

#[test]
fn malformed_owner_state_fails_closed() -> Result<(), Box<dyn std::error::Error>> {
    let home = temp_home("malformed")?;
    let legacy = home.join(".rivetos");
    fs::create_dir_all(&legacy)?;
    for pid in ["", "0", "-1", "not a PID", "4294967295"] {
        fs::write(legacy.join("gateway.pid"), pid)?;
        let result = config::adopt_legacy_state(&env_for(&home)?);
        assert!(matches!(
            result,
            Err(config::StateError::InvalidOwner { .. })
        ));
        assert!(legacy.is_dir());
        assert!(!home.join(".rivethub").exists());
    }
    fs::write(legacy.join("gateway.pid"), format!("1{}", " ".repeat(1024)))?;
    assert!(matches!(
        config::adopt_legacy_state(&env_for(&home)?),
        Err(config::StateError::InvalidOwner { .. })
    ));
    fs::remove_dir_all(home)?;
    Ok(())
}

#[test]
fn owner_file_symlinks_cannot_escape_the_state_directory() -> Result<(), Box<dyn std::error::Error>>
{
    let home = temp_home("symlink")?;
    let legacy = home.join(".rivetos");
    fs::create_dir(&legacy)?;
    let sibling = home.join("sibling.pid");
    fs::write(&sibling, "1")?;
    std::os::unix::fs::symlink(&sibling, legacy.join("gateway.pid"))?;
    assert!(matches!(
        config::adopt_legacy_state(&env_for(&home)?),
        Err(config::StateError::Inspect { .. })
    ));
    assert!(legacy.is_dir());
    assert!(!home.join(".rivethub").exists());
    assert_eq!(fs::read_to_string(sibling)?, "1");
    fs::remove_dir_all(home)?;
    Ok(())
}
