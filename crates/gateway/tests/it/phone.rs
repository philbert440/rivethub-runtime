use crate::device_support;

use std::path::Path;
use std::sync::Arc;

use device_support::CSR;
use device_support::FakeCa;
use device_support::FakeMode;
use device_support::GATEWAY_URL;
use device_support::OWNER;
use device_support::body;
use device_support::call;
use device_support::expect_error;
use device_support::pair_body;
use device_support::pair_csr;
use device_support::published_chain;
use gateway::routes::devices::DeviceRuntime;
use gateway::routes::devices::PhoneIssuer;
use gateway::routes::devices::SetupError;
use kernel::ca::Principal;
use tempfile::TempDir;
use wire::gateway::devices::PairRedeemResponse;
use wire::gateway::devices::PhonePairingAvailability;
use wire::gateway::devices::PhonePairingCode;
use wire::gateway::devices::PhonePairingState;
use wire::gateway::devices::PhonePairingStatus;

#[expect(
    clippy::panic,
    reason = "a helper that cannot build the fixture stops the test"
)]
fn issuer(pairing: &Path) -> PhoneIssuer {
    PhoneIssuer::try_new(pairing.to_owned(), GATEWAY_URL.to_owned(), OWNER.to_owned())
        .unwrap_or_else(|error| panic!("phone issuer: {error}"))
}

fn phone_app(pairing: &Path, fake: Arc<FakeCa>) -> (axum::Router, Arc<FakeCa>) {
    device_support::router_with(
        DeviceRuntime::unconfigured().enable_phone_issuer(issuer(pairing)),
        fake,
    )
}

#[tokio::test]
async fn issues_and_redeems_a_phone_certificate() {
    let dir = TempDir::new().expect("temp");
    let pairing = dir.path().join("pairing");
    let (app, fake) = phone_app(&pairing, Arc::new(FakeCa::new(FakeMode::Ready)));
    let availability: PhonePairingAvailability =
        body(&call(&app, "GET", "/api/phone-pairing", "").await);
    assert!(availability.available);
    assert_eq!(availability.gateway.as_deref(), Some(GATEWAY_URL));
    let started = call(&app, "POST", "/api/phone-pairing", r#"{"name":"pixel"}"#).await;
    assert_eq!(
        started.status,
        200,
        "{}",
        String::from_utf8_lossy(&started.body)
    );
    let started: PhonePairingCode = body(&started);
    assert_eq!(started.gateway, GATEWAY_URL);
    assert_eq!(started.qr_text, "issued-0");
    assert_eq!(started.reshown, 0);
    assert!(!started.added_to_users);
    assert!(!pairing.join("certs").exists());
    let pending: PhonePairingStatus =
        body(&call(&app, "GET", "/api/phone-pairing/pixel", "").await);
    assert_eq!(pending.state, PhonePairingState::Pending);
    let redeemed = call(
        &app,
        "POST",
        "/api/devices/pair",
        &pair_body(&started.qr_text),
    )
    .await;
    assert_eq!(
        redeemed.status,
        200,
        "{}",
        String::from_utf8_lossy(&redeemed.body)
    );
    let redeemed: PairRedeemResponse = body(&redeemed);
    assert_eq!(redeemed.device_id, started.device_id);
    assert_eq!(redeemed.certificate.leaf, format!("leaf:{CSR}"));
    assert_eq!(redeemed.certificate.chain, published_chain());
    assert!(!pairing.join("certs").exists());
    let record = std::fs::read_dir(pairing.join("claimed"))
        .expect("claimed")
        .next()
        .expect("one record")
        .expect("entry")
        .path();
    let stored = std::fs::read_to_string(&record).expect("record");
    assert!(!stored.contains("p12"));
    assert!(!stored.contains("passphrase"));
    let paired: PhonePairingStatus = body(&call(&app, "GET", "/api/phone-pairing/pixel", "").await);
    assert_eq!(paired.state, PhonePairingState::Paired);
    expect_error(
        &call(
            &app,
            "POST",
            "/api/devices/pair",
            &pair_body(&started.qr_text),
        )
        .await,
        403,
        "invalid or expired pairing code",
    );
    let shot = fake.snapshot().await;
    assert_eq!(shot.signed_tokens, vec![started.qr_text]);
    assert_eq!(
        shot.principals,
        vec![Principal::Device {
            user: OWNER.to_owned(),
            device: started.device_id,
        }]
    );
    assert_eq!(shot.revoked.len(), 0);
}

#[tokio::test]
async fn refuses_a_pairing_file_that_leaves_its_directory() {
    let dir = TempDir::new().expect("temp");
    let pairing = dir.path().join("pairing");
    let (app, _) = phone_app(&pairing, Arc::new(FakeCa::new(FakeMode::Ready)));
    let started: PhonePairingCode =
        body(&call(&app, "POST", "/api/phone-pairing", r#"{"name":"pixel"}"#).await);
    let pending = pairing.join("pending");
    let record = std::fs::read_dir(&pending)
        .expect("pending")
        .next()
        .expect("one record")
        .expect("entry")
        .path();
    let outside = dir.path().join("outside.json");
    std::fs::write(&outside, br#"{"v":1,"name":"LEAKED"}"#).expect("outside");
    std::fs::remove_file(&record).expect("unlink record");
    std::os::unix::fs::symlink(&outside, &record).expect("escape symlink");
    let redeemed = call(
        &app,
        "POST",
        "/api/devices/pair",
        &pair_body(&started.qr_text),
    )
    .await;
    expect_error(&redeemed, 403, "invalid or expired pairing code");
    assert!(!String::from_utf8_lossy(&redeemed.body).contains("LEAKED"));
    assert_eq!(
        std::fs::read(&outside).expect("outside intact"),
        br#"{"v":1,"name":"LEAKED"}"#
    );
}

#[tokio::test]
async fn rejects_a_redeem_without_a_csr_and_a_bad_name() {
    let dir = TempDir::new().expect("temp");
    let pairing = dir.path().join("pairing");
    let (app, fake) = phone_app(&pairing, Arc::new(FakeCa::new(FakeMode::Ready)));
    let started: PhonePairingCode =
        body(&call(&app, "POST", "/api/phone-pairing", r#"{"name":"pixel"}"#).await);
    expect_error(
        &call(
            &app,
            "POST",
            "/api/devices/pair",
            &pair_csr(&started.qr_text, None),
        )
        .await,
        400,
        "a certificate signing request is required",
    );
    let pending: PhonePairingStatus =
        body(&call(&app, "GET", "/api/phone-pairing/pixel", "").await);
    assert_eq!(pending.state, PhonePairingState::Pending);
    assert_eq!(fake.snapshot().await.signed_tokens.len(), 0);
    expect_error(
        &call(&app, "POST", "/api/phone-pairing", r#"{"name":"bad name"}"#).await,
        400,
        "name may only use letters, digits, \".\", \"_\" and \"-\"",
    );
    expect_error(
        &call(
            &app,
            "POST",
            "/api/phone-pairing",
            r#"{"name":"pixel","host":"bad host"}"#,
        )
        .await,
        400,
        "host must be an IP address or a host name",
    );
    expect_error(
        &call(&app, "GET", "/api/phone-pairing/%E0%A4%A", "").await,
        400,
        "malformed device name",
    );
    expect_error(
        &call(&app, "GET", "/api/phone-pairing/bad%20name", "").await,
        404,
        "not found",
    );
    let dotted: PhonePairingStatus =
        body(&call(&app, "GET", "/api/phone-pairing/%2e%2e", "").await);
    assert_eq!(dotted.state, PhonePairingState::Expired);
    expect_error(
        &call(&app, "GET", "/api/phone-pairing/%2e%2e%2f%2e%2e", "").await,
        404,
        "not found",
    );
}

#[tokio::test]
async fn a_lost_record_revokes_the_signed_certificate() {
    let dir = TempDir::new().expect("temp");
    let pairing = dir.path().join("pairing");
    let (app, fake) = phone_app(&pairing, Arc::new(FakeCa::new(FakeMode::Ready)));
    let started: PhonePairingCode =
        body(&call(&app, "POST", "/api/phone-pairing", r#"{"name":"pixel"}"#).await);
    let pending = pairing.join("pending");
    let record = std::fs::read_dir(&pending)
        .expect("pending")
        .next()
        .expect("one record")
        .expect("entry")
        .path();
    fake.sabotage(record).await;
    expect_error(
        &call(
            &app,
            "POST",
            "/api/devices/pair",
            &pair_body(&started.qr_text),
        )
        .await,
        403,
        "invalid or expired pairing code",
    );
    let status: PhonePairingStatus = body(&call(&app, "GET", "/api/phone-pairing/pixel", "").await);
    assert_eq!(status.state, PhonePairingState::Expired);
    let shot = fake.snapshot().await;
    assert_eq!(
        shot.revoked,
        vec![Principal::Device {
            user: OWNER.to_owned(),
            device: started.device_id,
        }]
    );
}

#[tokio::test]
async fn phone_gates_fail_closed() {
    let off = device_support::router(DeviceRuntime::unconfigured().phone_off());
    expect_error(
        &call(&off, "GET", "/api/phone-pairing", "").await,
        503,
        "phone pairing is off on this node",
    );
    expect_error(
        &call(&off, "POST", "/api/phone-pairing", r#"{"name":"pixel"}"#).await,
        503,
        "phone pairing is off on this node",
    );
    let owner = device_support::router(DeviceRuntime::unconfigured().phone_not_owner());
    expect_error(
        &call(&owner, "GET", "/api/phone-pairing/%E0%A4%A", "").await,
        403,
        "only the owner can pair a phone",
    );
    let dir = TempDir::new().expect("temp");
    let pairing = dir.path().join("pairing");
    let (broken, _) = phone_app(&pairing, Arc::new(FakeCa::new(FakeMode::Unavailable)));
    let availability: PhonePairingAvailability =
        body(&call(&broken, "GET", "/api/phone-pairing", "").await);
    assert!(!availability.available);
    assert_eq!(
        availability.reason.as_deref(),
        Some("device certificate authority is not usable")
    );
    expect_error(
        &call(&broken, "POST", "/api/phone-pairing", r#"{"name":"pixel"}"#).await,
        503,
        "device certificate authority is not usable",
    );
    let bare = device_support::router_without_authority(
        DeviceRuntime::unconfigured().enable_phone_issuer(issuer(&pairing)),
    );
    expect_error(
        &call(&bare, "POST", "/api/phone-pairing", r#"{"name":"pixel"}"#).await,
        503,
        "device certificate authority is not usable",
    );
    assert_eq!(
        PhoneIssuer::try_new(pairing, GATEWAY_URL.to_owned(), String::new()).err(),
        Some(SetupError::Owner)
    );
    let disabled = device_support::router(DeviceRuntime::unconfigured());
    expect_error(
        &call(&disabled, "GET", "/api/devices", "").await,
        503,
        "device enrollment disabled on this node",
    );
    expect_error(
        &call(&disabled, "PUT", "/api/devices/x", "").await,
        503,
        "device enrollment disabled on this node",
    );
}

#[tokio::test]
async fn an_unsafe_token_is_not_written_as_a_path() {
    let dir = TempDir::new().expect("temp");
    let pairing = dir.path().join("pairing");
    let (app, fake) = phone_app(
        &pairing,
        Arc::new(FakeCa::new(FakeMode::Ready).token("../escape")),
    );
    expect_error(
        &call(&app, "POST", "/api/phone-pairing", r#"{"name":"pixel"}"#).await,
        503,
        "device certificate authority is not usable",
    );
    assert!(!pairing.join("escape.json").exists());
    assert!(!dir.path().join("escape.json").exists());
    assert_eq!(fake.snapshot().await.revoked.len(), 1);
    let status: PhonePairingStatus = body(&call(&app, "GET", "/api/phone-pairing/pixel", "").await);
    assert_eq!(status.state, PhonePairingState::Expired);
}

#[tokio::test]
async fn a_token_as_long_as_a_step_ca_one_time_token_still_pairs() {
    let dir = TempDir::new().expect("temp");
    let pairing = dir.path().join("pairing");
    let token = format!("{}.{}.{}", "h".repeat(36), "p".repeat(548), "s".repeat(86));
    let (app, fake) = phone_app(
        &pairing,
        Arc::new(FakeCa::new(FakeMode::Ready).token(token.clone())),
    );
    let started = call(&app, "POST", "/api/phone-pairing", r#"{"name":"pixel"}"#).await;
    assert_eq!(
        started.status,
        200,
        "{}",
        String::from_utf8_lossy(&started.body)
    );
    let started: PhonePairingCode = body(&started);
    assert_eq!(started.qr_text, token);
    let redeemed = call(&app, "POST", "/api/devices/pair", &pair_body(&token)).await;
    assert_eq!(
        redeemed.status,
        200,
        "{}",
        String::from_utf8_lossy(&redeemed.body)
    );
    let redeemed: PairRedeemResponse = body(&redeemed);
    assert_eq!(redeemed.device_id, started.device_id);
    assert_eq!(fake.snapshot().await.revoked.len(), 0);
}

#[tokio::test]
async fn status_answers_for_the_device_id_the_code_carries() {
    let dir = TempDir::new().expect("temp");
    let pairing = dir.path().join("pairing");
    let (app, _) = phone_app(&pairing, Arc::new(FakeCa::new(FakeMode::Ready)));
    let started: PhonePairingCode =
        body(&call(&app, "POST", "/api/phone-pairing", r#"{"name":"pixel"}"#).await);
    let target = format!("/api/phone-pairing/{}", started.device_id);
    let pending: PhonePairingStatus = body(&call(&app, "GET", &target, "").await);
    assert_eq!(pending.state, PhonePairingState::Pending);
    let redeemed = call(
        &app,
        "POST",
        "/api/devices/pair",
        &pair_body(&started.qr_text),
    )
    .await;
    assert_eq!(redeemed.status, 200);
    let paired: PhonePairingStatus = body(&call(&app, "GET", &target, "").await);
    assert_eq!(paired.state, PhonePairingState::Paired);
    let other: PhonePairingStatus = body(
        &call(
            &app,
            "GET",
            "/api/phone-pairing/d00000000000000000000000000000000",
            "",
        )
        .await,
    );
    assert_eq!(other.state, PhonePairingState::Expired);
}
