#[expect(
    dead_code,
    reason = "shared CA fixture exposes helpers used by the other integration targets"
)]
mod support;

use kernel::ca::{CertificateAuthority, SignAuthority};
use pki::{ImportLayout, Pki};
use rustls_pki_types::pem::PemObject;
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use support::{Fixture, Result, csr, node};
use tokio_util::sync::CancellationToken;

fn snapshot(path: &Path) -> Result<BTreeMap<PathBuf, Vec<u8>>> {
    let mut result = BTreeMap::new();
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            result.extend(snapshot(&entry.path())?);
        } else {
            result.insert(entry.path(), fs::read(entry.path())?);
        }
    }
    Ok(result)
}

#[tokio::test]
async fn import_once_keys_stay_in_state_and_source_is_unchanged() -> Result {
    let fixture = Fixture::new()?;
    let before = snapshot(&fixture.source)?;
    let ca = fixture.import().await?;
    let token = ca.issue_token(node()).await?;
    ca.sign(
        csr(&["spiffe://rivet.mesh/node/node-a"])?,
        SignAuthority::Token(token),
    )
    .await?;
    ca.revoke(node()).await?;
    ca.crl().await?;
    assert_eq!(before, snapshot(&fixture.source)?);
    let files = snapshot(fixture.directory.path())?;
    for (path, bytes) in files {
        if !before.contains_key(&path) {
            assert!(path.starts_with(&fixture.state));
            if bytes.windows(11).any(|chunk| chunk == b"PRIVATE KEY") {
                assert_eq!(path, fixture.state.join("intermediate.key.pem"));
            }
        }
    }
    assert_eq!(
        fs::metadata(fixture.state.join("intermediate.key.pem"))?
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert!(
        Pki::open(fixture.state.clone(), CancellationToken::new())
            .await
            .is_err()
    );
    drop(ca);
    assert!(fixture.import().await.is_err());
    Pki::open(fixture.state.clone(), CancellationToken::new()).await?;
    Ok(())
}

#[tokio::test]
async fn import_refuses_parent_paths_shared_prefix_siblings_and_symlinks() -> Result {
    let fixture = Fixture::new()?;
    let sibling = fixture.directory.path().join("legacy-sibling");
    fs::create_dir(&sibling)?;
    fs::write(
        sibling.join("root.pem"),
        fs::read(fixture.source.join("root/certs/ca.cert.pem"))?,
    )?;
    for (index, root_path) in [
        PathBuf::from("../legacy-sibling/root.pem"),
        sibling.join("root.pem"),
    ]
    .into_iter()
    .enumerate()
    {
        let layout = ImportLayout {
            root_certificate: root_path,
            ..ImportLayout::openssl()
        };
        let state = fixture.directory.path().join(format!("refused-{index}"));
        assert!(
            Pki::import(
                fixture.source.clone(),
                state,
                layout,
                CancellationToken::new()
            )
            .await
            .is_err()
        );
    }
    let original = fixture.source.join("root/certs/ca.cert.pem");
    fs::remove_file(&original)?;
    symlink(sibling.join("root.pem"), &original)?;
    assert!(fixture.import().await.is_err());
    let source_alias = fixture.directory.path().join("source-alias");
    symlink(&fixture.source, &source_alias)?;
    assert!(
        Pki::import(
            source_alias,
            fixture.state.clone(),
            ImportLayout::openssl(),
            CancellationToken::new()
        )
        .await
        .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn state_symlinks_and_broad_key_permissions_are_refused() -> Result {
    let fixture = Fixture::new()?;
    let outside = fixture.directory.path().join("outside");
    fs::create_dir(&outside)?;
    symlink(&outside, &fixture.state)?;
    assert!(fixture.import().await.is_err());
    assert_eq!(fs::read_dir(&outside)?.count(), 0);
    fs::remove_file(&fixture.state)?;
    let ca = fixture.import().await?;
    drop(ca);
    let key = fixture.state.join("intermediate.key.pem");
    fs::set_permissions(&key, fs::Permissions::from_mode(0o644))?;
    assert!(
        Pki::open(fixture.state.clone(), CancellationToken::new())
            .await
            .is_err()
    );
    fs::set_permissions(&key, fs::Permissions::from_mode(0o600))?;
    let outside_key = outside.join("key.pem");
    fs::rename(&key, &outside_key)?;
    symlink(outside_key, &key)?;
    assert!(
        Pki::open(fixture.state.clone(), CancellationToken::new())
            .await
            .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn invalid_imports_never_become_usable_authorities() -> Result {
    for invalid_index in [
        "garbage\n",
        "V\t351001000000Z\t\t1002\tunknown\t/CN=node\n",
        "R\t351001000000Z\t\t1000\tunknown\t/CN=node\n",
    ] {
        let fixture = Fixture::new()?;
        fs::write(fixture.source.join("intermediate/index.txt"), invalid_index)?;
        assert!(fixture.import().await.is_err());
        assert!(
            Pki::open(fixture.state.clone(), CancellationToken::new())
                .await
                .is_err()
        );
    }
    let fixture = Fixture::new()?;
    let other = Fixture::new()?;
    fs::copy(
        other
            .source
            .join("intermediate/private/intermediate.key.pem"),
        fixture
            .source
            .join("intermediate/private/intermediate.key.pem"),
    )?;
    assert!(fixture.import().await.is_err());
    assert!(!fixture.state.join("intermediate.key.pem").exists());
    Ok(())
}

#[tokio::test]
async fn compatibility_crl_publication_refreshes_without_exposing_keys() -> Result {
    let fixture = Fixture::new()?;
    let public = fixture.directory.path().join("public");
    fs::create_dir(&public)?;
    let destination = public.join("intermediate.crl.pem");
    let ca = fixture
        .import()
        .await?
        .with_crl_publication(destination.clone())
        .await?;
    ca.sign(
        csr(&["spiffe://rivet.mesh/node/node-a"])?,
        SignAuthority::Renewal(node()),
    )
    .await?;
    ca.revoke(node()).await?;
    let published = fs::read_to_string(&destination)?;
    assert_eq!(
        published,
        fs::read_to_string(fixture.state.join("intermediate.crl.pem"))?
    );
    let crl_der =
        rustls_pki_types::CertificateRevocationListDer::from_pem_slice(published.as_bytes())?;
    let (_, crl) = x509_parser::parse_x509_crl(&crl_der)?;
    assert_eq!(crl.iter_revoked_certificates().count(), 2);
    assert_eq!(fs::read_dir(public)?.count(), 1);
    assert!(!published.contains("PRIVATE KEY"));
    Ok(())
}

#[tokio::test]
async fn publication_refuses_state_overwrites_traversal_and_symlinks() -> Result {
    for kind in 0..3 {
        let fixture = Fixture::new()?;
        let ca = fixture.import().await?;
        let public = fixture.directory.path().join("public");
        fs::create_dir(&public)?;
        let destination = match kind {
            0 => fixture.state.join("intermediate.key.pem"),
            1 => public.join("../state/intermediate.key.pem"),
            _ => {
                let path = public.join("crl.pem");
                symlink(fixture.state.join("intermediate.key.pem"), &path)?;
                path
            }
        };
        let key_before = fs::read(fixture.state.join("intermediate.key.pem"))?;
        assert!(ca.with_crl_publication(destination).await.is_err());
        assert_eq!(
            key_before,
            fs::read(fixture.state.join("intermediate.key.pem"))?
        );
    }
    Ok(())
}
