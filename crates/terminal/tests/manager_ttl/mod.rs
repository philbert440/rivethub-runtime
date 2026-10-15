use std::collections::BTreeMap;
use std::time::Duration;

use crate::{
    Error, MuxPlan, RosterKey, SessionKey, SpawnSpec, TerminalManager, TerminalSettings,
    TerminalState,
};
use kernel::terminal::{TermSize, TerminalId};

type Failed = Box<dyn std::error::Error>;

fn manager(vars: &[(&str, &str)]) -> TerminalManager {
    let env = config::Environment::from_vars(vars.iter().copied());
    TerminalManager::new(
        TerminalSettings::from_config(&config::Config::default(), &env, "test-host".into()),
        MuxPlan::Direct,
    )
}

async fn start(
    manager: &TerminalManager,
    root: &std::path::Path,
    key: &str,
    script: &str,
) -> Result<TerminalId, Error> {
    manager
        .start(SpawnSpec {
            session: Some(SessionKey::new(key)?),
            roster_key: RosterKey::new("sh")?,
            argv: vec!["/bin/sh".into(), "-c".into(), script.into()],
            cwd: root.into(),
            size: TermSize { cols: 80, rows: 24 },
            entry_env: BTreeMap::new(),
        })
        .await
}

async fn exited(manager: &TerminalManager, id: &TerminalId) -> Result<(), Failed> {
    tokio::time::resume();
    let result = tokio::time::timeout(Duration::from_secs(30), manager.wait_exited(id)).await;
    tokio::time::pause();
    result??;
    Ok(())
}

async fn printed(manager: &TerminalManager, id: &TerminalId) -> Result<(), Failed> {
    tokio::time::resume();
    let mut stream = manager.attach_stream(id)?;
    let result = tokio::time::timeout(Duration::from_secs(30), async {
        while !stream.replay.windows(7).any(|bytes| bytes == b"ready\r\n") {
            stream.replay.extend(
                stream
                    .live
                    .as_mut()
                    .ok_or("no live stream")?
                    .recv()
                    .await
                    .ok_or("output closed")?,
            );
        }
        Ok::<(), Failed>(())
    })
    .await;
    drop(stream);
    let mut notices = manager
        .inner
        .detach_rx
        .lock()
        .take()
        .ok_or("detach receiver")?;
    let detached = tokio::time::timeout(Duration::from_secs(30), notices.recv())
        .await?
        .ok_or("detach closed")?;
    manager.mark_detached(&detached);
    *manager.inner.detach_rx.lock() = Some(notices);
    tokio::time::pause();
    result??;
    Ok(())
}

async fn advance(manager: &TerminalManager, seconds: u64) {
    tokio::time::advance(Duration::from_secs(seconds)).await;
    manager.sweep();
}

#[tokio::test(start_paused = true)]
async fn linger_and_session_gc_use_the_later_deadline() -> Result<(), Failed> {
    for (gc, deadline) in [("0", 60), ("120000", 120)] {
        let manager = manager(&[
            ("RIVETHUB_TERM_EXIT_LINGER_MS", "60000"),
            ("RIVETHUB_TERM_SESSION_GC_MS", gc),
        ]);
        let work = tempfile::tempdir()?;
        let id = start(&manager, work.path(), "linger", "exit 3").await?;
        exited(&manager, &id).await?;
        assert_eq!(
            manager.list().first().expect("terminal record").state,
            TerminalState::Exited { code: 3 }
        );
        advance(&manager, deadline - 1).await;
        assert_eq!(manager.list().len(), 1);
        advance(&manager, 1).await;
        assert_eq!(manager.list(), []);
    }
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn the_cap_evicts_the_lru_idle_viewerless_terminal() -> Result<(), Failed> {
    let manager = manager(&[("RIVETHUB_TERM_MAX", "1")]);
    let work = tempfile::tempdir()?;
    let a = start(&manager, work.path(), "a", "printf 'ready\\n'; exec cat").await?;
    printed(&manager, &a).await?;
    advance(&manager, 3).await;
    let b = start(&manager, work.path(), "b", "exec cat").await?;
    exited(&manager, &a).await?;
    let error = start(&manager, work.path(), "c", "exec cat")
        .await
        .expect_err("silent B is not evictable");
    assert!(matches!(error, Error::PtyLimit { max: 1 }));
    manager.close();
    exited(&manager, &b).await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn detached_ttl_spares_attached_viewers() -> Result<(), Failed> {
    let manager = manager(&[("RIVETHUB_TERM_DETACHED_TTL_MS", "1800000")]);
    let work = tempfile::tempdir()?;
    let id = start(&manager, work.path(), "detached", "exec cat").await?;
    let stream = manager.attach_stream(&id)?;
    advance(&manager, 2000).await;
    assert_eq!(
        manager.list().first().expect("terminal record").state,
        TerminalState::Running
    );
    drop(stream);
    let mut notices = manager
        .inner
        .detach_rx
        .lock()
        .take()
        .ok_or("detach receiver")?;
    let detached = tokio::time::timeout(Duration::from_secs(30), notices.recv())
        .await?
        .ok_or("detach closed")?;
    manager.mark_detached(&detached);
    advance(&manager, 1799).await;
    assert_eq!(
        manager.list().first().expect("terminal record").state,
        TerminalState::Running
    );
    advance(&manager, 1).await;
    exited(&manager, &id).await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn idle_ttl_resets_on_input_and_zero_disables_it() -> Result<(), Failed> {
    for idle in ["1800000", "0"] {
        let manager = manager(&[
            ("RIVETHUB_TERM_IDLE_TTL_MS", idle),
            ("RIVETHUB_TERM_DETACHED_TTL_MS", "999999999999"),
        ]);
        let work = tempfile::tempdir()?;
        let id = start(&manager, work.path(), "idle", "exec cat").await?;
        advance(&manager, 1700).await;
        manager.input(&id, b"ready\n".to_vec()).await?;
        printed(&manager, &id).await?;
        advance(&manager, 1700).await;
        assert_eq!(
            manager.list().first().expect("terminal record").state,
            TerminalState::Running
        );
        advance(&manager, 101).await;
        if idle == "0" {
            assert_eq!(
                manager.list().first().expect("terminal record").state,
                TerminalState::Running
            );
            manager.close();
        }
        exited(&manager, &id).await?;
    }
    Ok(())
}

#[tokio::test]
async fn port_listing_refuses_an_invalid_session_instead_of_dropping_it() -> Result<(), Failed> {
    let manager = manager(&[]);
    let work = tempfile::tempdir()?;
    let id = start(&manager, work.path(), "missing-prefix", "exec cat").await?;
    let result = kernel::terminal::TerminalMux::list(&manager).await;
    assert!(matches!(
        result,
        Err(kernel::terminal::TerminalMuxError::Unavailable(_))
    ));
    manager.close();
    tokio::time::timeout(Duration::from_secs(30), manager.wait_exited(&id)).await??;
    Ok(())
}

#[tokio::test]
async fn child_wait_failure_is_retained_as_a_typed_error() -> Result<(), Failed> {
    let manager = manager(&[]);
    let work = tempfile::tempdir()?;
    let id = manager
        .start(SpawnSpec {
            session: None,
            roster_key: RosterKey::new("missing")?,
            argv: vec!["acme-missing-binary".into()],
            cwd: work.path().into(),
            size: TermSize { cols: 80, rows: 24 },
            entry_env: BTreeMap::new(),
        })
        .await?;
    let (output, input) = tokio::sync::mpsc::channel(1);
    let (exit, outcome) = tokio::sync::oneshot::channel();
    if let Some(super::registry::Runtime::Pty { exit_watch, .. }) =
        manager.inner.state.lock().get_mut(&id)
    {
        exit_watch.send_replace(TerminalState::Running);
    }
    manager.stream_until_exit(id.clone(), input, outcome);
    drop(output);
    exit.send(Err(Error::Io(std::io::Error::other("child wait failed"))))
        .expect("send failure");
    let result = tokio::time::timeout(Duration::from_secs(30), manager.wait_exited(&id)).await?;
    assert!(matches!(result, Err(Error::PtyFailure(source)) if matches!(*source, Error::Io(_))));
    assert_eq!(
        manager.list().first().expect("record").state,
        TerminalState::Failed
    );
    assert!(matches!(
        manager.attach_stream(&id),
        Err(Error::PtyFailure(_))
    ));
    Ok(())
}
