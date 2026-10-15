use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use chrono::{TimeDelta, Utc};
use config::{Config, Environment};
use gateway::mesh::{AgentCall, AgentChannel, ChannelSettings, Heartbeat, Registry};
use kernel::agent::{Turn, TurnLimits, TurnRunner, excluded_names};
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use wire::mesh::{MeshNode, MeshNodeStatus};

use super::ComposeError;

pub(super) async fn start(
    config: &Config,
    env: &Environment,
    agents: BTreeMap<String, super::models::AgentBinding>,
    tenancy: gateway::tenancy::Tenancy,
    shutdown: &CancellationToken,
) -> Result<JoinSet<Result<(), ComposeError>>, ComposeError> {
    let mut tasks = JoinSet::new();
    let Some(mesh) = config
        .mesh
        .as_ref()
        .filter(|mesh| mesh.enabled == Some(true))
    else {
        return Ok(tasks);
    };
    let source = gateway::server::BindSource::from_config(config);
    let port = source.port;
    let tls = tokio::task::spawn_blocking(move || gateway::tls::resolve_tls(&source))
        .await??
        .ok_or(gateway::mesh::ChannelError::TlsRequired)?;
    let settings = ChannelSettings::from_config(config, tls, Duration::from_secs(120))?;
    let registry = Registry::open(config::shared_dir(env), "mesh.json".into()).await?;
    let name = crate::identity::resolve(config, env)?.name;
    let host = mesh
        .advertise_host
        .clone()
        .ok_or_else(|| ComposeError::Invalid("mesh.advertise_host is required".into()))?;
    let agent_ids = agents.keys().cloned().collect();
    let mut mailboxes = BTreeMap::new();
    for (id, binding) in agents {
        let (send, receive) = mpsc::channel(32);
        mailboxes.insert(id.clone(), send);
        tasks.spawn(listen(
            id,
            binding.model,
            binding.runner,
            receive,
            shutdown.child_token(),
        ));
    }
    let channel_port = settings.agent_channel_port;
    let channel = AgentChannel::bind(settings, mailboxes, shutdown.child_token())
        .await?
        .with_tenancy(tenancy);
    let local = local_node(config, name, host, channel_port, port, agent_ids)?;
    let heartbeat = Heartbeat::new(
        registry,
        local,
        Duration::from_secs(30),
        TimeDelta::seconds(120),
    )?;
    let heartbeat_cancel = shutdown.child_token();
    tasks.spawn(async move {
        heartbeat.run(heartbeat_cancel).await?;
        Ok(())
    });
    tasks.spawn(async move {
        channel.serve().await?;
        Ok(())
    });
    Ok(tasks)
}

const GATEWAY_CAPABILITY: &str = "den";

fn local_node(
    config: &Config,
    name: String,
    host: String,
    channel_port: u16,
    gateway_port: u16,
    agents: Vec<String>,
) -> Result<MeshNode, ComposeError> {
    let now = Utc::now();
    let gateway_url = url::Url::parse(&format!("https://{host}:{gateway_port}/"))
        .map_err(|error| ComposeError::Invalid(format!("mesh gateway url: {error}")))?;
    Ok(MeshNode {
        id: name.clone(),
        name,
        role: None,
        agents,
        host,
        port: channel_port,
        providers: config
            .providers
            .as_ref()
            .map_or_else(Vec::new, |providers| providers.keys().cloned().collect()),
        models: Vec::new(),
        capabilities: vec![GATEWAY_CAPABILITY.to_owned()],
        status: MeshNodeStatus::Online,
        last_seen: now,
        registered_at: now,
        version: env!("CARGO_PKG_VERSION").into(),
        metadata: Some(wire::mesh::MeshMetadata {
            gateway_url: Some(gateway_url),
            gateway_port: Some(gateway_port),
            extra: wire::Extra::default(),
        }),
        ssh_user: None,
        install_root: None,
        platform: None,
        extra: wire::Extra::default(),
    })
}

async fn listen(
    id: String,
    model: String,
    runner: Arc<dyn TurnRunner>,
    mut calls: mpsc::Receiver<AgentCall>,
    cancel: CancellationToken,
) -> Result<(), ComposeError> {
    let mut turns = JoinSet::new();
    loop {
        tokio::select! {
            () = cancel.cancelled() => break,
            result = turns.join_next(), if !turns.is_empty() => {
                if let Some(result) = result { result?; }
            },
            call = calls.recv() => {
                let Some(call) = call else { break };
                turns.spawn(run_call(id.clone(), model.clone(), runner.clone(), call));
            }
        }
    }
    while let Some(result) = turns.join_next().await {
        result?;
    }
    Ok(())
}

async fn run_call(id: String, model: String, runner: Arc<dyn TurnRunner>, call: AgentCall) {
    let session = call.message.session_key;
    let turn = Turn {
        request: kernel::model::ModelRequest {
            model,
            messages: vec![wire::message::Message {
                role: wire::message::Role::User,
                content: wire::message::MessageContent::Text(call.message.content),
                reasoning: None,
                tool_calls: None,
                tool_call_id: None,
            }],
            tools: Vec::new(),
            thinking: None,
            max_tokens: None,
        },
        context: wire::hooks::HookContextBase {
            agent_id: Some(id.clone()),
            session_id: session.clone(),
            timestamp: Utc::now(),
            metadata: wire::JsonObject::default(),
        },
        tool_context: kernel::ToolContext {
            agent_id: Some(id),
            working_dir: None,
            session: None,
        },
        authorized_tools: BTreeSet::new(),
        excluded_tools: excluded_names(None),
        limits: TurnLimits {
            steps: 1,
            timeout: Duration::from_secs(120),
            cleanup_timeout: Duration::from_secs(5),
            response_bytes: 4 * 1024 * 1024,
        },
    };
    let (sender, mut events) = mpsc::channel(32);
    let drain = async { while events.recv().await.is_some() {} };
    let (result, ()) = tokio::join!(runner.run(turn, sender, call.cancellation), drain);
    match result {
        Ok(result) => {
            if call
                .reply
                .send(wire::mesh_channel::AgentReply {
                    content: result.response,
                    session_key: session,
                })
                .is_err()
            {
                tracing::debug!("agent channel caller disconnected");
            }
        }
        Err(error) => tracing::warn!("agent channel turn failed: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_local_node_advertises_its_gateway_and_agent_channel()
    -> Result<(), Box<dyn std::error::Error>> {
        let loaded = Config::from_str(
            &config::Environment::default(),
            "runtime:\n  workspace: /tmp\n  default_agent: main\nagents:\n  main:\n    provider: p\nproviders:\n  p:\n    type: openai-compatible\n    base_url: http://127.0.0.1:9/v1\n    model: m\n",
        )?;
        let node = local_node(
            &loaded.config,
            "node-a".to_owned(),
            "192.0.2.15".to_owned(),
            3001,
            5175,
            vec!["rivet".to_owned()],
        )?;
        assert_eq!(node.port, 3001);
        assert_eq!(node.capabilities, vec![GATEWAY_CAPABILITY.to_owned()]);
        let metadata = node.metadata.ok_or("metadata")?;
        assert_eq!(
            metadata.gateway_url.map(String::from).as_deref(),
            Some("https://192.0.2.15:5175/")
        );
        assert_eq!(metadata.gateway_port, Some(5175));
        Ok(())
    }
}
