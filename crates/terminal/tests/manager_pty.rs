#[expect(
    dead_code,
    reason = "the harness module is included whole by every test binary and this one uses a subset"
)]
mod common;

use terminal::TerminalState;

async fn a_real_pty_runs_spawn_attach_input_resize_and_kill()
-> Result<(), Box<dyn std::error::Error>> {
    let manager = common::direct_manager(&config::Environment::default());
    let work = tempfile::tempdir()?;
    let id = manager
        .start(common::spec(
            "claude-code:00000000-0000-4000-8000-000000000001",
            work.path(),
            &["/bin/sh", "-i"],
        )?)
        .await?;

    let stream = manager.attach_stream(&id)?;
    assert_eq!(stream.record.state, TerminalState::Running);

    manager
        .input(&id, b"printf 'hel%s\\n' lo\r".to_vec())
        .await?;
    let echoed = String::from_utf8(common::until_output(stream, b"hello\r\n").await?)?;
    assert!(
        echoed.contains("hello"),
        "expected the echo, got {echoed:?}"
    );

    manager.resize(
        &id,
        kernel::terminal::TermSize {
            cols: 120,
            rows: 40,
        },
    )?;
    manager.input(&id, b"stty size\r".to_vec()).await?;
    let sized =
        String::from_utf8(common::until_output(manager.attach_stream(&id)?, b"40 120\r\n").await?)?;
    assert!(
        sized.contains("40 120"),
        "stty prints rows cols, got {sized:?}"
    );

    let second = manager.attach_stream(&id)?;
    assert!(
        second.replay.windows(5).any(|window| window == b"hello"),
        "the replay carries the scrollback"
    );
    drop(second);

    manager.kill(&id).await?;
    let record = common::wait_for_exit(&manager, &id).await?;
    let lingering = manager
        .list()
        .into_iter()
        .find(|record| record.id == id)
        .ok_or("the record lingers after exit")?;
    assert_eq!(lingering.state, TerminalState::Exited { code: 0 });
    assert_eq!(record.state, TerminalState::Exited { code: 0 });
    Ok(())
}

async fn the_exit_code_is_preserved_and_the_record_lingers()
-> Result<(), Box<dyn std::error::Error>> {
    let manager = common::direct_manager(&config::Environment::default());
    let work = tempfile::tempdir()?;
    let id = manager
        .start(common::spec(
            "claude-code:00000000-0000-4000-8000-000000000002",
            work.path(),
            &["/bin/sh", "-c", "printf 'bye 3\\n'; exit 3"],
        )?)
        .await?;
    let record = common::wait_for_exit(&manager, &id).await?;

    let late = manager.attach_stream(&id)?;
    assert_eq!(
        late.record.state,
        TerminalState::Exited { code: 3 },
        "late attach sees the exited record with the recorded code"
    );
    assert_eq!(late.replay, b"bye 3\r\n");
    assert!(late.live.is_none());
    assert_eq!(record.state, TerminalState::Exited { code: 3 });
    Ok(())
}

async fn a_missing_binary_is_a_live_record_that_prints_the_recorded_line()
-> Result<(), Box<dyn std::error::Error>> {
    let manager = common::direct_manager(&config::Environment::default());
    let work = tempfile::tempdir()?;
    let id = manager
        .start(common::spec(
            "claude-code:00000000-0000-4000-8000-000000000003",
            work.path(),
            &["acme-missing-binary"],
        )?)
        .await?;

    common::wait_for_exit(&manager, &id).await?;
    let late = manager.attach_stream(&id)?;
    assert_eq!(
        late.record.state,
        TerminalState::Exited { code: 1 },
        "the child exits 1"
    );
    assert_eq!(
        String::from_utf8_lossy(&late.replay),
        "execvp(3) failed.: No such file or directory\r\n"
    );
    Ok(())
}

async fn signal_deaths_reach_the_record_as_exit_code_zero() -> Result<(), Box<dyn std::error::Error>>
{
    let manager = common::direct_manager(&config::Environment::default());
    let work = tempfile::tempdir()?;
    let id = manager
        .start(common::spec(
            "claude-code:00000000-0000-4000-8000-000000000004",
            work.path(),
            &["/bin/sh", "-c", "kill -TERM $$"],
        )?)
        .await?;
    let record = common::wait_for_exit(&manager, &id).await?;
    assert_eq!(
        record.state,
        TerminalState::Exited { code: 0 },
        "signal deaths are exit code 0 (node-pty.json self-sigterm)"
    );
    Ok(())
}

async fn spawn_or_get_returns_the_live_record_for_the_same_session()
-> Result<(), Box<dyn std::error::Error>> {
    let manager = common::direct_manager(&config::Environment::default());
    let work = tempfile::tempdir()?;
    let session = "claude-code:00000000-0000-4000-8000-000000000005";
    let first = manager
        .start(common::spec(
            session,
            work.path(),
            &["/bin/sh", "-c", "exec cat"],
        )?)
        .await?;
    let second = manager
        .start(common::spec(
            session,
            work.path(),
            &["/bin/sh", "-c", "different program"],
        )?)
        .await?;
    assert_eq!(first, second, "spawn-or-get is keyed on the session only");
    assert_eq!(manager.list().len(), 1);
    manager.close();
    common::wait_for_exit(&manager, &first).await?;
    Ok(())
}

async fn input_to_an_exited_terminal_is_a_typed_error() -> Result<(), Box<dyn std::error::Error>> {
    let manager = common::direct_manager(&config::Environment::default());
    let work = tempfile::tempdir()?;
    let id = manager
        .start(common::spec(
            "claude-code:00000000-0000-4000-8000-000000000006",
            work.path(),
            &["/bin/sh", "-c", "exit 0"],
        )?)
        .await?;
    let _record = common::wait_for_exit(&manager, &id).await?;
    let error = manager
        .input(&id, b"x".to_vec())
        .await
        .err()
        .ok_or("input to an exited terminal must fail")?;
    assert!(error.to_string().contains("has exited"), "{error}");
    Ok(())
}

async fn the_polite_kill_escalates_to_sigkill_after_three_seconds()
-> Result<(), Box<dyn std::error::Error>> {
    let manager = common::direct_manager(&config::Environment::default());
    let work = tempfile::tempdir()?;
    let id = manager
        .start(common::spec(
            "claude-code:00000000-0000-4000-8000-000000000007",
            work.path(),
            &["/bin/sh", "-c", "trap '' HUP; printf 'ready\\n'; exec cat"],
        )?)
        .await?;
    common::until_output(manager.attach_stream(&id)?, b"ready\r\n").await?;
    let started = std::time::Instant::now();
    manager.kill(&id).await?;
    let record = common::wait_for_exit(&manager, &id).await?;
    let elapsed = started.elapsed();
    assert!(
        elapsed >= std::time::Duration::from_millis(2_800),
        "the SIGKILL escalation waits out the recorded grace, took {elapsed:?}"
    );
    assert_eq!(record.state, TerminalState::Exited { code: 0 });
    Ok(())
}

async fn a_storm_of_shortlived_children_drains_the_last_line_before_the_exit_is_reported()
-> Result<(), Box<dyn std::error::Error>> {
    let manager = common::direct_manager(&config::Environment::default());
    let work = tempfile::tempdir()?;
    let bound = std::time::Duration::from_secs(60);
    for i in 0..24 {
        let expected_code = (i % 255) + 1;
        let id = manager
            .start(common::spec(
                &format!("claude-code:00000000-0000-4000-8000-{i:012}"),
                work.path(),
                &[
                    "/bin/sh",
                    "-c",
                    &format!("printf 'bye {i}\\n'; exit {expected_code}"),
                ],
            )?)
            .await?;

        let viewer = manager.attach_stream(&id)?;
        let record = tokio::time::timeout(bound, manager.wait_exited(&id)).await??;
        let line = format!("bye {i}\r\n");
        assert_eq!(
            record.state,
            TerminalState::Exited {
                code: expected_code
            },
            "child {i} recorded the wrong exit"
        );
        let seen = common::until_output(viewer, line.as_bytes()).await?;
        assert!(
            seen.windows(line.len())
                .any(|window| window == line.as_bytes())
        );
        let late = manager.attach_stream(&id)?;
        assert!(
            late.replay
                .windows(line.len())
                .any(|window| window == line.as_bytes()),
            "child {i}: exit is reported only after the last line is drained, replay {:?}",
            String::from_utf8_lossy(&late.replay)
        );
        assert_eq!(
            late.record.state,
            TerminalState::Exited {
                code: expected_code
            }
        );
        assert!(late.live.is_none());
    }
    Ok(())
}

async fn killing_an_exited_terminal_keeps_the_record() -> Result<(), Box<dyn std::error::Error>> {
    let manager = common::direct_manager(&config::Environment::default());
    let work = tempfile::tempdir()?;
    let id = manager
        .start(common::spec(
            "claude-code:00000000-0000-4000-8000-000000000008",
            work.path(),
            &["/bin/sh", "-c", "exit 0"],
        )?)
        .await?;
    let exited = common::wait_for_exit(&manager, &id).await?;
    assert_eq!(exited.state, TerminalState::Exited { code: 0 });
    manager.kill(&id).await?;
    let again = common::wait_for_exit(&manager, &id).await?;
    assert_eq!(again.state, TerminalState::Exited { code: 0 });
    assert!(
        manager.list().iter().any(|record| record.id == id),
        "an exited terminal stays addressable after kill"
    );
    Ok(())
}

async fn unknown_terminals_are_typed_errors() -> Result<(), Box<dyn std::error::Error>> {
    let manager = common::direct_manager(&config::Environment::default());
    let unknown = kernel::terminal::TerminalId::new("pty-00000000");
    let error = manager
        .attach_stream(&unknown)
        .err()
        .ok_or("an unknown terminal must fail to attach")?;
    assert!(error.to_string().contains("is unknown"), "{error}");
    assert!(manager.kill(&unknown).await.is_err());
    Ok(())
}

#[test]
fn the_direct_pty_behaviours() -> Result<(), Box<dyn std::error::Error>> {
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(a_real_pty_runs_spawn_attach_input_resize_and_kill())?;
    runtime.block_on(the_exit_code_is_preserved_and_the_record_lingers())?;
    runtime.block_on(a_missing_binary_is_a_live_record_that_prints_the_recorded_line())?;
    runtime.block_on(signal_deaths_reach_the_record_as_exit_code_zero())?;
    runtime.block_on(spawn_or_get_returns_the_live_record_for_the_same_session())?;
    runtime.block_on(input_to_an_exited_terminal_is_a_typed_error())?;
    runtime.block_on(
        a_storm_of_shortlived_children_drains_the_last_line_before_the_exit_is_reported(),
    )?;
    runtime.block_on(the_polite_kill_escalates_to_sigkill_after_three_seconds())?;
    runtime.block_on(killing_an_exited_terminal_keeps_the_record())?;
    runtime.block_on(unknown_terminals_are_typed_errors())
}

#[tokio::test(flavor = "multi_thread")]
async fn shutdown_joins_workers_even_when_a_viewer_stays_attached()
-> Result<(), Box<dyn std::error::Error>> {
    let manager = std::sync::Arc::new(terminal::TerminalManager::new(
        common::settings(&config::Environment::default()),
        terminal::MuxPlan::Direct,
    ));
    let shutdown = tokio_util::sync::CancellationToken::new();
    let tasks = tokio_util::task::TaskTracker::new();
    manager.run_tracked(shutdown.clone(), &tasks);
    let work = tempfile::tempdir()?;
    let id = manager
        .start(common::spec(
            "shutdown",
            work.path(),
            &["/bin/sh", "-c", "trap '' HUP; printf 'ready\\n'; exec cat"],
        )?)
        .await?;
    common::until_output(manager.attach_stream(&id)?, b"ready\r\n").await?;
    let viewer = manager.attach_stream(&id)?;
    shutdown.cancel();
    tasks.close();
    tokio::time::timeout(std::time::Duration::from_secs(30), tasks.wait()).await?;
    assert_eq!(
        manager.list().first().ok_or("record")?.state,
        TerminalState::Exited { code: 0 }
    );
    drop(viewer);
    let result = manager
        .start(common::spec("after-shutdown", work.path(), &["/bin/cat"])?)
        .await;
    manager.close();
    assert!(matches!(result, Err(terminal::Error::ShuttingDown)));
    Ok(())
}
