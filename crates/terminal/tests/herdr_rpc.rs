use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use terminal::herdr::AgentKind;
use terminal::herdr::HerdrEventKind;
use terminal::herdr::client::HerdrClient;
use terminal::herdr::codec;
use terminal::herdr::codec::RequestParams;
use terminal::herdr::codec::RpcMethod;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
use tokio::net::UnixListener;

type Failed = Box<dyn std::error::Error + Send + Sync>;

struct Step {
    method: RpcMethod,
    params: RequestParams,
    frames: Vec<&'static str>,
    exact: Option<&'static str>,
}

fn step(method: RpcMethod, params: RequestParams, frames: &'static [&'static str]) -> Step {
    Step {
        method,
        params,
        frames: frames.to_vec(),
        exact: None,
    }
}

fn pinned(
    method: RpcMethod,
    params: RequestParams,
    exact: &'static str,
    frames: &'static [&'static str],
) -> Step {
    Step {
        method,
        params,
        frames: frames.to_vec(),
        exact: Some(exact),
    }
}

async fn serve(listener: UnixListener, steps: Vec<Step>) -> Result<(), Failed> {
    for expected in &steps {
        let (stream, _) = listener.accept().await?;
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);
        let mut line = Vec::new();
        let read = reader.read_until(b'\n', &mut line).await?;
        if read == 0 {
            return Err("the client closed the socket early".into());
        }
        let frame = codec::decode_request(&line)?;
        assert_eq!(
            frame.method, expected.method,
            "the client must send {}",
            expected.method
        );
        assert_eq!(
            frame.params, expected.params,
            "params for {}",
            expected.method
        );
        for template in &expected.frames {
            let out = template.replace("{ID}", &frame.id);
            writer.write_all(out.as_bytes()).await?;
            writer.flush().await?;
        }
        if let Some(text) = expected.exact {
            assert_eq!(
                String::from_utf8(line)?,
                text.replace("{ID}", &frame.id),
                "the wire bytes for {} are the recorded ones",
                expected.method
            );
        }
    }
    Ok(())
}

const ACK_PLAIN: &[&str] = &["{\"id\":\"{ID}\",\"result\":{\"type\":\"workspace_created\"}}\n"];
const ACK_SUBSCRIPTION: &[&str] =
    &["{\"id\":\"{ID}\",\"result\":{\"type\":\"subscription_started\"}}\n"];
const ACK_OK: &[&str] = &["{\"id\":\"{ID}\",\"result\":{\"type\":\"ok\"}}\n"];

fn workspace_create_params() -> RequestParams {
    RequestParams::WorkspaceCreate(codec::WorkspaceCreateParams {
        cwd: PathBuf::from("/home/user/AcmeKit"),
        env: BTreeMap::from([("ACME".to_owned(), "roster".to_owned())]),
        label: "acme".to_owned(),
    })
}

fn pane_get_params() -> RequestParams {
    RequestParams::PaneGet(codec::PaneGetParams {
        pane_id: "w1:p1".to_owned(),
    })
}

fn send_text_params() -> RequestParams {
    RequestParams::PaneSendText(codec::PaneSendTextParams {
        pane_id: "w1:p1".to_owned(),
        text: "true\n".to_owned(),
    })
}

fn agent_start_params() -> RequestParams {
    RequestParams::AgentStart(codec::AgentStartParams {
        name: "d0123456789ab".to_owned(),
        kind: AgentKind::Claude,
        pane_id: "w1:p1".to_owned(),
        args: vec![
            "--resume".to_owned(),
            "00000000-0000-4000-8000-000000000002".to_owned(),
        ],
        timeout_ms: 10_000,
    })
}

fn subscription_params() -> RequestParams {
    RequestParams::EventsSubscribe(codec::EventsSubscribeParams {
        subscriptions: vec![
            codec::SubscriptionParams {
                event: HerdrEventKind::PaneAgentStatusChanged,
                pane_id: "w1:p1".to_owned(),
            },
            codec::SubscriptionParams {
                event: HerdrEventKind::PaneAgentDetected,
                pane_id: "w1:p1".to_owned(),
            },
        ],
    })
}

async fn the_client_speaks_the_recorded_frames_and_maps_the_acks() -> Result<(), Failed> {
    let dir = tempfile::tempdir()?;
    let socket = dir.path().join("herdr.sock");
    let listener = UnixListener::bind(&socket)?;
    let server = tokio::spawn(serve(
        listener,
        vec![
            pinned(
                RpcMethod::WorkspaceCreate,
                workspace_create_params(),
                "{\"id\":\"{ID}\",\"method\":\"workspace.create\",\"params\":{\"cwd\":\"/home/user/AcmeKit\",\"env\":{\"ACME\":\"roster\"},\"label\":\"acme\"}}\n",
                ACK_PLAIN,
            ),
            step(RpcMethod::PaneGet, pane_get_params(), ACK_PLAIN),
            step(
                RpcMethod::EventsSubscribe,
                subscription_params(),
                ACK_SUBSCRIPTION,
            ),
        ],
    ));
    let client = HerdrClient::new(&socket);
    client
        .workspace_create(codec::WorkspaceCreateParams {
            cwd: PathBuf::from("/home/user/AcmeKit"),
            env: BTreeMap::from([("ACME".to_owned(), "roster".to_owned())]),
            label: "acme".to_owned(),
        })
        .await?;
    client
        .pane_get(codec::PaneGetParams {
            pane_id: "w1:p1".to_owned(),
        })
        .await?;
    client
        .events_subscribe(codec::EventsSubscribeParams {
            subscriptions: vec![
                codec::SubscriptionParams {
                    event: HerdrEventKind::PaneAgentStatusChanged,
                    pane_id: "w1:p1".to_owned(),
                },
                codec::SubscriptionParams {
                    event: HerdrEventKind::PaneAgentDetected,
                    pane_id: "w1:p1".to_owned(),
                },
            ],
        })
        .await?;
    let joined = tokio::time::timeout(Duration::from_secs(5), server).await??;
    joined?;
    Ok(())
}

async fn event_frames_flow_to_subscribers_through_both_aliases() -> Result<(), Failed> {
    let dir = tempfile::tempdir()?;
    let socket = dir.path().join("herdr.sock");
    let listener = UnixListener::bind(&socket)?;
    let server = tokio::spawn(serve(
        listener,
        vec![
            step(RpcMethod::PaneGet, pane_get_params(), ACK_OK),
            step(
                RpcMethod::EventsSubscribe,
                subscription_params(),
                &[
                    "{\"id\":\"{ID}\",\"result\":{\"type\":\"subscription_started\"}}\n",
                    "{\"event\":\"pane_agent_status_changed\",\"data\":{\"pane_id\":\"w1:p1\"}}\n",
                    "{\"event\":\"pane.agent_detected\",\"data\":{\"pane_id\":\"w1:p1\"}}\n",
                ],
            ),
        ],
    ));
    let client = HerdrClient::new(&socket);
    let mut events = client.subscribe_events();
    client
        .pane_get(codec::PaneGetParams {
            pane_id: "w1:p1".to_owned(),
        })
        .await?;
    client
        .events_subscribe(codec::EventsSubscribeParams {
            subscriptions: vec![
                codec::SubscriptionParams {
                    event: HerdrEventKind::PaneAgentStatusChanged,
                    pane_id: "w1:p1".to_owned(),
                },
                codec::SubscriptionParams {
                    event: HerdrEventKind::PaneAgentDetected,
                    pane_id: "w1:p1".to_owned(),
                },
            ],
        })
        .await?;
    let first = events.recv().await.map_err(|error| error.to_string())?;
    assert_eq!(first.event, HerdrEventKind::PaneAgentStatusChanged);
    assert_eq!(
        first.data.get("pane_id").and_then(|found| found.as_str()),
        Some("w1:p1")
    );
    let second = events.recv().await.map_err(|error| error.to_string())?;
    assert_eq!(second.event, HerdrEventKind::PaneAgentDetected);
    let joined = tokio::time::timeout(Duration::from_secs(5), server).await??;
    joined?;
    Ok(())
}

async fn error_acks_become_typed_rpc_errors() -> Result<(), Failed> {
    let dir = tempfile::tempdir()?;
    let socket = dir.path().join("herdr.sock");
    let listener = UnixListener::bind(&socket)?;
    let server = tokio::spawn(serve(
        listener,
        vec![step(
            RpcMethod::PaneGet,
            pane_get_params(),
            &[
                "{\"id\":\"{ID}\",\"error\":{\"code\":\"pane_not_found\",\"message\":\"pane not found\"}}\n",
            ],
        )],
    ));
    let client = HerdrClient::new(&socket);
    let error = client
        .pane_get(codec::PaneGetParams {
            pane_id: "w1:p1".to_owned(),
        })
        .await
        .err()
        .ok_or("expected a typed error")?;
    let rendered = error.to_string();
    assert!(
        matches!(
            &error,
            terminal::Error::Ack {
                method: RpcMethod::PaneGet,
                error: codec::AckError {
                    code: codec::AckErrorCode::Other(code),
                    ..
                },
            } if code == "pane_not_found"
        ),
        "{rendered}"
    );
    assert!(rendered.contains("pane.get"), "{rendered}");
    assert!(rendered.contains("pane not found"), "{rendered}");
    let joined = tokio::time::timeout(Duration::from_secs(5), server).await??;
    joined?;
    Ok(())
}

async fn a_silent_server_times_out_with_the_recorded_ctl_window() -> Result<(), Failed> {
    let dir = tempfile::tempdir()?;
    let socket = dir.path().join("herdr.sock");
    let listener = UnixListener::bind(&socket)?;
    let server = tokio::spawn(async move {
        let (_stream, _) = listener.accept().await.ok()?;
        tokio::time::sleep(Duration::from_secs(2)).await;
        Some(())
    });
    let client = HerdrClient::new(&socket);
    let error = client
        .pane_get(codec::PaneGetParams {
            pane_id: "w1:p1".to_owned(),
        })
        .await
        .err()
        .ok_or("expected the timeout")?;
    assert!(
        error.to_string().contains("timed out after 250ms"),
        "{error}"
    );
    server.abort();
    Ok(())
}

async fn agent_start_retries_while_the_pane_is_busy() -> Result<(), Failed> {
    let dir = tempfile::tempdir()?;
    let socket = dir.path().join("herdr.sock");
    let listener = UnixListener::bind(&socket)?;
    let server = tokio::spawn(serve(
        listener,
        vec![
            step(
                RpcMethod::AgentStart,
                agent_start_params(),
                &[
                    "{\"id\":\"{ID}\",\"error\":{\"code\":\"agent_pane_busy\",\"message\":\"pane w1:p1 is busy\"}}\n",
                ],
            ),
            step(RpcMethod::AgentStart, agent_start_params(), ACK_PLAIN),
        ],
    ));
    let client = HerdrClient::new(&socket);
    client
        .agent_start(codec::AgentStartParams {
            name: "d0123456789ab".to_owned(),
            kind: AgentKind::Claude,
            pane_id: "w1:p1".to_owned(),
            args: vec![
                "--resume".to_owned(),
                "00000000-0000-4000-8000-000000000002".to_owned(),
            ],
            timeout_ms: 10_000,
        })
        .await?;
    let joined = tokio::time::timeout(Duration::from_secs(5), server).await??;
    joined?;
    Ok(())
}

async fn send_text_carries_the_shell_joined_line() -> Result<(), Failed> {
    let dir = tempfile::tempdir()?;
    let socket = dir.path().join("herdr.sock");
    let listener = UnixListener::bind(&socket)?;
    let server = tokio::spawn(serve(
        listener,
        vec![pinned(
            RpcMethod::PaneSendText,
            send_text_params(),
            "{\"id\":\"{ID}\",\"method\":\"pane.send_text\",\"params\":{\"pane_id\":\"w1:p1\",\"text\":\"true\\n\"}}\n",
            ACK_OK,
        )],
    ));
    let client = HerdrClient::new(&socket);
    client
        .pane_send_text(codec::PaneSendTextParams {
            pane_id: "w1:p1".to_owned(),
            text: "true\n".to_owned(),
        })
        .await?;
    let joined = tokio::time::timeout(Duration::from_secs(5), server).await??;
    joined?;
    Ok(())
}

#[test]
fn the_herdr_rpc_client_behaviours() -> Result<(), Failed> {
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(the_client_speaks_the_recorded_frames_and_maps_the_acks())?;
    runtime.block_on(event_frames_flow_to_subscribers_through_both_aliases())?;
    runtime.block_on(error_acks_become_typed_rpc_errors())?;
    runtime.block_on(a_silent_server_times_out_with_the_recorded_ctl_window())?;
    runtime.block_on(agent_start_retries_while_the_pane_is_busy())?;
    runtime.block_on(send_text_carries_the_shell_joined_line())
}
