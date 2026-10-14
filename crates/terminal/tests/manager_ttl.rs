#[expect(
    dead_code,
    reason = "the harness module is included whole by every test binary and this one uses a subset"
)]
mod common;

use kernel::terminal::TerminalId;
use std::time::Duration as RealDuration;
use terminal::TerminalManager;
use terminal::TerminalState;
use tokio::time::Duration as TokioDuration;

type Failed = Box<dyn std::error::Error>;

fn limits_env(vars: &[(&str, &str)]) -> config::Environment {
    common::env_from(vars)
}

async fn tick() {
    #[expect(
        clippy::disallowed_methods,
        reason = "the reaper's blocking tasks only observe the paused clock after a real-time slice"
    )]
    std::thread::sleep(RealDuration::from_millis(10));
    tokio::time::advance(TokioDuration::from_millis(200)).await;
    tokio::task::yield_now().await;
}

async fn sweep_seconds(seconds: u64) {
    for _ in 0..seconds {
        tokio::time::advance(TokioDuration::from_secs(1)).await;
        tokio::task::yield_now().await;
        #[expect(
            clippy::disallowed_methods,
            reason = "the reaper's blocking tasks only observe the paused clock after a real-time slice"
        )]
        std::thread::sleep(RealDuration::from_millis(2));
    }
}

fn record_of(manager: &TerminalManager, id: &TerminalId) -> Option<terminal::TerminalRecord> {
    manager.list().into_iter().find(|record| record.id == *id)
}

async fn wait_reaped(manager: &TerminalManager, id: &TerminalId) -> bool {
    for _ in 0..600 {
        tick().await;
        if record_of(manager, id).is_none() {
            return true;
        }
    }
    false
}

#[tokio::test(start_paused = true)]
async fn the_exit_linger_keeps_the_record_then_the_reaper_reaps_it() -> Result<(), Failed> {
    let env = limits_env(&[
        ("RIVETHUB_TERM_EXIT_LINGER_MS", "60000"),
        ("RIVETHUB_TERM_IDLE_TTL_MS", "0"),
        ("RIVETHUB_TERM_DETACHED_TTL_MS", "999999999999"),
    ]);
    let manager = common::direct_manager(&env);
    let work = tempfile::tempdir()?;
    let session = "claude-code:00000000-0000-4000-8000-000000000101";
    let id = manager
        .start(common::spec(
            session,
            work.path(),
            &["/bin/sh", "-c", "exit 3"],
        )?)
        .await?;
    let record = manager.wait_exited(&id).await?;
    assert_eq!(record.state, TerminalState::Exited { code: 3 });

    for _ in 0..200 {
        tick().await;
        assert!(
            record_of(&manager, &id).is_some(),
            "still inside the recorded linger window"
        );
    }

    let reaped = wait_reaped(&manager, &id).await;
    assert!(
        reaped,
        "the reaper reaps after the recorded linger: {:?}",
        manager.list()
    );
    let again = manager
        .start(common::spec(
            session,
            work.path(),
            &["/bin/sh", "-c", "exit 0"],
        )?)
        .await?;
    assert!(record_of(&manager, &again).is_some());
    manager.wait_exited(&again).await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn the_detached_ttl_kills_unattached_terminals_and_spares_attached_ones() -> Result<(), Failed>
{
    let env = limits_env(&[
        ("RIVETHUB_TERM_DETACHED_TTL_MS", "1800000"),
        ("RIVETHUB_TERM_IDLE_TTL_MS", "0"),
    ]);
    let manager = common::direct_manager(&env);
    let work = tempfile::tempdir()?;
    let id = manager
        .start(common::spec(
            "claude-code:00000000-0000-4000-8000-000000000102",
            work.path(),
            &["/bin/sh", "-c", "sleep 30"],
        )?)
        .await?;

    let stream = manager.attach_stream(&id)?;
    for _ in 0..100 {
        tokio::time::advance(TokioDuration::from_secs(20)).await;
        tokio::task::yield_now().await;
        #[expect(
            clippy::disallowed_methods,
            reason = "the reaper's blocking tasks only observe the paused clock after a real-time slice"
        )]
        std::thread::sleep(RealDuration::from_millis(2));
    }
    assert!(
        record_of(&manager, &id).is_some(),
        "both TTLs are suspended while a viewer is attached"
    );
    drop(stream);
    tick().await;

    let mut killed = None;
    for _ in 0..60 {
        sweep_seconds(60).await;
        if let Some(record) = record_of(&manager, &id)
            && record.state != TerminalState::Running
        {
            killed = Some(record);
            break;
        }
    }
    let killed = killed.ok_or("the reaper kills the long-detached terminal")?;
    assert_eq!(
        killed.state,
        TerminalState::Exited { code: 0 },
        "the reaper's SIGHUP is a signal death: exit code 0"
    );
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn the_idle_ttl_kills_quiet_terminals_and_a_write_resets_it() -> Result<(), Failed> {
    let env = limits_env(&[
        ("RIVETHUB_TERM_IDLE_TTL_MS", "1800000"),
        ("RIVETHUB_TERM_DETACHED_TTL_MS", "999999999999"),
    ]);
    let manager = common::direct_manager(&env);
    let work = tempfile::tempdir()?;
    let id = manager
        .start(common::spec(
            "claude-code:00000000-0000-4000-8000-000000000103",
            work.path(),
            &["/bin/sh", "-c", "cat; sleep 30"],
        )?)
        .await?;
    let stream = manager.attach_stream(&id)?;

    for _ in 0..80 {
        tokio::time::advance(TokioDuration::from_secs(20)).await;
        tokio::task::yield_now().await;
        #[expect(
            clippy::disallowed_methods,
            reason = "the reaper's blocking tasks only observe the paused clock after a real-time slice"
        )]
        std::thread::sleep(RealDuration::from_millis(2));
    }
    assert!(
        record_of(&manager, &id).is_some(),
        "attached viewers suspend the idle rule"
    );

    manager.input(&id, b"keep-alive\r".to_vec()).await?;
    for _ in 0..80 {
        tokio::time::advance(TokioDuration::from_secs(20)).await;
        tokio::task::yield_now().await;
        #[expect(
            clippy::disallowed_methods,
            reason = "the reaper's blocking tasks only observe the paused clock after a real-time slice"
        )]
        std::thread::sleep(RealDuration::from_millis(2));
        assert!(
            record_of(&manager, &id).is_some(),
            "the write reset the idle clock"
        );
    }
    drop(stream);
    manager.close();
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn an_idle_ttl_of_zero_disables_the_reaper_rule() -> Result<(), Failed> {
    let env = limits_env(&[
        ("RIVETHUB_TERM_IDLE_TTL_MS", "0"),
        ("RIVETHUB_TERM_DETACHED_TTL_MS", "999999999999"),
    ]);
    let manager = common::direct_manager(&env);
    let work = tempfile::tempdir()?;
    let id = manager
        .start(common::spec(
            "claude-code:00000000-0000-4000-8000-000000000104",
            work.path(),
            &["/bin/sh", "-c", "sleep 30"],
        )?)
        .await?;
    for _ in 0..120 {
        tokio::time::advance(TokioDuration::from_secs(20)).await;
        tokio::task::yield_now().await;
        #[expect(
            clippy::disallowed_methods,
            reason = "the reaper's blocking tasks only observe the paused clock after a real-time slice"
        )]
        std::thread::sleep(RealDuration::from_millis(2));
    }
    assert!(
        record_of(&manager, &id).is_some(),
        "idle TTL 0 disables the rule (the recorded default is 0-disabled)"
    );
    manager.close();
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn the_cap_evicts_the_lru_idle_viewerless_terminal() -> Result<(), Failed> {
    let env = limits_env(&[("RIVETHUB_TERM_MAX", "1")]);
    let manager = common::direct_manager(&env);
    let work = tempfile::tempdir()?;
    let a = manager
        .start(common::spec(
            "claude-code:00000000-0000-4000-8000-000000000105",
            work.path(),
            &["/bin/sh", "-c", "echo start; sleep 30"],
        )?)
        .await?;

    let stream = manager.attach_stream(&a)?;
    let mut banner = false;
    for _ in 0..300 {
        let replay = manager.attach_stream(&a)?.replay;
        if replay.windows(5).any(|window| window == b"start") {
            banner = true;
            break;
        }
        drop(replay);
        tick().await;
    }
    assert!(banner, "A printed its banner");
    drop(stream);
    tick().await;

    sweep_seconds(3).await;

    let b = manager
        .start(common::spec(
            "claude-code:00000000-0000-4000-8000-000000000106",
            work.path(),
            &["/bin/sh", "-c", "sleep 30"],
        )?)
        .await?;
    let a_record = manager.wait_exited(&a).await?;
    assert_eq!(
        a_record.state,
        TerminalState::Exited { code: 0 },
        "the eviction is a SIGHUP: exit code 0"
    );

    let error = manager
        .start(common::spec(
            "claude-code:00000000-0000-4000-8000-000000000107",
            work.path(),
            &["/bin/sh", "-c", "sleep 30"],
        )?)
        .await
        .err()
        .ok_or("B never printed, so it is not evictable")?;
    assert_eq!(
        error.to_string(),
        "pty limit reached (1); all active",
        "the recorded cap text"
    );
    assert!(record_of(&manager, &b).is_some());
    manager.close();
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn the_session_gc_reaps_past_the_linger_when_enabled() -> Result<(), Failed> {
    let env = limits_env(&[
        ("RIVETHUB_TERM_EXIT_LINGER_MS", "60000"),
        ("RIVETHUB_TERM_SESSION_GC_MS", "120000"),
        ("RIVETHUB_TERM_IDLE_TTL_MS", "0"),
        ("RIVETHUB_TERM_DETACHED_TTL_MS", "999999999999"),
    ]);
    let manager = common::direct_manager(&env);
    let work = tempfile::tempdir()?;
    let id = manager
        .start(common::spec(
            "claude-code:00000000-0000-4000-8000-000000000108",
            work.path(),
            &["/bin/sh", "-c", "exit 0"],
        )?)
        .await?;

    manager.wait_exited(&id).await?;
    for _ in 0..250 {
        tick().await;
        assert!(
            record_of(&manager, &id).is_some(),
            "with GC enabled the record outlives the bare linger"
        );
    }

    let reaped = wait_reaped(&manager, &id).await;
    assert!(
        reaped,
        "the GC interval reaps the record: {:?}",
        manager.list()
    );
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn close_sighups_every_live_terminal() -> Result<(), Failed> {
    let manager = common::direct_manager(&limits_env(&[]));
    let work = tempfile::tempdir()?;
    let id = manager
        .start(common::spec(
            "claude-code:00000000-0000-4000-8000-000000000109",
            work.path(),
            &["/bin/sh", "-c", "sleep 30"],
        )?)
        .await?;
    manager.close();
    let record = manager.wait_exited(&id).await?;
    assert_eq!(record.state, TerminalState::Exited { code: 0 });
    Ok(())
}
