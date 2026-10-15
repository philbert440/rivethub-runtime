#[expect(
    dead_code,
    reason = "the shared test harness supplies helpers for several test binaries"
)]
mod common;

use std::time::Duration;

use serde::Deserialize;
use terminal::TerminalState;
use wire::gateway::term::{HelloState, TermServerFrame};

const SKIPPED: &[(&str, &str)] = &[];
const ENVIRONMENT_CASE: &str = "attach-envdump";

#[derive(Deserialize)]
struct Recording {
    steps: Vec<Step>,
}

#[derive(Deserialize)]
struct Step {
    id: String,
    hello: Option<Hello>,
    exit: Option<TermServerFrame>,
    replay: Option<String>,
    frames: Option<Vec<Frame>>,
}

#[derive(Deserialize)]
struct Hello {
    cols: u16,
    rows: u16,
    state: HelloState,
    #[serde(rename = "exitCode")]
    exit_code: u32,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum Frame {
    Binary { text: String },
    Text { text: String },
}

fn recording() -> Result<Recording, Box<dyn std::error::Error>> {
    Ok(serde_json::from_str(&std::fs::read_to_string(
        testkit::recording("term/ws-pty-frames.json"),
    )?)?)
}

#[test]
fn every_terminal_recording_scenario_is_accounted_for() -> Result<(), Box<dyn std::error::Error>> {
    let cases = recording()?;
    let covered = [
        "exit-3",
        "late-attach-exited",
        "attach-missing",
        ENVIRONMENT_CASE,
        "sigterm-self",
        "sigkill-self",
        "delete-term",
    ];
    assert_eq!(cases.steps.len(), covered.len() + SKIPPED.len());
    for step in &cases.steps {
        assert!(
            covered.contains(&step.id.as_str())
                || SKIPPED
                    .iter()
                    .any(|(name, reason)| *name == step.id && !reason.is_empty()),
            "unaccounted scenario {}",
            step.id
        );
    }
    for (name, reason) in SKIPPED {
        assert_ne!(*reason, "");
        assert!(cases.steps.iter().any(|step| step.id == *name));
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn recorded_pty_frames_replay_through_the_manager() -> Result<(), Box<dyn std::error::Error>>
{
    let work = tempfile::tempdir()?;
    for (index, step) in recording()?.steps.into_iter().enumerate() {
        if step.id == ENVIRONMENT_CASE || SKIPPED.iter().any(|(name, _)| *name == step.id) {
            continue;
        }
        let manager = common::direct_manager(&config::Environment::default());
        let argv = match step.id.as_str() {
            "exit-3" => vec!["/bin/sh", "-c", "printf 'exit 3\\nbye 3\\n'; exit 3"],
            "late-attach-exited" => vec!["/bin/sh", "-c", "exit 3"],
            "attach-missing" => vec!["acme-missing-binary"],
            "sigterm-self" => vec!["/bin/sh", "-c", "kill -TERM $$"],
            "sigkill-self" => vec!["/bin/sh", "-c", "kill -KILL $$"],
            "delete-term" => vec!["/bin/cat"],
            other => return Err(format!("unhandled scenario {other}").into()),
        };
        let mut spec = common::spec(&format!("fixture-{index}"), work.path(), &argv)?;
        if let Some(hello) = &step.hello {
            spec.size = kernel::terminal::TermSize {
                cols: hello.cols,
                rows: hello.rows,
            };
        }
        let id = manager.start(spec).await?;
        if step.id == "delete-term" {
            manager.kill(&id).await?;
        }
        let record =
            tokio::time::timeout(Duration::from_secs(30), manager.wait_exited(&id)).await??;
        let TerminalState::Exited { code } = record.state else {
            return Err("expected exited state".into());
        };
        let attached = manager.attach_stream(&id)?;
        assert!(attached.live.is_none());
        if let Some(hello) = step.hello {
            assert_eq!(hello.state, HelloState::Exited);
            assert_eq!(code, hello.exit_code);
            assert_eq!(
                record.size,
                kernel::terminal::TermSize {
                    cols: hello.cols,
                    rows: hello.rows
                }
            );
        }
        if let Some(replay) = step.replay {
            assert_eq!(attached.replay, replay.as_bytes());
        }
        let actual_exit = TermServerFrame::Exit {
            code: Some(i32::try_from(code)?),
            signal: None,
        };
        if let Some(exit) = step.exit {
            assert_eq!(actual_exit, exit);
        }
        if let Some(frames) = step.frames {
            for frame in frames {
                match frame {
                    Frame::Binary { text } => assert_eq!(attached.replay, text.as_bytes()),
                    Frame::Text { text } => {
                        assert_eq!(actual_exit, serde_json::from_str::<TermServerFrame>(&text)?);
                    }
                }
            }
        }
    }
    Ok(())
}
