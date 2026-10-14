#[expect(
    dead_code,
    reason = "shared CA fixture exposes helpers used by the other integration targets"
)]
mod support;

use kernel::ca::{CaError, CertificateAuthority, EnrollmentToken, SignAuthority};
use pki::Pki;
use support::{Fixture, Result, csr, node};
use tokio_util::sync::CancellationToken;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn enrollment_token_is_atomic_once_and_survives_restart_hashed() -> Result {
    let fixture = Fixture::new()?;
    let ca = fixture.import().await?;
    let token = ca.issue_token(node()).await?;
    let saved = token.0.clone();
    let connection = rusqlite::Connection::open(fixture.state.join("ca.sqlite"))?;
    let digest: Vec<u8> =
        connection.query_row("SELECT digest FROM rh_ca_tokens", [], |row| row.get(0))?;
    assert_eq!(digest.len(), 32);
    assert_ne!(digest, saved.as_bytes());
    drop(connection);
    drop(ca);
    let ca = Pki::open(fixture.state.clone(), CancellationToken::new()).await?;
    let mut jobs = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let authority = SignAuthority::Token(EnrollmentToken(saved.clone()));
        let request = csr(&["spiffe://rivet.mesh/node/node-a"])?;
        let ca = ca.clone();
        jobs.spawn(async move { ca.sign(request, authority).await });
    }
    let mut successes = 0;
    while let Some(result) =
        tokio::time::timeout(std::time::Duration::from_secs(10), jobs.join_next()).await?
    {
        match result? {
            Ok(_) => successes += 1,
            Err(CaError::Refused(_)) => (),
            Err(error) => return Err(error.into()),
        }
    }
    assert_eq!(successes, 1);
    drop(ca);
    let ca = Pki::open(fixture.state.clone(), CancellationToken::new()).await?;
    assert!(matches!(
        ca.sign(
            csr(&["spiffe://rivet.mesh/node/node-a"])?,
            SignAuthority::Token(token)
        )
        .await,
        Err(CaError::Refused(_))
    ));
    for entry in std::fs::read_dir(&fixture.state)? {
        let bytes = std::fs::read(entry?.path())?;
        assert!(
            !bytes
                .windows(saved.len())
                .any(|window| window == saved.as_bytes())
        );
    }
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn token_expires_at_fifteen_minutes_without_wall_clock_sleep() -> Result {
    let fixture = Fixture::new()?;
    let ca = fixture.import().await?;
    let token = ca.issue_token(node()).await?;
    tokio::time::advance(std::time::Duration::from_secs(899)).await;
    ca.sign(
        csr(&["spiffe://rivet.mesh/node/node-a"])?,
        SignAuthority::Token(token),
    )
    .await?;
    let expires = ca.issue_token(node()).await?;
    tokio::time::advance(std::time::Duration::from_mins(15)).await;
    assert!(matches!(
        ca.sign(
            csr(&["spiffe://rivet.mesh/node/node-a"])?,
            SignAuthority::Token(expires)
        )
        .await,
        Err(CaError::Refused(_))
    ));
    Ok(())
}

#[tokio::test]
async fn invalid_request_does_not_consume_bound_token() -> Result {
    let fixture = Fixture::new()?;
    let ca = fixture.import().await?;
    let token = ca.issue_token(node()).await?;
    let copy = EnrollmentToken(token.0.clone());
    assert!(matches!(
        ca.sign(
            csr(&["spiffe://rivet.mesh/node/node-b"])?,
            SignAuthority::Token(copy)
        )
        .await,
        Err(CaError::Refused(_))
    ));
    ca.sign(
        csr(&["spiffe://rivet.mesh/node/node-a"])?,
        SignAuthority::Token(token),
    )
    .await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn rate_limits_and_cancellation_fail_closed() -> Result {
    let fixture = Fixture::new()?;
    let ca = fixture.import().await?;
    for _ in 0..60 {
        ca.issue_token(node()).await?;
    }
    assert!(matches!(
        ca.issue_token(node()).await,
        Err(CaError::Refused(_))
    ));
    for _ in 0..60 {
        assert!(
            ca.sign(
                csr(&["spiffe://rivet.mesh/node/node-a"])?,
                SignAuthority::Token(EnrollmentToken("invalid".to_owned()))
            )
            .await
            .is_err()
        );
    }
    assert!(matches!(
        ca.sign(
            csr(&["spiffe://rivet.mesh/node/node-a"])?,
            SignAuthority::Renewal(node())
        )
        .await,
        Err(CaError::Refused(_))
    ));
    tokio::time::advance(std::time::Duration::from_secs(60)).await;
    let token = ca.issue_token(node()).await?;
    ca.sign(
        csr(&["spiffe://rivet.mesh/node/node-a"])?,
        SignAuthority::Token(token),
    )
    .await?;
    drop(ca);
    let cancellation = CancellationToken::new();
    let ca = Pki::open(fixture.state.clone(), cancellation.clone()).await?;
    cancellation.cancel();
    assert!(matches!(ca.chain().await, Err(CaError::Unavailable(_))));
    assert!(matches!(
        ca.issue_token(node()).await,
        Err(CaError::Unavailable(_))
    ));
    assert!(matches!(
        ca.revoke(node()).await,
        Err(CaError::Unavailable(_))
    ));
    Ok(())
}
