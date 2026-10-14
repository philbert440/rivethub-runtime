#![cfg(test)]

use mcp::{Error, transport::bind_unix};
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::time::Duration;

#[tokio::test]
async fn stale_socket_is_replaced_at_mode_0600() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let directory = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).expect("directory");
        let path = directory.path().join("mcp.sock");
        let stale = std::os::unix::net::UnixListener::bind(&path).expect("stale socket");
        drop(stale);
        let listener = bind_unix(&path).await.expect("replace stale socket");
        assert_eq!(
            std::fs::metadata(&path)
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let connection = tokio::net::UnixStream::connect(&path)
            .await
            .expect("replacement listens");
        drop(connection);
        drop(listener);
    })
    .await
    .expect("startup deadline");
}

#[tokio::test]
async fn live_socket_is_refused_without_replacement() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let directory = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).expect("directory");
        let path = directory.path().join("mcp.sock");
        let listener = tokio::net::UnixListener::bind(&path).expect("live socket");
        let original = std::fs::metadata(&path).expect("metadata");
        let error = bind_unix(&path).await.expect_err("refuse live socket");
        assert!(error.to_string().contains(&path.display().to_string()));
        assert!(matches!(error, Error::SocketListening { path: found } if found == path));
        assert_eq!(
            std::fs::metadata(&path).expect("metadata").ino(),
            original.ino()
        );
        let (_stream, _address) = listener.accept().await.expect("liveness probe");
    })
    .await
    .expect("startup deadline");
}

#[tokio::test]
async fn nonsockets_and_symlinks_are_refused_without_removal() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let directory = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).expect("directory");
        let file = directory.path().join("file");
        std::fs::write(&file, "preserve me").expect("file");
        let folder = directory.path().join("folder");
        std::fs::create_dir(&folder).expect("folder");
        let stale = directory.path().join("stale");
        drop(std::os::unix::net::UnixListener::bind(&stale).expect("stale socket"));
        let link = directory.path().join("link");
        symlink(&stale, &link).expect("symlink");
        for path in [&file, &folder, &link] {
            let error = bind_unix(path).await.expect_err("refuse non-socket");
            assert!(error.to_string().contains(&path.display().to_string()));
            assert!(matches!(error, Error::SocketNotSocket { path: found } if found == *path));
            assert!(std::fs::symlink_metadata(path).is_ok());
        }
        assert_eq!(
            std::fs::read_to_string(&file).expect("preserved content"),
            "preserve me"
        );
        assert_eq!(std::fs::read_link(&link).expect("preserved link"), stale);
        assert!(std::fs::metadata(stale).is_ok());
    })
    .await
    .expect("startup deadline");
}
