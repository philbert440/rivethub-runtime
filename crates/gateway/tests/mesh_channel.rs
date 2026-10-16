pub mod mesh_support;

use std::collections::BTreeMap;
use std::time::Duration;

use gateway::mesh::{AgentCall, AgentChannel, ChannelError, ChannelSettings};
use mesh_support::{Material, TestResult};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use wire::mesh_channel::{AgentList, AgentMessage, AgentReply};

fn message(agent: &str) -> AgentMessage {
    AgentMessage {
        agent_id: agent.to_owned(),
        content: "Read 'quoted text', $(date), and https://example.test/redirect?next=/wiki 🦀"
            .to_owned(),
        session_key: Some("peer:conversation".to_owned()),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn authenticated_peers_reach_independent_agents_over_tls() -> TestResult {
    let material = Material::new()?;
    let client = material.client(Some(material.peer.clone()))?;
    let (slow, mut slow_calls) = mpsc::channel::<AgentCall>(1);
    let (fast, mut fast_calls) = mpsc::channel::<AgentCall>(1);
    let shutdown = CancellationToken::new();
    let channel = AgentChannel::bind(
        material.settings.clone(),
        BTreeMap::from([("slow".to_owned(), slow), ("fast".to_owned(), fast)]),
        shutdown.clone(),
    )
    .await?;
    let base = format!("https://{}", channel.local_addr()?);
    let server = tokio::spawn(channel.serve());
    let list: AgentList = client
        .get(format!("{base}/agents"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(list.agents, ["fast", "slow"]);
    let slow_request = tokio::spawn(
        client
            .post(format!("{base}/message"))
            .json(&message("slow"))
            .send(),
    );
    let held = tokio::time::timeout(Duration::from_secs(2), slow_calls.recv())
        .await?
        .ok_or("no slow message")?;
    assert_eq!(held.message, message("slow"));
    let fast_request = tokio::spawn(
        client
            .post(format!("{base}/message"))
            .json(&message("fast"))
            .send(),
    );
    let call = tokio::time::timeout(Duration::from_secs(2), fast_calls.recv())
        .await?
        .ok_or("no fast message")?;
    let reply = AgentReply {
        content: call.message.content,
        session_key: call.message.session_key,
    };
    assert!(call.reply.send(reply.clone()).is_ok());
    let fast_reply: AgentReply = fast_request.await??.error_for_status()?.json().await?;
    assert_eq!(fast_reply, reply);
    assert!(!slow_request.is_finished());
    assert!(held.reply.send(reply.clone()).is_ok());
    let slow_reply: AgentReply = slow_request.await??.error_for_status()?.json().await?;
    assert_eq!(slow_reply, reply);
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(2), server).await???;
    Ok(())
}

#[tokio::test]
async fn missing_untrusted_and_plaintext_clients_cannot_reach_agents() -> TestResult {
    let material = Material::new()?;
    let foreign = Material::new()?;
    let (agent, mut calls) = mpsc::channel(1);
    let shutdown = CancellationToken::new();
    let channel = AgentChannel::bind(
        material.settings.clone(),
        BTreeMap::from([("local".to_owned(), agent)]),
        shutdown.clone(),
    )
    .await?;
    let addr = channel.local_addr()?;
    let server = tokio::spawn(channel.serve());
    for identity in [None, Some(foreign.peer)] {
        let client = material.client(identity)?;
        assert!(
            client
                .post(format!("https://{addr}/message"))
                .json(&message("local"))
                .send()
                .await
                .is_err()
        );
    }
    let plain = material.client(None)?;
    assert!(
        plain
            .post(format!("http://{addr}/message"))
            .json(&message("local"))
            .send()
            .await
            .is_err()
    );
    assert!(matches!(
        calls.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(2), server).await???;
    Ok(())
}

#[tokio::test]
async fn malformed_unknown_oversized_and_unavailable_messages_fail_explicitly() -> TestResult {
    let material = Material::new()?;
    let client = material.client(Some(material.peer.clone()))?;
    let (agent, calls) = mpsc::channel(1);
    drop(calls);
    let shutdown = CancellationToken::new();
    let channel = AgentChannel::bind(
        material.settings.clone(),
        BTreeMap::from([("local".to_owned(), agent)]),
        shutdown.clone(),
    )
    .await?;
    let url = format!("https://{}/message", channel.local_addr()?);
    let server = tokio::spawn(channel.serve());
    assert_eq!(
        client
            .post(&url)
            .json(&message("unknown"))
            .send()
            .await?
            .status(),
        404
    );
    assert_eq!(
        client
            .post(&url)
            .json(&message("local"))
            .send()
            .await?
            .status(),
        503
    );
    for input in [
        r#"{"agentId":3,"content":"hi"}"#,
        r#"{"agentId":"local","content":false}"#,
    ] {
        assert_eq!(
            client
                .post(&url)
                .header("content-type", "application/json")
                .body(input)
                .send()
                .await?
                .status(),
            422
        );
    }
    let mut empty = message("local");
    empty.content = "   ".to_owned();
    assert_eq!(client.post(&url).json(&empty).send().await?.status(), 400);
    let mut large = message("local");
    large.content = "x".repeat(300 * 1024);
    assert_eq!(client.post(&url).json(&large).send().await?.status(), 413);
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(2), server).await???;
    Ok(())
}

#[tokio::test]
async fn request_timeout_cancels_work_and_shutdown_drains_pending_requests() -> TestResult {
    let mut material = Material::new()?;
    material.settings.request_timeout = Duration::from_millis(250);
    let client = material.client(Some(material.peer.clone()))?;
    let (agent, mut calls) = mpsc::channel(1);
    let shutdown = CancellationToken::new();
    let channel = AgentChannel::bind(
        material.settings.clone(),
        BTreeMap::from([("local".to_owned(), agent)]),
        shutdown.clone(),
    )
    .await?;
    let url = format!("https://{}/message", channel.local_addr()?);
    let server = tokio::spawn(channel.serve());
    let first = tokio::spawn(client.post(&url).json(&message("local")).send());
    let call = tokio::time::timeout(Duration::from_secs(2), calls.recv())
        .await?
        .ok_or("no first call")?;
    tokio::time::timeout(Duration::from_secs(2), call.cancellation.cancelled()).await?;
    assert_eq!(first.await??.status(), 504);
    let second = tokio::spawn(client.post(&url).json(&message("local")).send());
    let call = tokio::time::timeout(Duration::from_secs(2), calls.recv())
        .await?
        .ok_or("no second call")?;
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(2), call.cancellation.cancelled()).await?;
    assert_eq!(second.await??.status(), 503);
    tokio::time::timeout(Duration::from_secs(2), server).await???;
    Ok(())
}

#[tokio::test]
async fn settings_use_mesh_port_and_gateway_tls_and_refuse_missing_security() -> TestResult {
    let material = Material::new()?;
    let config: config::Config = serde_yaml_ng::from_str(
        "mesh:\n  enabled: true\n  node_name: local\n  tls: true\n  agent_channel_port: 5800\n",
    )?;
    let settings = ChannelSettings::from_config(
        &config,
        &config::Environment::default(),
        "local",
        std::path::Path::new("unused-shared"),
        material.settings.tls.clone(),
        Duration::from_secs(30),
    )?;
    assert_eq!(settings.agent_channel_port, 5800);
    assert_eq!(settings.tls, material.settings.tls);
    let mut insecure = settings.clone();
    insecure.tls.ca = None;
    assert!(matches!(
        AgentChannel::bind(insecure, BTreeMap::new(), CancellationToken::new()).await,
        Err(ChannelError::TlsRequired)
    ));
    let mut broken = settings;
    broken.tls.cert = std::path::PathBuf::from("missing-mesh-node.crt");
    assert!(matches!(
        AgentChannel::bind(broken, BTreeMap::new(), CancellationToken::new()).await,
        Err(ChannelError::Tls(_))
    ));
    let disabled: config::Config = serde_yaml_ng::from_str(
        "mesh:\n  enabled: true\n  tls: false\n  agent_channel_port: 5800\n",
    )?;
    assert!(matches!(
        ChannelSettings::from_config(
            &disabled,
            &config::Environment::default(),
            "local",
            std::path::Path::new("unused-shared"),
            material.settings.tls,
            Duration::from_secs(30),
        ),
        Err(ChannelError::TlsRequired)
    ));
    Ok(())
}
