use std::sync::Arc;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use wire::plugin::host::{PluginLogs, PluginState};
use wire::plugin::manifest::{Permission, PluginManifest};

use crate::plugin_support::{
    Harness, POLL, TestResult, enable, harness, refused, write_manifest, write_manifest_with,
};

const BASE: [Permission; 2] = [Permission::SessionsRead, Permission::TasksRead];

fn get(path: &str) -> Result<Request<Body>, axum::http::Error> {
    Request::builder()
        .method(Method::GET)
        .uri(path)
        .header("host", "127.0.0.1:5174")
        .body(Body::empty())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_plugin_page_is_kept_off_the_gateway_origin() -> TestResult {
    let root = tempfile::tempdir()?;
    let source = root.path().join("source");
    write_manifest(&source, &BASE)?;
    let host = harness(root.path()).await?;
    host.install_and_enable(&source).await?;
    for path in [
        "/api/plugins/fixture/http/hello",
        "/api/plugins/fixture/panel/index.html",
    ] {
        let answer = host.send(get(path)?).await?;
        let sandboxed = answer
            .headers
            .get_all("content-security-policy")
            .iter()
            .filter_map(|value| value.to_str().ok())
            .any(|policy| policy.starts_with("sandbox ") && !policy.contains("allow-same-origin"));
        assert!(sandboxed, "{path}: {:?}", answer.headers);
        assert_eq!(
            answer
                .headers
                .get("x-content-type-options")
                .map(axum::http::HeaderValue::as_bytes),
            Some(&b"nosniff"[..]),
            "{path}"
        );
    }
    host.gateway.close().await;
    Ok(())
}

async fn stopping(host: &Harness) -> TestResult {
    let mut poll = tokio::time::interval(POLL);
    for _ in 0..200 {
        poll.tick().await;
        let (_, bytes) = host
            .call(Method::GET, "/api/plugins/fixture/logs", None, false)
            .await?;
        let logs: PluginLogs = serde_json::from_slice(&bytes)?;
        if logs
            .lines
            .iter()
            .any(|line| line.text.contains("ignored SIGTERM"))
        {
            return Ok(());
        }
    }
    Err("the reload never stopped the plug-in".into())
}

fn slow_to_stop(manifest: &mut PluginManifest) {
    manifest
        .run
        .env
        .insert("FIXTURE_IGNORE_SIGTERM".to_owned(), "1".to_owned());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reload_queued_ahead_of_enable_cannot_slip_a_permission_past_the_device_check()
-> TestResult {
    let root = tempfile::tempdir()?;
    let source = root.path().join("source");
    write_manifest_with(&source, &BASE, slow_to_stop)?;
    let host = Arc::new(harness(root.path()).await?);
    host.install_and_enable(&source).await?;
    let wider = [BASE[0], BASE[1], Permission::AgentsRead];
    write_manifest_with(&source, &wider, slow_to_stop)?;
    let reloading = tokio::spawn({
        let host = Arc::clone(&host);
        async move {
            host.call(Method::POST, "/api/plugins/fixture/reload", None, false)
                .await
        }
    });
    stopping(&host).await?;
    let (status, bytes) = host
        .call(
            Method::POST,
            "/api/plugins/fixture/enable",
            enable(&[Permission::AgentsRead])?,
            false,
        )
        .await?;
    let (reloaded, _) = reloading.await??;
    assert_eq!(reloaded, StatusCode::OK);
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    assert_eq!(
        refused(&bytes)?.code.as_deref(),
        Some("device_certificate_required")
    );
    let record = host.record().await?;
    assert!(!record.granted.contains(&Permission::AgentsRead));
    assert_eq!(record.state, PluginState::AwaitingApproval);
    host.gateway.close().await;
    Ok(())
}
