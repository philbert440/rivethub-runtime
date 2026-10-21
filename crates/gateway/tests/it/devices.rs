use crate::device_support;

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use device_support::CSR;
use device_support::FakeCa;
use device_support::FakeMode;
use device_support::GATEWAY_URL;
use device_support::OWNER;
use device_support::POOL;
use device_support::WG_KEY;
use device_support::WG_KEY_TWO;
use device_support::account;
use device_support::body;
use device_support::call;
use device_support::call_via;
use device_support::config;
use device_support::create_body;
use device_support::enrollment;
use device_support::expect_error;
use device_support::published_chain;
use gateway::routes::devices::DeviceAccount;
use gateway::routes::devices::DeviceRuntime;
use gateway::routes::devices::Enrollment;
use gateway::routes::devices::SetupError;
use kernel::ca::Principal;
use tempfile::TempDir;
use wire::gateway::devices::DeviceCreateResponse;
use wire::gateway::devices::DeviceEnrollResponse;
use wire::gateway::devices::DeviceListResponse;

fn enabled(dir: &Path, pool: Option<&str>, relay: Option<&str>) -> axum::Router {
    device_support::router(runtime(dir, pool, relay))
}

fn enabled_with(dir: &Path, pool: Option<&str>, fake: Arc<FakeCa>) -> (axum::Router, Arc<FakeCa>) {
    device_support::router_with(runtime(dir, pool, None), fake)
}

fn runtime(dir: &Path, pool: Option<&str>, relay: Option<&str>) -> DeviceRuntime {
    DeviceRuntime::unconfigured().enable_enrollment(enrollment(
        dir.join("mesh-devices.json"),
        pool,
        relay,
    ))
}

#[tokio::test]
async fn creates_lists_and_clips_names() {
    let dir = TempDir::new().expect("temp");
    let (app, fake) = enabled_with(
        dir.path(),
        Some(POOL),
        Arc::new(FakeCa::new(FakeMode::Ready)),
    );
    let created = call(&app, "POST", "/api/devices", "{}").await;
    assert_eq!(
        created.status,
        200,
        "{}",
        String::from_utf8_lossy(&created.body)
    );
    let created: DeviceCreateResponse = body(&created);
    assert_eq!(created.pending.name, "device");
    assert_eq!(created.pending.address, "192.0.2.1/32");
    assert_eq!(created.qr.v, 1);
    assert_eq!(created.qr.gateway, GATEWAY_URL);
    assert_eq!(created.qr.token, "issued-0");
    assert_eq!(created.qr.config.wg_address, created.pending.address);
    let issued = fake.snapshot().await;
    assert_eq!(
        issued.principals,
        vec![Principal::Device {
            user: OWNER.to_owned(),
            device: created.pending.id.clone(),
        }]
    );
    let long = "é".repeat(70);
    let named = call(&app, "POST", "/api/devices", &create_body(Some(&long))).await;
    let named: DeviceCreateResponse = body(&named);
    assert_eq!(named.pending.name.chars().count(), 64);
    assert_eq!(named.pending.address, "192.0.2.2/32");
    let empty = call(&app, "POST", "/api/devices", r#"{"name":""}"#).await;
    let empty: DeviceCreateResponse = body(&empty);
    assert_eq!(empty.pending.name, "");
    let listed = call(&app, "GET", "/api/devices", "").await;
    assert_eq!(listed.status, 200);
    let raw = String::from_utf8_lossy(&listed.body);
    assert!(!raw.contains(&created.qr.token));
    let listed: DeviceListResponse = body(&listed);
    assert_eq!(listed.devices.len(), 0);
    assert_eq!(listed.pending.len(), 3);
    assert!(!listed.relay_configured);
}

#[tokio::test]
async fn enrolls_deletes_and_rejects_bad_input() {
    let dir = TempDir::new().expect("temp");
    let app = enabled(dir.path(), Some(POOL), None);
    let created = call(&app, "POST", "/api/devices", r#"{"name":"handset"}"#).await;
    let created: DeviceCreateResponse = body(&created);
    let enrolled = call(
        &app,
        "POST",
        "/api/devices/enroll",
        &device_support::enroll_body(&created.qr.token, WG_KEY, Some("renamed")),
    )
    .await;
    assert_eq!(
        enrolled.status,
        200,
        "{}",
        String::from_utf8_lossy(&enrolled.body)
    );
    let enrolled: DeviceEnrollResponse = body(&enrolled);
    assert!(enrolled.ok);
    assert_eq!(enrolled.device.name, "renamed");
    assert_eq!(enrolled.device.address, "192.0.2.1/32");
    assert_eq!(enrolled.config.wg_address, "192.0.2.1/32");
    assert_eq!(enrolled.certificate.leaf, format!("leaf:{CSR}"));
    assert_eq!(enrolled.certificate.chain, published_chain());
    let again = call(&app, "POST", "/api/devices", "{}").await;
    let again: DeviceCreateResponse = body(&again);
    let duplicate = call(
        &app,
        "POST",
        "/api/devices/enroll",
        &device_support::enroll_body(&again.qr.token, WG_KEY, None),
    )
    .await;
    expect_error(&duplicate, 409, "this device is already enrolled");
    let fresh = call(
        &app,
        "POST",
        "/api/devices/enroll",
        &device_support::enroll_body(&again.qr.token, WG_KEY_TWO, None),
    )
    .await;
    assert_eq!(
        fresh.status,
        200,
        "{}",
        String::from_utf8_lossy(&fresh.body)
    );
}

#[tokio::test]
async fn rejects_bad_enrollment_input() {
    let dir = TempDir::new().expect("temp");
    let app = enabled(dir.path(), Some(POOL), None);
    let created: DeviceCreateResponse = body(&call(&app, "POST", "/api/devices", "{}").await);
    let enrolled = call(
        &app,
        "POST",
        "/api/devices/enroll",
        &device_support::enroll_body(&created.qr.token, WG_KEY, None),
    )
    .await;
    let enrolled: DeviceEnrollResponse = body(&enrolled);
    expect_error(
        &call(&app, "POST", "/api/devices/enroll", "{}").await,
        400,
        "token and a valid WireGuard publicKey are required",
    );
    expect_error(
        &call(
            &app,
            "POST",
            "/api/devices/enroll",
            &device_support::enroll_body("whatever", "not-a-key", None),
        )
        .await,
        400,
        "token and a valid WireGuard publicKey are required",
    );
    expect_error(
        &call(
            &app,
            "POST",
            "/api/devices/enroll",
            &device_support::enroll_body("missing-token", WG_KEY_TWO, None),
        )
        .await,
        403,
        "invalid or expired enrollment token",
    );
    let pending: DeviceCreateResponse = body(&call(&app, "POST", "/api/devices", "{}").await);
    expect_error(
        &call(
            &app,
            "POST",
            "/api/devices/enroll",
            &device_support::enroll_csr(&pending.qr.token, WG_KEY_TWO, None, None),
        )
        .await,
        400,
        "a certificate signing request is required",
    );
    let removed = call(
        &app,
        "DELETE",
        &format!("/api/devices/{}", enrolled.device.id),
        "",
    )
    .await;
    assert_eq!(removed.status, 200);
    expect_error(
        &call(&app, "DELETE", "/api/devices/missing-device", "").await,
        404,
        "unknown device",
    );
    expect_error(
        &call(&app, "DELETE", "/api/devices/has@dot", "").await,
        404,
        "not found",
    );
    expect_error(
        &call(&app, "PUT", "/api/devices", "").await,
        405,
        "method not allowed",
    );
}

#[tokio::test]
async fn exhausts_the_pool_and_reports_relay_failure() {
    let dir = TempDir::new().expect("temp");
    let tiny = enabled(dir.path(), Some("192.0.2.0/30"), None);
    assert_eq!(call(&tiny, "POST", "/api/devices", "{}").await.status, 200);
    assert_eq!(call(&tiny, "POST", "/api/devices", "{}").await.status, 200);
    expect_error(
        &call(&tiny, "POST", "/api/devices", "{}").await,
        409,
        "address pool exhausted (or RIVETOS_DEN_DEVICES_POOL unset)",
    );
    let unset = TempDir::new().expect("temp");
    let unset = enabled(unset.path(), None, None);
    expect_error(
        &call(&unset, "POST", "/api/devices", "{}").await,
        409,
        "address pool exhausted (or RIVETOS_DEN_DEVICES_POOL unset)",
    );
    let relay_dir = TempDir::new().expect("temp");
    let relay = enabled(relay_dir.path(), Some(POOL), Some("relay.example.test"));
    let created = call(&relay, "POST", "/api/devices", "{}").await;
    let created: DeviceCreateResponse = body(&created);
    assert!(
        body::<DeviceListResponse>(&call(&relay, "GET", "/api/devices", "").await).relay_configured
    );
    expect_error(
        &call(
            &relay,
            "POST",
            "/api/devices/enroll",
            &device_support::enroll_body(&created.qr.token, WG_KEY, None),
        )
        .await,
        502,
        "relay registration failed: ssh relay is not implemented",
    );
    let listed: DeviceListResponse = body(&call(&relay, "GET", "/api/devices", "").await);
    assert_eq!(listed.devices.len(), 0);
    assert_eq!(listed.pending.len(), 1);
}

#[tokio::test]
async fn refuses_a_roster_that_leaves_its_directory() {
    let dir = TempDir::new().expect("temp");
    let safe = dir.path().join("safe");
    let sibling = dir.path().join("safe-evil");
    std::fs::create_dir(&safe).expect("safe");
    std::fs::create_dir(&sibling).expect("sibling");
    let secret = sibling.join("mesh-devices.json");
    std::fs::write(
        &secret,
        br#"{"v":1,"devices":[{"id":"d1","name":"leaked","publicKey":"k","address":"192.0.2.1/32","createdAt":1}],"pending":[]}"#,
    )
    .expect("secret");
    let link = safe.join("mesh-devices.json");
    std::os::unix::fs::symlink(&secret, &link).expect("symlink");
    let app = device_support::router(DeviceRuntime::unconfigured().enable_enrollment(enrollment(
        link,
        Some(POOL),
        None,
    )));
    let listed = call(&app, "GET", "/api/devices", "").await;
    expect_error(&listed, 503, "device registry unavailable");
    assert!(!String::from_utf8_lossy(&listed.body).contains("leaked"));
    let dotted = dir.path().join("sub").join("..").join("roster.json");
    let rejected = Enrollment::try_new(
        dotted,
        Some(POOL),
        GATEWAY_URL.to_owned(),
        config(),
        None,
        account(),
    );
    assert_eq!(rejected.err(), Some(SetupError::Path));
}

#[tokio::test]
async fn rejects_enrollment_setup() {
    let dir = TempDir::new().expect("temp");
    let roster = dir.path().join("mesh-devices.json");
    assert_eq!(
        Enrollment::try_new(
            PathBuf::from("relative.json"),
            Some(POOL),
            GATEWAY_URL.to_owned(),
            config(),
            None,
            account(),
        )
        .err(),
        Some(SetupError::Path)
    );
    assert_eq!(
        Enrollment::try_new(
            roster.clone(),
            Some("192.0.2.1/24"),
            GATEWAY_URL.to_owned(),
            config(),
            None,
            account(),
        )
        .err(),
        Some(SetupError::Pool)
    );
    assert_eq!(
        Enrollment::try_new(
            roster.clone(),
            Some(POOL),
            String::new(),
            config(),
            None,
            account(),
        )
        .err(),
        Some(SetupError::GatewayUrl)
    );
    assert_eq!(
        Enrollment::try_new(
            roster.clone(),
            Some(POOL),
            GATEWAY_URL.to_owned(),
            config(),
            Some(String::new()),
            account(),
        )
        .err(),
        Some(SetupError::Relay)
    );
    assert_eq!(
        Enrollment::try_new(
            roster.clone(),
            Some(POOL),
            GATEWAY_URL.to_owned(),
            config(),
            None,
            DeviceAccount {
                pg_role: String::new(),
                owner: OWNER.to_owned(),
            },
        )
        .err(),
        Some(SetupError::Role)
    );
    assert_eq!(
        Enrollment::try_new(
            roster,
            Some(POOL),
            GATEWAY_URL.to_owned(),
            config(),
            None,
            DeviceAccount {
                pg_role: "rivet_device".to_owned(),
                owner: String::new(),
            },
        )
        .err(),
        Some(SetupError::Owner)
    );
}

#[tokio::test]
async fn remote_callers_reach_only_enroll_and_pair() {
    let dir = TempDir::new().expect("temp");
    let app = enabled(dir.path(), Some(POOL), None);
    let remote = Some("remote (source 127.0.0.2)");
    expect_error(
        &call_via(&app, "GET", "/api/devices", "", remote).await,
        401,
        "unauthorized",
    );
    expect_error(
        &call_via(&app, "POST", "/api/devices", "{}", remote).await,
        401,
        "unauthorized",
    );
    expect_error(
        &call_via(&app, "GET", "/api/devices/enroll", "", remote).await,
        401,
        "unauthorized",
    );
    expect_error(
        &call_via(&app, "POST", "/api/devices/enroll", "{}", remote).await,
        400,
        "token and a valid WireGuard publicKey are required",
    );
    let created: DeviceCreateResponse = body(&call(&app, "POST", "/api/devices", "{}").await);
    let enrolled = call_via(
        &app,
        "POST",
        "/api/devices/enroll",
        &device_support::enroll_body(&created.qr.token, WG_KEY, None),
        remote,
    )
    .await;
    assert_eq!(
        enrolled.status,
        200,
        "{}",
        String::from_utf8_lossy(&enrolled.body)
    );
    let pair = call_via(&app, "GET", "/api/devices/pair", "", remote).await;
    expect_error(&pair, 405, "POST only");
    expect_error(
        &call_via(
            &app,
            "POST",
            "/api/devices/pair",
            r#"{"token":"not-a-real-pairing-code"}"#,
            remote,
        )
        .await,
        403,
        "invalid or expired pairing code",
    );
}

#[tokio::test]
async fn an_unavailable_authority_does_not_write_the_roster() {
    let dir = TempDir::new().expect("temp");
    let (app, _) = enabled_with(
        dir.path(),
        Some(POOL),
        Arc::new(FakeCa::new(FakeMode::Unavailable)),
    );
    expect_error(
        &call(&app, "POST", "/api/devices", "{}").await,
        503,
        "device certificate authority is not usable",
    );
    let listed: DeviceListResponse = body(&call(&app, "GET", "/api/devices", "").await);
    assert_eq!(listed.devices.len(), 0);
    assert_eq!(listed.pending.len(), 0);
    let bare = device_support::router_without_authority(runtime(dir.path(), Some(POOL), None));
    expect_error(
        &call(&bare, "POST", "/api/devices", "{}").await,
        503,
        "device certificate authority is not usable",
    );
}

#[tokio::test]
async fn a_rejected_csr_leaves_the_pending_row() {
    let dir = TempDir::new().expect("temp");
    let (app, fake) = enabled_with(
        dir.path(),
        Some(POOL),
        Arc::new(FakeCa::new(FakeMode::RejectsCsr)),
    );
    let created: DeviceCreateResponse = body(&call(&app, "POST", "/api/devices", "{}").await);
    expect_error(
        &call(
            &app,
            "POST",
            "/api/devices/enroll",
            &device_support::enroll_body(&created.qr.token, WG_KEY, None),
        )
        .await,
        400,
        "certificate signing request was rejected",
    );
    let listed: DeviceListResponse = body(&call(&app, "GET", "/api/devices", "").await);
    assert_eq!(listed.devices.len(), 0);
    assert_eq!(listed.pending.len(), 1);
    let shot = fake.snapshot().await;
    assert_eq!(shot.revoked.len(), 0);
    assert_eq!(shot.csrs, vec![CSR.to_owned()]);
}

#[tokio::test]
async fn a_chain_mismatch_revokes_and_does_not_enroll() {
    let dir = TempDir::new().expect("temp");
    let (app, fake) = enabled_with(
        dir.path(),
        Some(POOL),
        Arc::new(FakeCa::new(FakeMode::SplitChain)),
    );
    let created: DeviceCreateResponse = body(&call(&app, "POST", "/api/devices", "{}").await);
    expect_error(
        &call(
            &app,
            "POST",
            "/api/devices/enroll",
            &device_support::enroll_body(&created.qr.token, WG_KEY, None),
        )
        .await,
        503,
        "device certificate authority is not usable",
    );
    let listed: DeviceListResponse = body(&call(&app, "GET", "/api/devices", "").await);
    assert_eq!(listed.devices.len(), 0);
    assert_eq!(listed.pending.len(), 1);
    let shot = fake.snapshot().await;
    assert_eq!(
        shot.revoked,
        vec![Principal::Device {
            user: OWNER.to_owned(),
            device: created.pending.id,
        }]
    );
}
