use std::sync::Arc;

use config::Environment;
use tokio_util::sync::CancellationToken;
use wire::mesh::MeshNode;

use super::{UpdateArgs, UpdateError};
use crate::service::command::{capture, command};
use crate::{Exit, Report};

#[derive(Debug)]
pub(super) struct Peer {
    node: MeshNode,
    user: Option<String>,
}

pub(super) async fn preflight(
    args: &UpdateArgs,
    config: &config::Config,
    env: &Environment,
    shutdown: &CancellationToken,
    report: &mut Report,
) -> Result<Vec<Peer>, UpdateError> {
    let hostname = nix::unistd::gethostname()?
        .into_string()
        .map_err(|_| UpdateError::HostnameEncoding)?;
    let local = config::resolve_node_name(config, env, &hostname)?;
    let registry =
        gateway::mesh::Registry::open(config::shared_dir(env), "mesh.json".into()).await?;
    let mesh = registry.snapshot().await?.ok_or(UpdateError::MeshMissing)?;
    let mut candidates = Vec::new();
    for node in mesh.nodes.into_values() {
        if node.id == local || node.name == local {
            continue;
        }
        let user = args.ssh_user.as_ref().or(node.ssh_user.as_ref()).cloned();
        validate(&node, user.as_deref())?;
        candidates.push(Peer { node, user });
    }
    let env = Arc::new(env.clone());
    let mut tasks = tokio::task::JoinSet::new();
    for (index, peer) in candidates.into_iter().enumerate() {
        let env = Arc::clone(&env);
        let shutdown = shutdown.clone();
        tasks.spawn(async move {
            let result = invoke(&peer, "true", &env, &shutdown).await;
            (index, peer, result)
        });
    }
    let mut results = std::collections::BTreeMap::new();
    while let Some(result) = tasks.join_next().await {
        let (index, peer, result) = result?;
        results.insert(index, (peer, result));
    }
    let mut peers = Vec::new();
    for (peer, result) in results.into_values() {
        let result = result?;
        if result.exit == Exit::Ok {
            report.stdout.extend(["Reachable: ", &peer.node.name, "\n"]);
            peers.push(peer);
        } else {
            report
                .stderr
                .extend(["Unreachable: ", &peer.node.name, "\n", &result.stderr]);
            report.exit = Exit::Failure;
        }
    }
    Ok(peers)
}

pub(super) async fn update(
    peers: Vec<Peer>,
    args: &UpdateArgs,
    version: &str,
    env: &Environment,
    shutdown: &CancellationToken,
    report: &mut Report,
) -> Result<(), UpdateError> {
    for peer in peers {
        report
            .stdout
            .extend(["Updating node: ", &peer.node.name, "\n"]);
        let selected = match args.channel {
            super::Channel::Stable | super::Channel::Beta => Some(version),
            super::Channel::Nightly => args.version.as_deref(),
        };
        let remote = remote_command(args, selected)?;
        let result = invoke(&peer, &remote, env, shutdown).await?;
        report.stdout.push_str(&result.stdout);
        report.stderr.push_str(&result.stderr);
        if result.exit != Exit::Ok {
            report
                .stderr
                .extend(["Update failed: ", &peer.node.name, "\n"]);
            report.exit = Exit::Failure;
        }
    }
    Ok(())
}

fn validate(node: &MeshNode, user: Option<&str>) -> Result<(), UpdateError> {
    let host = &node.host;
    let valid_host = !host.is_empty()
        && !host.starts_with('-')
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '[' | ']'));
    let valid_user = user.is_none_or(|user| {
        !user.is_empty()
            && !user.starts_with('-')
            && user
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    });
    if valid_host && valid_user {
        Ok(())
    } else {
        Err(UpdateError::SshDestination(node.id.clone()))
    }
}

fn remote_command(args: &UpdateArgs, version: Option<&str>) -> Result<String, UpdateError> {
    let mut words = vec![
        "rh".to_owned(),
        "update".to_owned(),
        "--channel".to_owned(),
        args.channel.to_string(),
    ];
    if let Some(version) = version {
        words.extend(["--version".to_owned(), version.to_owned()]);
    }
    if args.no_restart {
        words.push("--no-restart".to_owned());
    }
    shlex::try_join(words.iter().map(String::as_str)).map_err(|_| UpdateError::ShellArgument)
}

async fn invoke(
    peer: &Peer,
    remote: &str,
    env: &Environment,
    shutdown: &CancellationToken,
) -> Result<Report, UpdateError> {
    let mut ssh = command("ssh", env);
    ssh.args([
        "-o",
        "BatchMode=yes",
        "-o",
        "ConnectTimeout=10",
        "-o",
        "StrictHostKeyChecking=yes",
        "-o",
        "ServerAliveInterval=15",
        "-o",
        "ServerAliveCountMax=2",
    ]);
    if let Some(user) = &peer.user {
        ssh.args(["-l", user]);
    }
    ssh.arg("--").arg(&peer.node.host).arg(remote);
    Ok(capture(ssh, "ssh", shutdown).await?)
}
