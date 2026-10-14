use std::path::Path;
use std::path::PathBuf;

use sha2::Digest;
use sha2::Sha256;

use crate::error::Error;
use crate::session::SessionKey;

pub const SUN_PATH_MAX: usize = 107;

#[must_use]
pub fn hash8(state_dir: &Path, port: u16) -> String {
    let mut hasher = Sha256::new();
    hasher.update(state_dir.to_string_lossy().as_bytes());
    hasher.update(b":");
    hasher.update(port.to_string().as_bytes());
    let digest = hasher.finalize();
    let prefix: Vec<u8> = digest.iter().take(4).copied().collect();
    hex::encode(prefix)
}

#[must_use]
pub fn session_dir_name(key: &SessionKey) -> String {
    let mut hasher = Sha256::new();
    hasher.update(key.as_str().as_bytes());
    let digest = hasher.finalize();
    let prefix: Vec<u8> = digest.iter().take(6).copied().collect();
    format!("d{}", hex::encode(prefix))
}

#[must_use]
pub fn config_home_name(state_dir: &Path, port: u16) -> String {
    format!("rivethub-{}", hash8(state_dir, port))
}

#[must_use]
pub fn resolve_config_home(state_dir: &Path, port: u16) -> PathBuf {
    state_dir
        .join("run")
        .join(config_home_name(state_dir, port))
}

#[must_use]
pub fn sessions_dir(config_home: &Path) -> PathBuf {
    config_home.join("herdr").join("sessions")
}

pub fn session_socket(config_home: &Path, session: &str) -> Result<PathBuf, Error> {
    assert_sun_path(
        &config_home
            .join("herdr")
            .join("sessions")
            .join(session)
            .join("herdr.sock"),
    )
}

pub fn client_socket(config_home: &Path) -> Result<PathBuf, Error> {
    assert_sun_path(&config_home.join("herdr-client.sock"))
}

pub fn assert_sun_path(path: &Path) -> Result<PathBuf, Error> {
    let len = path.as_os_str().len();
    if len > SUN_PATH_MAX {
        return Err(Error::SocketPathTooLong {
            len,
            path: Box::from(path),
        });
    }
    Ok(path.to_path_buf())
}

pub async fn wait_for_socket(
    socket: &Path,
    timeout: std::time::Duration,
    session: &str,
) -> Result<(), Error> {
    let timeout_ms = duration_ms(timeout);
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if socket.exists() {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(Error::HerdrServerTimeout {
                socket: Box::from(socket),
                timeout_ms,
                session: session.to_owned(),
            });
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

#[must_use]
pub fn duration_ms(timeout: std::time::Duration) -> u64 {
    timeout.as_secs() * 1000 + u64::from(timeout.subsec_millis())
}

#[cfg(test)]
mod tests {
    use super::assert_sun_path;
    use super::client_socket;
    use super::config_home_name;
    use super::hash8;
    use super::resolve_config_home;
    use super::session_dir_name;
    use super::session_socket;
    use crate::error::Error;
    use crate::session::SessionKey;

    fn long_dir() -> std::path::PathBuf {
        let mut path = std::path::PathBuf::from("/tmp");
        for _ in 0..24 {
            path.push("state-directory-segment-0123456789");
        }
        path
    }

    #[test]
    fn config_home_uses_the_recorded_hash_shape() {
        assert_eq!(
            config_home_name(std::path::Path::new("/home/user/.rivethub"), 5174),
            format!(
                "rivethub-{}",
                hash8(std::path::Path::new("/home/user/.rivethub"), 5174)
            ),
        );
        assert_eq!(
            hash8(std::path::Path::new("/home/user/.rivethub"), 5174).len(),
            8
        );
    }

    #[test]
    fn config_home_stays_under_the_state_root_when_a_host_runtime_dir_exists() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let state = tmp.path().join("state");
        let name = config_home_name(&state, 5174);
        let runtime_root = tmp.path().join("run-user");
        std::fs::create_dir_all(runtime_root.join("1000").join(&name)).expect("mkdir");
        std::fs::create_dir_all(tmp.path().join("tmp").join(&name)).expect("mkdir");
        let resolved = resolve_config_home(&state, 5174);
        assert_eq!(resolved, state.join("run").join(name));
        assert!(resolved.starts_with(&state));
        assert_ne!(
            resolved,
            runtime_root
                .join("1000")
                .join(config_home_name(&state, 5174))
        );
    }

    #[test]
    fn session_name_is_the_hashed_d_prefix() {
        let key = SessionKey::new("claude-code:00000000-0000-4000-8000-000000000001").expect("key");
        let name = session_dir_name(&key);
        assert_eq!(name.len(), 13);
        assert!(name.starts_with('d'));
    }

    #[test]
    fn socket_paths_sit_under_the_config_home() {
        let home = std::path::Path::new("/run/user/1000/rivethub-0123abcd");
        let socket = session_socket(home, "d0123456789ab").expect("socket");
        assert_eq!(
            socket,
            std::path::PathBuf::from(
                "/run/user/1000/rivethub-0123abcd/herdr/sessions/d0123456789ab/herdr.sock"
            )
        );
        let client = client_socket(home).expect("socket");
        assert_eq!(
            client,
            std::path::PathBuf::from("/run/user/1000/rivethub-0123abcd/herdr-client.sock")
        );
    }

    #[test]
    fn overlong_socket_paths_are_a_typed_error() {
        let error = session_socket(&long_dir(), "d0123456789ab").expect_err("too long");
        let Error::SocketPathTooLong { len, .. } = error else {
            panic!("unexpected error: {error}");
        };
        assert!(len > 107);
    }

    #[test]
    fn the_sun_path_limit_is_107_bytes() {
        let mut path = std::path::PathBuf::from("/tmp");
        path.push("a".repeat(101));
        assert_eq!(path.as_os_str().len(), 106);
        assert!(assert_sun_path(&path).is_ok());
        path.push("b");
        assert_eq!(path.as_os_str().len(), 108);
        assert!(assert_sun_path(&path).is_err());
    }
}
