use axum::body::Body;
use axum::http::{Method, Request};

use wire::plugin::manifest::Permission;

use crate::plugin_support::{TestResult, harness, write_manifest};

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
