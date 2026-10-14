mod support;

use axum::{Json, Router, routing::get};
use cli::Exit;
use serde::Serialize;
use support::{config, environment, home, run};
use wire::gateway::HealthzResponse;

#[derive(Serialize)]
struct Health {
    #[serde(flatten)]
    response: HealthzResponse,
    version: &'static str,
    uptime: f64,
}

#[tokio::test]
async fn status_reads_health_and_optional_version_and_uptime() {
    let stub = testkit::stub_server(Router::new().route(
        "/healthz",
        get(|| async {
            Json(Health {
                response: HealthzResponse {
                    ok: true,
                    sessions: 0,
                    name: "host".to_owned(),
                    node: "node".to_owned(),
                },
                version: "1.2.3",
                uptime: 12.5,
            })
        }),
    ))
    .await
    .expect("health route");
    let home = home();
    let env = environment(&home);
    config(
        &home,
        &format!("den:\n  host: 0.0.0.0\n  port: {}\n", stub.addr.port()),
    );
    let status = run(&["status"], &env).await;
    assert_eq!(status.exit, Exit::Ok);
    assert_eq!(
        status.stdout,
        "RivetHub is running (v1.2.3); uptime 12.5s\n"
    );
    assert_eq!(status.stderr, "");
    stub.close().await;
    let unavailable = run(&["status"], &env).await;
    assert_eq!(unavailable.exit, Exit::Ok);
    assert!(unavailable.stdout.contains("not running"));
    assert!(unavailable.stderr.contains("Health unavailable"));
}

#[tokio::test]
async fn malformed_health_falls_back_with_a_diagnostic() {
    let stub = testkit::stub_server(Router::new().route("/healthz", get(|| async { "not JSON" })))
        .await
        .expect("health route");
    let home = home();
    let env = environment(&home);
    config(
        &home,
        &format!("den:\n  host: 127.0.0.1\n  port: {}\n", stub.addr.port()),
    );
    let status = run(&["status"], &env).await;
    assert_eq!(status.exit, Exit::Ok);
    assert!(status.stderr.contains("Health unavailable"));
    assert!(status.stdout.contains("not running"));
    stub.close().await;
}

#[tokio::test]
async fn ordinary_redirects_reach_the_health_route() {
    let stub = testkit::stub_server(
        Router::new()
            .route(
                "/healthz",
                get(|| async { axum::response::Redirect::temporary("/ready") }),
            )
            .route(
                "/ready",
                get(|| async {
                    Json(HealthzResponse {
                        ok: true,
                        sessions: 0,
                        name: "host".to_owned(),
                        node: "node".to_owned(),
                    })
                }),
            ),
    )
    .await
    .expect("redirect server");
    let home = home();
    let env = environment(&home);
    config(
        &home,
        &format!("den:\n  host: 127.0.0.1\n  port: {}\n", stub.addr.port()),
    );
    let report = run(&["status"], &env).await;
    assert_eq!(report.exit, Exit::Ok);
    assert_eq!(report.stdout, "RivetHub is running\n");
    assert_eq!(report.stderr, "");
    stub.close().await;
}

#[tokio::test]
async fn stalled_health_request_times_out_after_three_seconds() {
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::Notify;
    use tokio_util::sync::CancellationToken;

    let reached = Arc::new(Notify::new());
    let release = CancellationToken::new();
    let handler_reached = Arc::clone(&reached);
    let handler_release = release.clone();
    let stub = testkit::stub_server(Router::new().route(
        "/healthz",
        get(move || {
            let reached = Arc::clone(&handler_reached);
            let release = handler_release.clone();
            async move {
                reached.notify_one();
                release.cancelled().await;
                "late"
            }
        }),
    ))
    .await
    .expect("stalled server");
    let home = home();
    let env = environment(&home);
    config(
        &home,
        &format!("den:\n  host: 127.0.0.1\n  port: {}\n", stub.addr.port()),
    );
    let request = tokio::spawn(async move { run(&["status"], &env).await });
    tokio::time::timeout(Duration::from_secs(5), reached.notified())
        .await
        .expect("request arrived");
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(3)).await;
    let report = tokio::time::timeout(Duration::from_secs(1), request)
        .await
        .expect("health deadline")
        .expect("status task");
    assert_eq!(report.exit, Exit::Ok);
    assert!(report.stderr.contains("Health unavailable"));
    assert!(report.stdout.contains("not running"));
    release.cancel();
    stub.close().await;
}
