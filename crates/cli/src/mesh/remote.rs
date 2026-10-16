use std::collections::BTreeMap;
use std::time::Duration;

use config::Environment;
use serde::Deserialize;
use serde::Serialize;
use tokio_util::sync::CancellationToken;
use url::Url;
use wire::mesh::{MeshFile, MeshNode};

use super::check::{self, Target};
use super::store::{self, Draft};
use super::{EffectAction, Finished, finish};
use crate::{Error, Exit, Report};

const MESH_TIMEOUT: Duration = Duration::from_secs(5);
const BODY_LIMIT: usize = 1024 * 1024;
const SYNC_USAGE: &str = "Usage: rh mesh sync <user@host> [--hub-cmd rivethub-hub]\n";
const JOIN_USAGE: &str = "Usage: rh mesh join --manual <host> [--port <port>]\n";
const JOIN_POINTER: &str = "\
Mesh membership is enrollment, not a join.
Run: rh mesh enroll <user@host> --name <node>
A legacy seed uses: rh mesh join --manual <host>
";

pub(super) struct SyncRequest<'a> {
    pub(super) args: &'a super::SyncArgs,
    pub(super) json: bool,
    pub(super) env: &'a Environment,
    pub(super) shutdown: &'a CancellationToken,
}

pub(super) struct JoinRequest<'a> {
    pub(super) args: &'a super::JoinArgs,
    pub(super) ssh_user: Option<String>,
    pub(super) json: bool,
    pub(super) env: &'a Environment,
    pub(super) shutdown: &'a CancellationToken,
}

enum Remote {
    Dropped,
    Status(u16),
    Reach(String),
    Bytes(Vec<u8>),
    TooLarge,
}

#[derive(Debug, Deserialize)]
struct JoinResponse {
    status: JoinStatus,
    nodes: BTreeMap<String, MeshNode>,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum JoinStatus {
    Joined,
}

#[derive(Debug, Serialize)]
struct PingReport {
    nodes: Vec<PingNode>,
}

#[derive(Debug, Serialize)]
struct PingNode {
    id: String,
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

pub(super) async fn sync(request: SyncRequest<'_>) -> Result<Report, Error> {
    let Some(spec) = request.args.target.as_deref() else {
        return Ok(check::usage(SYNC_USAGE));
    };
    let target = check::target(spec)?;
    check::hub_cmd(&request.args.hub_cmd)?;
    let mesh = store::mesh_path(request.env).await?;
    let port = hub_port(&mesh, &target.host).await?;
    let url = http_url(&target.host, port, "/mesh.json")?;
    let client = http_client(MESH_TIMEOUT)?;
    let fetched = fetch(&client, url, request.shutdown).await?;
    let file = match fetched {
        Remote::Dropped => return Ok(Report::success("")),
        Remote::Status(code) => return Err(Error::HubStatus { code }),
        Remote::Reach(message) => return Err(Error::HubReach(message)),
        Remote::Bytes(bytes) => parse_file(&bytes)?,
        Remote::TooLarge => return Err(Error::HubBody),
    };
    let warnings = merge_nodes(&mesh, file.nodes, request.shutdown).await?;
    let message = sync_message(&target, port);
    finish(Finished {
        action: EffectAction::Synced,
        node: None,
        state: None,
        mesh,
        message,
        warnings,
        json: request.json,
    })
}

pub(super) async fn join(request: JoinRequest<'_>) -> Result<Report, Error> {
    if !request.args.manual {
        return Ok(plain_join(request.args.host.as_deref()));
    }
    let Some(host) = request.args.host.as_deref() else {
        return Ok(check::usage(JOIN_USAGE));
    };
    if !check::safe_host(host) {
        return Err(Error::UnsafeHost);
    }
    let port = check::join_port(request.args.port)?;
    let name = local_name(request.args.name.as_deref(), request.env)?;
    let advertise = check::advertise(request.args.advertise.as_deref())?;
    let client = http_client(MESH_TIMEOUT)?;
    let seed = fetch_seed(&client, host, port, request.shutdown).await?;
    let seed = match seed {
        Seed::Dropped => return Ok(Report::success("")),
        Seed::Report(report) => return Ok(report),
        Seed::File(file) => file,
    };
    let node = store::node(Draft {
        name: name.clone(),
        host: advertise,
        agents: Vec::new(),
        ssh_user: request.ssh_user,
    })?;
    let joined = post_join(&client, host, port, &node, request.shutdown).await?;
    let joined = match joined {
        Posted::Dropped => return Ok(Report::success("")),
        Posted::Report(report) => return Ok(report),
        Posted::Body(response) => response,
    };
    if request.shutdown.is_cancelled() {
        return Ok(Report::success(""));
    }
    let mesh = store::mesh_path(request.env).await?;
    let mut warnings = merge_nodes(&mesh, seed.nodes, request.shutdown).await?;
    warnings.extend(merge_nodes(&mesh, joined.nodes, request.shutdown).await?);
    let message = join_message(&name, host, port);
    finish(Finished {
        action: EffectAction::Joined,
        node: Some(name),
        state: None,
        mesh,
        message,
        warnings,
        json: request.json,
    })
}

pub(super) async fn ping(
    timeout: Option<u64>,
    json: bool,
    env: &Environment,
    shutdown: &CancellationToken,
) -> Result<Report, Error> {
    let timeout = check::ping_timeout(timeout)?;
    let file = store::read_file(store::mesh_path(env).await?).await?;
    if file.nodes.is_empty() {
        return Err(Error::NoNodes);
    }
    let client = http_client(timeout)?;
    let nodes = probe_all(client, file, shutdown).await?;
    render_ping(nodes, json)
}

enum Seed {
    Dropped,
    Report(Report),
    File(MeshFile),
}

enum Posted {
    Dropped,
    Report(Report),
    Body(JoinResponse),
}

async fn fetch_seed(
    client: &reqwest::Client,
    host: &str,
    port: u16,
    shutdown: &CancellationToken,
) -> Result<Seed, Error> {
    let url = http_url(host, port, "/mesh.json")?;
    match fetch(client, url, shutdown).await? {
        Remote::Dropped => Ok(Seed::Dropped),
        Remote::Status(code) => Ok(Seed::Report(seed_status(code))),
        Remote::Reach(message) => Ok(Seed::Report(seed_reach(&message))),
        Remote::Bytes(bytes) => match serde_json::from_slice(&bytes) {
            Ok(file) => Ok(Seed::File(file)),
            Err(error) => {
                drop(error);
                Err(Error::SeedBody)
            }
        },
        Remote::TooLarge => Err(Error::SeedBody),
    }
}

async fn post_join(
    client: &reqwest::Client,
    host: &str,
    port: u16,
    node: &MeshNode,
    shutdown: &CancellationToken,
) -> Result<Posted, Error> {
    if shutdown.is_cancelled() {
        return Err(Error::Cancelled);
    }
    let url = http_url(host, port, "/api/mesh/join")?;
    let pending = client.post(url).json(node).send();
    tokio::pin!(pending);
    let response = tokio::select! {
        biased;
        () = shutdown.cancelled() => return Ok(Posted::Dropped),
        response = &mut pending => response,
    };
    match response {
        Err(error) => Ok(Posted::Report(seed_reach(&error.to_string()))),
        Ok(response) => classify_join(response).await,
    }
}

async fn classify_join(response: reqwest::Response) -> Result<Posted, Error> {
    if response.status().is_success() {
        let Loaded::Bytes(bytes) = read_limited(response).await? else {
            return Err(Error::JoinBody);
        };
        return match serde_json::from_slice::<JoinResponse>(&bytes) {
            Ok(body) => Ok(Posted::Body(require_joined(body))),
            Err(error) => {
                drop(error);
                Err(Error::JoinBody)
            }
        };
    }
    let code = response.status().as_u16();
    let bytes = match read_limited(response).await? {
        Loaded::Bytes(bytes) => bytes,
        Loaded::TooLarge => {
            return Ok(Posted::Report(Report {
                stdout: String::new(),
                stderr: format!("❌ Join failed: HTTP {code} — body exceeds 1 MiB\n"),
                exit: Exit::Failure,
            }));
        }
    };
    match String::from_utf8(bytes) {
        Ok(body) => Ok(Posted::Report(Report {
            stdout: String::new(),
            stderr: format!("❌ Join failed: HTTP {code} — {body}\n"),
            exit: Exit::Failure,
        })),
        Err(error) => {
            drop(error);
            Err(Error::MeshBody)
        }
    }
}

async fn probe_all(
    client: reqwest::Client,
    file: MeshFile,
    shutdown: &CancellationToken,
) -> Result<Vec<PingNode>, Error> {
    let mut tasks = tokio::task::JoinSet::new();
    for node in file.nodes.into_values() {
        let client = client.clone();
        let shutdown = shutdown.clone();
        tasks.spawn(async move { probe(client, node, shutdown).await });
    }
    let mut nodes = Vec::new();
    loop {
        tokio::select! {
            biased;
            () = shutdown.cancelled() => {
                tasks.abort_all();
                return Err(Error::Cancelled);
            }
            joined = tasks.join_next() => {
                let Some(joined) = joined else {
                    break;
                };
                nodes.push(joined?);
            }
        }
    }
    nodes.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(nodes)
}

async fn probe(client: reqwest::Client, node: MeshNode, shutdown: CancellationToken) -> PingNode {
    let id = node.id.clone();
    match ping_target(&node) {
        Ok(url) => match one_get(&client, url, &shutdown).await {
            Ok(()) => PingNode {
                id,
                ok: true,
                error: None,
            },
            Err(error) => PingNode {
                id,
                ok: false,
                error: Some(error),
            },
        },
        Err(error) => PingNode {
            id,
            ok: false,
            error: Some(error),
        },
    }
}

fn ping_target(node: &MeshNode) -> Result<Url, String> {
    if let Some(metadata) = &node.metadata
        && let Some(url) = &metadata.gateway_url
    {
        if url.scheme() == "http" || url.scheme() == "https" {
            return Ok(url.clone());
        }
        return Err("gateway URL scheme is not http or https".to_owned());
    }
    Url::parse(&format!("http://{}:{}/healthz", node.host, node.port))
        .map_err(|error| error.to_string())
}

async fn one_get(
    client: &reqwest::Client,
    url: Url,
    shutdown: &CancellationToken,
) -> Result<(), String> {
    let pending = client.get(url).send();
    tokio::pin!(pending);
    let response = tokio::select! {
        biased;
        () = shutdown.cancelled() => return Err("cancelled".to_owned()),
        response = &mut pending => response,
    };
    match response {
        Ok(response) if response.status().is_success() => Ok(()),
        Ok(response) => Err(format!("HTTP {}", response.status().as_u16())),
        Err(error) => Err(error.to_string()),
    }
}

fn render_ping(nodes: Vec<PingNode>, json: bool) -> Result<Report, Error> {
    let failed = nodes.iter().any(|node| !node.ok);
    let stdout = if json {
        format!("{}\n", serde_json::to_string(&PingReport { nodes })?)
    } else {
        ping_lines(&nodes)
    };
    Ok(Report {
        stdout,
        stderr: String::new(),
        exit: if failed { Exit::Failure } else { Exit::Ok },
    })
}

fn ping_lines(nodes: &[PingNode]) -> String {
    let mut text = String::new();
    for node in nodes {
        text.push_str(&node.id);
        match &node.error {
            None => text.push_str(" ok\n"),
            Some(error) => {
                text.push_str(" unreachable: ");
                text.push_str(error);
                text.push('\n');
            }
        }
    }
    text
}

async fn hub_port(path: &std::path::Path, host: &str) -> Result<u16, Error> {
    match store::read_file(path.to_path_buf()).await {
        Ok(file) => Ok(matched_port(&file, host)),
        Err(Error::NoMesh) => Ok(store::GATEWAY_PORT),
        Err(error) => Err(error),
    }
}

fn matched_port(file: &MeshFile, host: &str) -> u16 {
    match file.nodes.values().find(|node| node.host == host) {
        Some(node) => node.port,
        None => store::GATEWAY_PORT,
    }
}

async fn merge_nodes(
    mesh: &std::path::Path,
    nodes: BTreeMap<String, MeshNode>,
    shutdown: &CancellationToken,
) -> Result<Vec<String>, Error> {
    let registry = store::open_writable(mesh.to_path_buf()).await?;
    let mut warnings = Vec::new();
    for node in nodes.into_values() {
        if let Some(warning) = store::save(&registry, node, shutdown).await?.warning {
            warnings.push(warning);
        }
    }
    Ok(warnings)
}

async fn fetch(
    client: &reqwest::Client,
    url: Url,
    shutdown: &CancellationToken,
) -> Result<Remote, Error> {
    if shutdown.is_cancelled() {
        return Err(Error::Cancelled);
    }
    let pending = client.get(url).send();
    tokio::pin!(pending);
    let response = tokio::select! {
        biased;
        () = shutdown.cancelled() => return Ok(Remote::Dropped),
        response = &mut pending => response,
    };
    match response {
        Err(error) => Ok(Remote::Reach(error.to_string())),
        Ok(response) if response.status().is_success() => match read_limited(response).await? {
            Loaded::Bytes(bytes) => Ok(Remote::Bytes(bytes)),
            Loaded::TooLarge => Ok(Remote::TooLarge),
        },
        Ok(response) => Ok(Remote::Status(response.status().as_u16())),
    }
}

enum Loaded {
    Bytes(Vec<u8>),
    TooLarge,
}

async fn read_limited(response: reqwest::Response) -> Result<Loaded, Error> {
    let bytes = response.bytes().await?;
    if bytes.len() > BODY_LIMIT {
        Ok(Loaded::TooLarge)
    } else {
        Ok(Loaded::Bytes(bytes.to_vec()))
    }
}

fn require_joined(response: JoinResponse) -> JoinResponse {
    match response.status {
        JoinStatus::Joined => response,
    }
}

fn parse_file(bytes: &[u8]) -> Result<MeshFile, Error> {
    match serde_json::from_slice(bytes) {
        Ok(file) => Ok(file),
        Err(error) => {
            drop(error);
            Err(Error::HubBody)
        }
    }
}

fn http_client(timeout: Duration) -> Result<reqwest::Client, Error> {
    Ok(reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(timeout)
        .build()?)
}

fn http_url(host: &str, port: u16, path: &str) -> Result<Url, Error> {
    Ok(Url::parse(&format!("http://{host}:{port}{path}"))?)
}

fn local_name(explicit: Option<&str>, env: &Environment) -> Result<String, Error> {
    match explicit {
        Some(name) => {
            check::node_name(name)?;
            Ok(name.to_owned())
        }
        None => Ok(env.node_name(&check::host_name()?)?),
    }
}

fn plain_join(host: Option<&str>) -> Report {
    match host {
        None => Report::success(JOIN_POINTER),
        Some(host) => Report {
            stdout: String::new(),
            stderr: format!(
                "mesh join {host} is no longer a join (exiting non-zero so scripts do not assume success). Use `rh mesh enroll <user@host> --name <node>`, or `rh mesh join --manual <host>` for the legacy YAML flow.\n"
            ),
            exit: Exit::Failure,
        },
    }
}

fn seed_status(code: u16) -> Report {
    Report {
        stdout: String::new(),
        stderr: format!("❌ Seed node responded with HTTP {code}\n"),
        exit: Exit::Failure,
    }
}

fn seed_reach(message: &str) -> Report {
    Report {
        stdout: String::new(),
        stderr: format!("❌ Cannot reach seed node: {message}\n"),
        exit: Exit::Failure,
    }
}

fn sync_message(target: &Target, port: u16) -> String {
    format!(
        "Synced mesh nodes from {}@{} via http://{}:{port}/mesh.json.\n\
SSH rivethub-hub mesh-export is retired.\n\
Local-only nodes were kept.\n",
        target.user, target.host, target.host
    )
}

fn join_message(name: &str, host: &str, port: u16) -> String {
    format!(
        "Joined {name} through the legacy seed at http://{host}:{port}/api/mesh/join.\n\
step-ca was not run.\n\
The registry was merged into mesh.json.\n"
    )
}
