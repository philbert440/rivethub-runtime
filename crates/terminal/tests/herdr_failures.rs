use std::time::Duration;

use terminal::Error;
use terminal::herdr::client::HerdrClient;
use terminal::herdr::codec::{AckResult, EventsSubscribeParams, PaneGetParams, RequestFrame};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;

type Failed = Box<dyn std::error::Error + Send + Sync>;

async fn reply(frame: &'static str) -> Result<Result<Option<AckResult>, Error>, Failed> {
    let dir = tempfile::tempdir()?;
    let socket = dir.path().join("control.sock");
    let listener = UnixListener::bind(&socket)?;
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await?;
        let (read, mut write) = stream.into_split();
        let mut line = String::new();
        BufReader::new(read).read_line(&mut line).await?;
        let request: RequestFrame = serde_json::from_str(&line)?;
        write
            .write_all(frame.replace("{ID}", &request.id).as_bytes())
            .await?;
        Ok::<(), Failed>(())
    });
    let result = tokio::time::timeout(
        Duration::from_secs(30),
        HerdrClient::new(&socket).pane_get(PaneGetParams {
            pane_id: "w1:p1".into(),
        }),
    )
    .await?;
    server.await??;
    Ok(result)
}

#[tokio::test]
async fn uncoded_and_empty_errors_are_errors_without_waiting_for_a_timeout() -> Result<(), Failed> {
    for frame in [
        "{\"id\":\"{ID}\",\"error\":{\"message\":\"refused\"}}\n",
        "{\"id\":\"{ID}\",\"error\":{}}\n",
    ] {
        assert!(
            matches!(reply(frame).await?, Err(Error::Ack { error, .. }) if error.code.is_none())
        );
    }
    Ok(())
}

#[tokio::test]
async fn malformed_frames_and_wrong_field_types_are_typed_errors() -> Result<(), Failed> {
    for frame in [
        "not json\n",
        "{\"id\":\"{ID}\",\"error\":{\"code\":42}}\n",
        "{\"id\":\"{ID}\",\"error\":{\"message\":false}}\n",
    ] {
        assert!(matches!(reply(frame).await?, Err(Error::Json(_))));
    }
    assert!(matches!(
        reply("{\"id\":\"another\"}\n").await?,
        Err(Error::AckId { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn missing_results_remain_absent() -> Result<(), Failed> {
    assert_eq!(reply("{\"id\":\"{ID}\"}\n").await??, None);
    Ok(())
}

#[tokio::test]
async fn malformed_event_stream_reports_its_error() -> Result<(), Failed> {
    let dir = tempfile::tempdir()?;
    let socket = dir.path().join("control.sock");
    let listener = UnixListener::bind(&socket)?;
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await?;
        let (read, mut write) = stream.into_split();
        let mut line = String::new();
        BufReader::new(read).read_line(&mut line).await?;
        let request: RequestFrame = serde_json::from_str(&line)?;
        let ack = format!(
            "{{\"id\":\"{}\",\"result\":{{\"type\":\"subscription_started\"}}}}\ninvalid\n",
            request.id
        );
        write.write_all(ack.as_bytes()).await?;
        Ok::<(), Failed>(())
    });
    let client = HerdrClient::new(&socket);
    let mut events = client.subscribe_events();
    client
        .events_subscribe(EventsSubscribeParams {
            subscriptions: vec![],
        })
        .await?;
    let event = tokio::time::timeout(Duration::from_secs(30), events.recv()).await??;
    assert!(matches!(event, Err(error) if matches!(*error, Error::Json(_))));
    server.await??;
    Ok(())
}

#[tokio::test]
async fn an_unexecutable_probe_preserves_the_io_error() -> Result<(), Failed> {
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("herdr"), "not executable")?;
    let failure = terminal::herdr::probe(&[dir.path().to_path_buf()], None)
        .await
        .expect_err("not executable");
    assert!(
        matches!(failure, terminal::herdr::ProbeFailure::Execute { source, .. } if source.kind() == std::io::ErrorKind::PermissionDenied)
    );
    Ok(())
}

#[tokio::test]
async fn extension_results_retain_their_kind_and_fields() -> Result<(), Failed> {
    let result =
        reply("{\"id\":\"{ID}\",\"result\":{\"type\":\"future_result\",\"label\":\"kept\"}}\n")
            .await??
            .ok_or("result")?;
    assert_eq!(
        result.kind,
        terminal::herdr::codec::AckResultKind::Other("future_result".into())
    );
    assert_eq!(
        result.fields.get("label").and_then(|value| value.as_str()),
        Some("kept")
    );
    Ok(())
}

#[test]
fn signalling_zero_is_a_typed_refusal_without_touching_the_process_group() {
    assert!(matches!(
        terminal::pty::signal(0, nix::sys::signal::Signal::SIGTERM),
        Err(Error::InvalidPid(0))
    ));
}
