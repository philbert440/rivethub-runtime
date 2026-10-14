#![expect(
    clippy::expect_used,
    reason = "helpers sit outside #[test] functions, the only place clippy.toml exempts expect"
)]
#![expect(
    clippy::panic,
    reason = "a helper that cannot build the fixture stops the test"
)]

mod common;

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use gateway::Gateway;
use gateway::routes::devices::DeviceRuntime;
use gateway::routes::devices::Enrollment;
use gateway::routes::devices::PhoneIssuer;
use gateway::routes::devices::SetupError;
use kernel::memory::MemoryStore;
use tempfile::TempDir;
use testkit::Request;
use testkit::Response;
use wire::gateway::GatewayError;
use wire::gateway::devices::DeviceCreateResponse;
use wire::gateway::devices::DeviceEnrollConfig;
use wire::gateway::devices::DeviceEnrollResponse;
use wire::gateway::devices::DeviceListResponse;
use wire::gateway::devices::PhonePairingAvailability;
use wire::gateway::devices::PhonePairingCode;
use wire::gateway::devices::PhonePairingState;
use wire::gateway::devices::PhonePairingStatus;

const WG_KEY: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
const WG_KEY_TWO: &str = "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB=";
const POOL: &str = "192.0.2.0/29";
const GATEWAY_URL: &str = "https://hub.example.test";

fn config() -> DeviceEnrollConfig {
    DeviceEnrollConfig {
        shared_host: "files.example.test".to_owned(),
        shared_export: "/srv/share".to_owned(),
        pg_url: "postgres://device@db/rivet".to_owned(),
        embed_url: "http://127.0.0.1:8080".to_owned(),
        wg_endpoint: "wg.example.test:51820".to_owned(),
        wg_peer_public_key: WG_KEY.to_owned(),
        wg_address: "192.0.2.1/32".to_owned(),
        wg_allowed_ips: "192.0.2.0/24".to_owned(),
        home_subnet: "198.51.100.0/24".to_owned(),
    }
}

fn enrollment(roster: PathBuf, pool: Option<&str>, relay: Option<&str>) -> Enrollment {
    Enrollment::try_new(
        roster,
        pool,
        GATEWAY_URL.to_owned(),
        config(),
        relay.map(str::to_owned),
        "rivet_device".to_owned(),
    )
    .unwrap_or_else(|error| panic!("enrollment setup: {error}"))
}

fn router(runtime: DeviceRuntime) -> axum::Router {
    let store: Arc<dyn MemoryStore> = Arc::new(testkit::FakeMemoryStore::default());
    let gateway = Gateway::new(store, common::settings()).with_devices(runtime);
    Arc::new(gateway).router(false)
}

fn enabled(dir: &Path, pool: Option<&str>, relay: Option<&str>) -> axum::Router {
    let runtime = DeviceRuntime::unconfigured().enable_enrollment(enrollment(
        dir.join("mesh-devices.json"),
        pool,
        relay,
    ));
    router(runtime)
}

fn request(method: &str, target: &str, body: &str, via: Option<&str>) -> Request {
    let mut headers = vec![("Host".to_owned(), "127.0.0.1:5174".to_owned())];
    if !body.is_empty() {
        headers.push(("Content-Type".to_owned(), "application/json".to_owned()));
    }
    Request {
        method: method.to_owned(),
        target: target.to_owned(),
        headers,
        body: body.as_bytes().to_vec(),
        recipe: None,
        via: via.map(str::to_owned),
    }
}

async fn call(app: &axum::Router, method: &str, target: &str, body: &str) -> Response {
    call_via(app, method, target, body, None).await
}

async fn call_via(
    app: &axum::Router,
    method: &str,
    target: &str,
    body: &str,
    via: Option<&str>,
) -> Response {
    common::answer(app.clone(), request(method, target, body, via))
        .await
        .unwrap_or_else(|error| panic!("{method} {target}: {error}"))
}

fn body<T: serde::de::DeserializeOwned>(response: &Response) -> T {
    serde_json::from_slice(&response.body)
        .unwrap_or_else(|error| panic!("{error}: {}", String::from_utf8_lossy(&response.body)))
}

fn create_body(name: Option<&str>) -> String {
    serde_json::to_string(&wire::gateway::devices::DeviceCreateRequest {
        name: name.map(str::to_owned),
    })
    .expect("create body")
}

fn enroll_body(token: &str, public_key: &str, name: Option<&str>) -> String {
    serde_json::to_string(&wire::gateway::devices::DeviceEnrollRequest {
        token: Some(token.to_owned()),
        public_key: Some(public_key.to_owned()),
        name: name.map(str::to_owned),
    })
    .expect("enroll body")
}

fn pair_body(token: &str) -> String {
    serde_json::to_string(&wire::gateway::PairRedeemRequest {
        token: token.to_owned(),
    })
    .expect("pair body")
}

fn expect_error(response: &Response, status: u16, message: &str) {
    assert_eq!(
        response.status,
        status,
        "{}",
        String::from_utf8_lossy(&response.body)
    );
    let parsed: GatewayError = body(response);
    assert_eq!(parsed, GatewayError::new(message));
}

fn write_ca(dir: &Path) {
    let root_key = rcgen::KeyPair::generate().expect("root key");
    let mut root_params = rcgen::CertificateParams::default();
    root_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    root_params.key_usages = vec![
        rcgen::KeyUsagePurpose::KeyCertSign,
        rcgen::KeyUsagePurpose::CrlSign,
    ];
    let mut root_name = rcgen::DistinguishedName::new();
    root_name.push(rcgen::DnType::CommonName, "RivetHub Test Root");
    root_params.distinguished_name = root_name;
    let root = root_params
        .self_signed(&root_key)
        .expect("root certificate");
    let intermediate_key = rcgen::KeyPair::generate().expect("intermediate key");
    let mut params = rcgen::CertificateParams::default();
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    params.key_usages = vec![
        rcgen::KeyUsagePurpose::KeyCertSign,
        rcgen::KeyUsagePurpose::CrlSign,
    ];
    let mut name = rcgen::DistinguishedName::new();
    name.push(rcgen::DnType::CommonName, "RivetHub Test Intermediate");
    name.push(rcgen::DnType::OrganizationalUnitName, "ca");
    params.distinguished_name = name;
    let intermediate = params
        .signed_by(&intermediate_key, &root, &root_key)
        .expect("intermediate certificate");
    let intermediate_dir = dir.join("intermediate");
    let root_dir = dir.join("root");
    std::fs::create_dir_all(&intermediate_dir).expect("intermediate dir");
    std::fs::create_dir_all(&root_dir).expect("root dir");
    std::fs::write(
        intermediate_dir.join("key.pem"),
        intermediate_key.serialize_pem(),
    )
    .expect("intermediate key");
    std::fs::write(intermediate_dir.join("cert.pem"), intermediate.pem()).expect("intermediate");
    std::fs::write(
        intermediate_dir.join("chain.pem"),
        format!("{}{}", intermediate.pem(), root.pem()),
    )
    .expect("chain");
    std::fs::write(root_dir.join("cert.pem"), root.pem()).expect("root cert");
}

#[tokio::test]
async fn creates_lists_and_clips_names() {
    let dir = TempDir::new().expect("temp");
    let app = enabled(dir.path(), Some(POOL), None);
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
    assert_eq!(created.qr.config.wg_address, created.pending.address);
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
        &enroll_body(&created.qr.token, WG_KEY, Some("renamed")),
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
    let again = call(&app, "POST", "/api/devices", "{}").await;
    let again: DeviceCreateResponse = body(&again);
    let duplicate = call(
        &app,
        "POST",
        "/api/devices/enroll",
        &enroll_body(&again.qr.token, WG_KEY, None),
    )
    .await;
    expect_error(&duplicate, 409, "this device is already enrolled");
    let fresh = call(
        &app,
        "POST",
        "/api/devices/enroll",
        &enroll_body(&again.qr.token, WG_KEY_TWO, None),
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
        &enroll_body(&created.qr.token, WG_KEY, None),
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
            &enroll_body("whatever", "not-a-key", None),
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
            &enroll_body("missing-token", WG_KEY_TWO, None),
        )
        .await,
        403,
        "invalid or expired enrollment token",
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
        &call(&app, "DELETE", "/api/devices/has.dot", "").await,
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
            &enroll_body(&created.qr.token, WG_KEY, None),
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
    let app = enabled_at(link, Some(POOL));
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
        "rivet_device".to_owned(),
    );
    assert_eq!(rejected.err(), Some(SetupError::Path));
}

fn enabled_at(roster: PathBuf, pool: Option<&str>) -> axum::Router {
    router(DeviceRuntime::unconfigured().enable_enrollment(enrollment(roster, pool, None)))
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
            "rivet_device".to_owned(),
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
            "rivet_device".to_owned(),
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
            "rivet_device".to_owned(),
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
            "rivet_device".to_owned(),
        )
        .err(),
        Some(SetupError::Relay)
    );
    assert_eq!(
        Enrollment::try_new(
            roster,
            Some(POOL),
            GATEWAY_URL.to_owned(),
            config(),
            None,
            String::new(),
        )
        .err(),
        Some(SetupError::Role)
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
        &enroll_body(&created.qr.token, WG_KEY, None),
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

fn phone_app(pairing: &Path, ca: &Path) -> axum::Router {
    let issuer = PhoneIssuer::try_new(pairing.to_owned(), ca.to_owned(), GATEWAY_URL.to_owned())
        .unwrap_or_else(|error| panic!("phone issuer: {error}"));
    router(DeviceRuntime::unconfigured().enable_phone_issuer(issuer))
}

#[tokio::test]
async fn issues_and_redeems_a_phone_certificate() {
    let dir = TempDir::new().expect("temp");
    let ca = dir.path().join("rivet-ca");
    write_ca(&ca);
    let pairing = dir.path().join("pairing");
    let app = phone_app(&pairing, &ca);
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
    assert_eq!(started.reshown, 0);
    assert!(!started.added_to_users);
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
    let redeemed: wire::gateway::devices::PairRedeemResponse = body(&redeemed);
    assert_eq!(redeemed.device_id, started.device_id);
    assert_ne!(redeemed.p12, "");
    assert_ne!(redeemed.passphrase, "");
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
}

#[tokio::test]
async fn refuses_a_pairing_file_that_leaves_its_directory() {
    let dir = TempDir::new().expect("temp");
    let ca = dir.path().join("rivet-ca");
    write_ca(&ca);
    let pairing = dir.path().join("pairing");
    let app = phone_app(&pairing, &ca);
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
    std::fs::write(&outside, br#"{"v":1,"passphrase":"LEAKED"}"#).expect("outside");
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
        br#"{"v":1,"passphrase":"LEAKED"}"#
    );
}

#[tokio::test]
async fn reports_a_missing_certificate_and_a_bad_name() {
    let dir = TempDir::new().expect("temp");
    let ca = dir.path().join("rivet-ca");
    write_ca(&ca);
    let pairing = dir.path().join("pairing");
    let app = phone_app(&pairing, &ca);
    let started: PhonePairingCode =
        body(&call(&app, "POST", "/api/phone-pairing", r#"{"name":"pixel"}"#).await);
    let certs = pairing.join("certs");
    for entry in std::fs::read_dir(&certs).expect("certs") {
        let path = entry.expect("cert entry").path();
        std::fs::remove_file(&path).expect("remove cert");
    }
    expect_error(
        &call(
            &app,
            "POST",
            "/api/devices/pair",
            &pair_body(&started.qr_text),
        )
        .await,
        410,
        "device certificate is gone — pair again",
    );
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
async fn phone_gates_fail_closed() {
    let off = router(DeviceRuntime::unconfigured().phone_off());
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
    let owner = router(DeviceRuntime::unconfigured().phone_not_owner());
    expect_error(
        &call(&owner, "GET", "/api/phone-pairing/%E0%A4%A", "").await,
        403,
        "only the owner can pair a phone",
    );
    let dir = TempDir::new().expect("temp");
    let empty = dir.path().join("empty-ca");
    std::fs::create_dir(&empty).expect("empty ca");
    let broken = phone_app(&dir.path().join("pairing"), &empty);
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
    let disabled = router(DeviceRuntime::unconfigured());
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
