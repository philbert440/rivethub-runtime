use crate::common;

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::ConnectInfo;
use axum::http::{Method, Request, StatusCode};
use gateway::Gateway;
use gateway::plugins::{HostSettings, PluginHost};
use gateway::tenancy::{PeerCertificate, Tenancy, UserId};
use serde::Serialize;
use tower::ServiceExt as _;
use wire::JsonObject;
use wire::gateway::GatewayError;
use wire::harness::HarnessId;
use wire::plugin::action::{ViewActionRefusal, ViewActionResult};
use wire::plugin::host::{
    PluginEnableRequest, PluginInstallRequest, PluginJobOutcome, PluginLogStream, PluginLogs,
    PluginRecord, PluginSettings, PluginState,
};
use wire::plugin::manifest::{Permission, PluginManifest};
use wire::plugin::process::{PluginEvent, PluginToolCall, PluginToolResult};
use wire::plugin::view::{View, ViewValue};
use wire::session_id::SessionId;

type Failure = Box<dyn std::error::Error + Send + Sync>;
type TestResult = Result<(), Failure>;

const POLL: Duration = Duration::from_millis(50);

const MANIFEST: &str = r#"{
  "manifest": 1,
  "id": "fixture",
  "name": "Fixture",
  "version": "0.1.0",
  "description": "A plug-in for the host tests.",
  "run": { "command": ["COMMAND"], "readyTimeoutMs": 5000 },
  "permissions": ["sessions.read", "tasks.read"],
  "tools": [
    { "name": "echo", "description": "Echo the arguments.", "inputSchema": { "type": "object" } },
    { "name": "events", "description": "List received events.", "inputSchema": { "type": "object" } },
    { "name": "pid", "description": "Answer the process id.", "inputSchema": { "type": "object" } },
    { "name": "api", "description": "Call the plug-in API.", "inputSchema": { "type": "object" } },
    { "name": "read", "description": "Read a file.", "inputSchema": { "type": "object" } }
  ],
  "settings": [
    { "key": "runs", "label": "Runs at once", "input": { "type": "number", "min": 1, "max": 8 }, "value": 2, "required": true },
    { "key": "token", "label": "Token", "input": { "type": "secret" } }
  ],
  "contributes": { "pages": [{ "id": "fixture", "title": "Fixture", "view": "status" }] },
  "jobs": [{ "id": "tick", "title": "Tick" }],
  "events": ["session.idle", "task.done"]
}"#;

#[derive(Serialize)]
struct Probe<'a> {
    method: &'a str,
    path: &'a str,
}

#[derive(Serialize)]
struct FileProbe<'a> {
    path: &'a Path,
}

struct Harness {
    app: Router,
    gateway: Arc<Gateway>,
}

impl Harness {
    async fn call(
        &self,
        method: Method,
        path: &str,
        body: Option<String>,
        device: bool,
    ) -> Result<(StatusCode, Bytes), Failure> {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header("host", "127.0.0.1:5174")
            .header("content-type", "application/json")
            .body(body.map_or_else(Body::empty, Body::from))?;
        request
            .extensions_mut()
            .insert(ConnectInfo("127.0.0.1:41000".parse::<SocketAddr>()?));
        if device {
            request.extensions_mut().insert(PeerCertificate::Device {
                user: Some("owner".to_owned()),
                device: "laptop".to_owned(),
            });
        }
        let response =
            tokio::time::timeout(Duration::from_secs(20), self.app.clone().oneshot(request))
                .await??;
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024).await?;
        Ok((status, bytes))
    }

    async fn record(&self) -> Result<PluginRecord, Failure> {
        let (_, bytes) = self
            .call(Method::GET, "/api/plugins/fixture", None, false)
            .await?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    async fn until(
        &self,
        what: &str,
        mut ready: impl FnMut(&PluginRecord) -> bool,
    ) -> Result<PluginRecord, Failure> {
        let mut poll = tokio::time::interval(POLL);
        for _ in 0..400 {
            poll.tick().await;
            let record = self.record().await?;
            if ready(&record) {
                return Ok(record);
            }
        }
        Err(format!("the plug-in never reached {what}").into())
    }

    async fn tool(&self, name: &str, arguments: JsonObject) -> Result<PluginToolResult, Failure> {
        let call = PluginToolCall {
            arguments,
            session_id: None,
        };
        let (status, bytes) = self
            .call(
                Method::POST,
                &format!("/api/plugins/fixture/tools/{name}"),
                Some(serde_json::to_string(&call)?),
                false,
            )
            .await?;
        assert_eq!(
            status,
            StatusCode::OK,
            "{}",
            String::from_utf8_lossy(&bytes)
        );
        Ok(serde_json::from_slice(&bytes)?)
    }

    async fn probe(&self, method: &str, path: &str) -> Result<String, Failure> {
        let answer = self
            .tool("api", JsonObject::from_serialize(&Probe { method, path })?)
            .await?
            .text;
        Ok(answer
            .split_once(' ')
            .map_or_else(|| answer.clone(), |(status, _)| status.to_owned()))
    }

    async fn probe_body(&self, method: &str, path: &str) -> Result<String, Failure> {
        Ok(self
            .tool("api", JsonObject::from_serialize(&Probe { method, path })?)
            .await?
            .text)
    }
}

fn write_manifest(source: &Path, permissions: &[Permission]) -> Result<(), Failure> {
    let text = MANIFEST.replace("COMMAND", env!("CARGO_BIN_EXE_fixture-plugin"));
    let mut manifest: PluginManifest = serde_json::from_str(&text)?;
    manifest.permissions = permissions.to_vec();
    std::fs::create_dir_all(source)?;
    std::fs::write(
        source.join("rivethub-plugin.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    Ok(())
}

async fn harness(root: &Path) -> Result<Harness, Failure> {
    let mut settings = common::settings();
    settings.state_dir = root.join("state");
    let owner = UserId::new("owner".to_owned())?;
    let tenancy = Tenancy::local(owner.clone()).with_device("laptop", owner)?;
    let host = PluginHost::open(HostSettings {
        root: root.join("state").join("plugins"),
        node: "node-a".to_owned(),
        inherited: Vec::new(),
    })
    .await?;
    let gateway = Arc::new(
        Gateway::new(Arc::new(testkit::FakeMemoryStore::default()), settings)
            .with_tenancy(tenancy)
            .with_plugins(host),
    );
    gateway::plugins::start(&gateway).await?;
    Ok(Harness {
        app: gateway.router(false),
        gateway,
    })
}

fn refused(bytes: &Bytes) -> Result<GatewayError, Failure> {
    Ok(serde_json::from_slice(bytes)?)
}

fn enable(approve: &[Permission]) -> Result<Option<String>, Failure> {
    Ok(Some(serde_json::to_string(&PluginEnableRequest {
        approve: approve.to_vec(),
    })?))
}

#[tokio::test(flavor = "multi_thread")]
async fn an_installed_plugin_is_approved_from_a_device_and_served_through_the_proxy() -> TestResult
{
    let root = tempfile::tempdir()?;
    let source = root.path().join("source");
    write_manifest(&source, &[Permission::SessionsRead, Permission::TasksRead])?;
    let host = harness(root.path()).await?;
    install_and_approve(&host, &source).await?;
    proxied_routes_are_checked(&host).await?;
    api_calls_are_scoped(&host).await?;
    the_process_is_confined_to_its_folders(&host, root.path()).await?;
    settings_are_checked_kept_and_delivered(&host).await?;
    events_and_jobs_reach_the_process(&host).await?;
    a_killed_process_is_restarted(&host).await?;
    a_wider_manifest_waits_for_approval(&host, &source).await?;
    disable_and_remove(&host, root.path()).await?;
    host.gateway.close().await;
    Ok(())
}

async fn install_and_approve(host: &Harness, source: &Path) -> TestResult {
    let install = serde_json::to_string(&PluginInstallRequest {
        path: source.display().to_string(),
    })?;
    let (status, bytes) = host
        .call(Method::POST, "/api/plugins", Some(install.clone()), false)
        .await?;
    assert_eq!(status, StatusCode::CREATED);
    let record: PluginRecord = serde_json::from_slice(&bytes)?;
    assert_eq!(record.state, PluginState::Disabled);
    assert_eq!(
        record.pending,
        [Permission::SessionsRead, Permission::TasksRead]
    );
    let (status, _) = host
        .call(Method::POST, "/api/plugins", Some(install), false)
        .await?;
    assert_eq!(status, StatusCode::CONFLICT);
    let everything = [Permission::SessionsRead, Permission::TasksRead];
    let (status, bytes) = host
        .call(
            Method::POST,
            "/api/plugins/fixture/enable",
            enable(&everything)?,
            false,
        )
        .await?;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(
        refused(&bytes)?.code.as_deref(),
        Some("device_certificate_required")
    );
    let (status, bytes) = host
        .call(
            Method::POST,
            "/api/plugins/fixture/enable",
            enable(&[Permission::SessionsRead])?,
            true,
        )
        .await?;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(
        refused(&bytes)?.code.as_deref(),
        Some("permissions_unapproved")
    );
    let (status, _) = host
        .call(
            Method::POST,
            "/api/plugins/fixture/enable",
            enable(&everything)?,
            true,
        )
        .await?;
    assert_eq!(status, StatusCode::OK);
    host.until("running", |record| record.state == PluginState::Running)
        .await?;
    Ok(())
}

async fn proxied_routes_are_checked(host: &Harness) -> TestResult {
    let post = |path: &'static str| host.call(Method::POST, path, Some("{}".into()), false);
    let (status, bytes) = post("/api/plugins/fixture/views/status").await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(serde_json::from_slice::<View>(&bytes)?.title, "Fixture");
    let (status, bytes) = post("/api/plugins/fixture/views/broken").await?;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    let broken = refused(&bytes)?;
    assert_eq!(broken.code.as_deref(), Some("plugin_invalid_response"));
    assert!(
        broken
            .detail
            .is_some_and(|detail| detail.starts_with("blocks[0]"))
    );
    let (status, bytes) = post("/api/plugins/fixture/actions/save").await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        serde_json::from_slice::<ViewActionResult>(&bytes)?
            .actions
            .len(),
        1
    );
    let (status, bytes) = post("/api/plugins/fixture/actions/refuse").await?;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        serde_json::from_slice::<ViewActionRefusal>(&bytes)?.message,
        "refused"
    );
    let (status, _) = post("/api/plugins/fixture/actions/loop").await?;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    let (status, bytes) = host
        .call(Method::GET, "/api/plugins/fixture/http/hello", None, false)
        .await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(&bytes[..], b"hello from the fixture plug-in");
    Ok(())
}

async fn api_calls_are_scoped(host: &Harness) -> TestResult {
    let own = host.probe_body("GET", "/api/plugin/self").await?;
    assert!(own.starts_with("200 "), "{own}");
    assert_eq!(host.probe("GET", "/api/agents").await?, "403");
    assert_eq!(host.probe("GET", "/api/plugins").await?, "403");
    assert_eq!(
        host.probe("POST", "/api/plugin/views/changed").await?,
        "204"
    );
    Ok(())
}

async fn the_process_is_confined_to_its_folders(host: &Harness, root: &Path) -> TestResult {
    let outside = root.join("outside.txt");
    std::fs::write(&outside, "not the plug-in's")?;
    let inside = root.join("state/plugins/fixture/data/inside.txt");
    std::fs::write(&inside, "the plug-in's own")?;
    let read = |path: &Path| -> Result<JsonObject, Failure> {
        Ok(JsonObject::from_serialize(&FileProbe { path })?)
    };
    let own = host.tool("read", read(&inside)?).await?;
    assert_eq!(own.text, "the plug-in's own");
    let (_, bytes) = host
        .call(Method::GET, "/api/plugins/fixture/logs", None, false)
        .await?;
    let logs: PluginLogs = serde_json::from_slice(&bytes)?;
    let confined = logs.lines.iter().any(|line| {
        line.stream == PluginLogStream::Host && line.text.ends_with("confined by Landlock")
    });
    let other = host.tool("read", read(&outside)?).await?;
    assert_eq!(other.is_error, confined, "{}", other.text);
    Ok(())
}

async fn settings_are_checked_kept_and_delivered(host: &Harness) -> TestResult {
    let (status, bytes) = host
        .call(Method::GET, "/api/plugins/fixture/settings", None, false)
        .await?;
    assert_eq!(status, StatusCode::OK);
    let shown: PluginSettings = serde_json::from_slice(&bytes)?;
    assert_eq!(shown.values.get("runs"), Some(&ViewValue::Number(2.0)));
    let put = |body: &str| {
        host.call(
            Method::PUT,
            "/api/plugins/fixture/settings",
            Some(body.to_owned()),
            false,
        )
    };
    let (status, bytes) = put(r#"{"values": {"runs": 9}}"#).await?;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(refused(&bytes)?.error.starts_with("runs: "));
    let (status, bytes) = put(r#"{"values": {"runs": 4, "token": "s3cret"}}"#).await?;
    assert_eq!(status, StatusCode::OK);
    let kept: PluginSettings = serde_json::from_slice(&bytes)?;
    assert_eq!(kept.values.get("runs"), Some(&ViewValue::Number(4.0)));
    assert!(!kept.values.contains_key("token"));
    assert_eq!(kept.secrets_set, ["token"]);
    let own = host.probe_body("GET", "/api/plugin/self").await?;
    assert!(own.contains(r#""token":"s3cret""#), "{own}");
    let mut delivered = String::new();
    let mut poll = tokio::time::interval(POLL);
    for _ in 0..200 {
        poll.tick().await;
        delivered = host.tool("events", JsonObject::default()).await?.text;
        if delivered.contains("settings.changed") {
            break;
        }
    }
    assert!(
        delivered.contains(r#""runs":4.0"#) || delivered.contains(r#""runs":4"#),
        "{delivered}"
    );
    Ok(())
}

async fn events_and_jobs_reach_the_process(host: &Harness) -> TestResult {
    let plugins = host.gateway.plugins().ok_or("no host")?;
    let session = SessionId::new(
        HarnessId::ClaudeCode,
        "00000000-0000-4000-8000-000000000001",
    );
    plugins.publish(&PluginEvent::SessionIdle {
        session_id: session.clone(),
    });
    plugins.publish(&PluginEvent::SessionEnded {
        session_id: session,
    });
    let mut delivered = String::new();
    let mut poll = tokio::time::interval(POLL);
    for _ in 0..200 {
        poll.tick().await;
        delivered = host.tool("events", JsonObject::default()).await?.text;
        if delivered.contains("session.idle") {
            break;
        }
    }
    assert!(delivered.contains("session.idle"), "{delivered}");
    assert!(
        !delivered.contains("session.ended"),
        "an unsubscribed kind was delivered: {delivered}"
    );
    let (status, _) = host
        .call(
            Method::POST,
            "/api/plugins/fixture/jobs/tick/run",
            None,
            false,
        )
        .await?;
    assert_eq!(status, StatusCode::ACCEPTED);
    host.until("a finished job", |record| {
        record.jobs.iter().any(|job| {
            job.last_run
                .as_ref()
                .is_some_and(|run| run.outcome == Some(PluginJobOutcome::Succeeded))
        })
    })
    .await?;
    Ok(())
}

async fn a_killed_process_is_restarted(host: &Harness) -> TestResult {
    let first: i32 = host
        .tool("pid", JsonObject::default())
        .await?
        .text
        .parse()?;
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(first),
        nix::sys::signal::Signal::SIGKILL,
    )?;
    let restarted = host
        .until("a restart", |record| {
            record.state == PluginState::Running && record.restarts == 1
        })
        .await?;
    assert_eq!(
        restarted.last_exit.and_then(|exit| exit.signal).as_deref(),
        Some("SIGKILL")
    );
    let second: i32 = host
        .tool("pid", JsonObject::default())
        .await?
        .text
        .parse()?;
    assert_ne!(first, second);
    let (_, bytes) = host
        .call(Method::GET, "/api/plugins/fixture/logs", None, false)
        .await?;
    let logs: PluginLogs = serde_json::from_slice(&bytes)?;
    assert!(
        logs.lines
            .iter()
            .any(|line| line.stream == PluginLogStream::Stdout
                && line.text.contains("/api/plugin/self answered 200 "))
    );
    assert!(
        logs.lines
            .iter()
            .any(|line| line.stream == PluginLogStream::Host && line.text == "ready")
    );
    Ok(())
}

async fn a_wider_manifest_waits_for_approval(host: &Harness, source: &Path) -> TestResult {
    write_manifest(
        source,
        &[
            Permission::SessionsRead,
            Permission::TasksRead,
            Permission::AgentsRead,
        ],
    )?;
    let (status, bytes) = host
        .call(Method::POST, "/api/plugins/fixture/reload", None, false)
        .await?;
    assert_eq!(status, StatusCode::OK);
    let reloaded: PluginRecord = serde_json::from_slice(&bytes)?;
    assert_eq!(reloaded.state, PluginState::AwaitingApproval);
    assert_eq!(reloaded.pending, [Permission::AgentsRead]);
    let (status, _) = host
        .call(
            Method::POST,
            "/api/plugins/fixture/enable",
            enable(&[Permission::AgentsRead])?,
            true,
        )
        .await?;
    assert_eq!(status, StatusCode::OK);
    host.until("running again", |record| {
        record.state == PluginState::Running
    })
    .await?;
    let agents = host.probe_body("GET", "/api/agents").await?;
    assert!(agents.starts_with("200 "), "{agents}");
    Ok(())
}

async fn disable_and_remove(host: &Harness, root: &Path) -> TestResult {
    let (status, _) = host
        .call(Method::POST, "/api/plugins/fixture/disable", None, false)
        .await?;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = host
        .call(
            Method::POST,
            "/api/plugins/fixture/views/status",
            Some("{}".into()),
            false,
        )
        .await?;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    let (status, _) = host
        .call(Method::DELETE, "/api/plugins/fixture", None, false)
        .await?;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = host
        .call(Method::GET, "/api/plugins/fixture", None, false)
        .await?;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(!root.join("state/plugins/fixture").exists());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn an_invalid_manifest_is_refused_with_its_field_path() -> TestResult {
    let root = tempfile::tempdir()?;
    let source = root.path().join("source");
    std::fs::create_dir_all(&source)?;
    std::fs::write(
        source.join("rivethub-plugin.json"),
        MANIFEST.replace(r#""tasks.read"]"#, r#""tasks.read"], "colour": "red""#),
    )?;
    let host = harness(root.path()).await?;
    let install = |path: &Path| -> Result<Option<String>, Failure> {
        Ok(Some(serde_json::to_string(&PluginInstallRequest {
            path: path.display().to_string(),
        })?))
    };
    let (status, bytes) = host
        .call(Method::POST, "/api/plugins", install(&source)?, false)
        .await?;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(refused(&bytes)?.error.contains("colour"));
    let (status, _) = host
        .call(
            Method::POST,
            "/api/plugins",
            install(Path::new("relative"))?,
            false,
        )
        .await?;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    host.gateway.close().await;
    Ok(())
}
