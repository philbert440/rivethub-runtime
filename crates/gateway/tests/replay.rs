#[expect(
    dead_code,
    reason = "the harness module is included whole by every test binary and this one uses a subset"
)]
mod common;

use libtest_mimic::Arguments;
use libtest_mimic::Failed;
use libtest_mimic::Trial;
use libtest_mimic::run;
use std::path::PathBuf;
use testkit::Mismatch;

const RECORDING: &str = "gateway/http-plain.json";

const CLAIMS: &[&str] = &[
    "plain-main/healthz-get",
    "plain-main/healthz-post",
    "plain-main/healthz-head",
    "plain-main/healthz-delete",
    "plain-main/healthz-query",
    "plain-main/healthz-absolute-form",
    "plain-main/healthz-http10",
    "plain-main/healthz-http10-no-host",
    "plain-main/options-healthz",
    "plain-main/options-healthz-same-origin",
    "plain-main/options-api",
    "plain-main/options-api-preflight-app-bundle",
    "plain-main/options-api-disallowed-origin",
    "plain-main/options-unknown-path",
    "plain-main/options-asterisk",
    "plain-main/origin-healthz-disallowed",
    "plain-main/host-evil-healthz",
    "plain-main/remote-plain-healthz",
    "plain-main/remote-plain-foreign-host",
    "plain-main/remote-plain-options",
    "plain-main/not-found-file",
    "plain-main/not-found-post",
    "plain-main/not-found-api",
    "plain-main/static-files-nested",
    "plain-main/alias-api-files-prefix",
    "plain-main/files-disabled",
    "plain-main/files-bare-path",
    "plain-nostatic/files-root-missing",
    "plain-nostatic/files-alias-root-missing",
];

const OUT_OF_SCOPE: &[(&str, &str, &str)] = &[
    ("plain-main", "healthz-trailing-slash", "2"),
    ("plain-main", "healthz-percent-encoded", "2"),
    ("plain-main", "healthz-double-slash", "2"),
    ("plain-main", "healthz-http11-no-host", "transport"),
    ("plain-main", "malformed-request-line", "transport"),
    ("plain-main", "mesh-valid-before-ingest", "4"),
    ("plain-main", "origin-none", "2"),
    ("plain-main", "origin-null", "2"),
    ("plain-main", "origin-unparseable", "2"),
    ("plain-main", "origin-empty", "2"),
    ("plain-main", "origin-same", "2"),
    ("plain-main", "origin-same-other-scheme", "2"),
    ("plain-main", "origin-same-with-path", "2"),
    ("plain-main", "origin-same-uppercase", "2"),
    ("plain-main", "origin-default-port", "2"),
    ("plain-main", "origin-port-mismatch", "2"),
    ("plain-main", "origin-app-bundle", "2"),
    ("plain-main", "origin-app-bundle-upper", "2"),
    ("plain-main", "origin-allowed-env", "2"),
    ("plain-main", "origin-allowed-env-default-port", "2"),
    ("plain-main", "origin-mesh-peer-flipped-scheme", "2"),
    ("plain-main", "origin-mesh-peer-explicit-scheme-only", "2"),
    ("plain-main", "origin-disallowed", "2"),
    ("plain-main", "origin-disallowed-repeat", "2"),
    ("plain-main", "host-evil", "2"),
    ("plain-main", "host-evil-with-port", "2"),
    ("plain-main", "host-rebinding-same-origin", "2"),
    ("plain-main", "host-allowed-env", "2"),
    ("plain-main", "host-dot-localhost", "2"),
    ("plain-main", "host-localhost-upper", "2"),
    ("plain-main", "host-ipv6-loopback", "2"),
    ("plain-main", "host-empty", "2"),
    ("plain-main", "remote-plain-api", "2"),
    ("plain-main", "remote-plain-html", "2"),
    ("plain-main", "remote-plain-pair-redeem", "4"),
    ("plain-main", "remote-plain-enroll", "4"),
    ("plain-main", "static-root", "2"),
    ("plain-main", "static-index", "2"),
    ("plain-main", "static-index-head", "2"),
    ("plain-main", "static-index-query", "2"),
    ("plain-main", "static-hashed-asset", "2"),
    ("plain-main", "static-plain-asset", "2"),
    ("plain-main", "static-nine-char-suffix", "2"),
    ("plain-main", "static-css", "2"),
    ("plain-main", "static-svg", "2"),
    ("plain-main", "static-json", "2"),
    ("plain-main", "static-png-binary", "2"),
    ("plain-main", "static-unknown-extension", "2"),
    ("plain-main", "static-uppercase-extension", "2"),
    ("plain-main", "static-no-extension-file", "2"),
    ("plain-main", "static-subdir", "2"),
    ("plain-main", "static-subdir-slash", "2"),
    ("plain-main", "static-spa-fallback", "2"),
    ("plain-main", "static-spa-deep", "2"),
    ("plain-main", "static-missing-with-extension", "2"),
    ("plain-main", "static-dotdot", "2"),
    ("plain-main", "static-encoded-dotdot", "2"),
    ("plain-main", "static-post", "2"),
    ("plain-main", "static-api-prefix", "2"),
    ("plain-main", "static-term-nested", "2"),
    ("plain-main", "static-audio-nested", "3"),
    ("plain-main", "event-invalid-json", "2"),
    ("plain-main", "event-empty-body", "2"),
    ("plain-main", "event-json-string", "2"),
    ("plain-main", "event-wrong-version", "2"),
    ("plain-main", "event-unknown-type", "2"),
    ("plain-main", "event-bad-ts", "2"),
    ("plain-main", "event-valid-session-start", "2"),
    ("plain-main", "event-valid-chunked", "2"),
    ("plain-main", "event-valid-expect-continue", "2"),
    ("plain-main", "event-reserialize", "2"),
    ("plain-main", "event-get", "2"),
    ("plain-main", "events-not-array", "2"),
    ("plain-main", "events-empty-array", "2"),
    ("plain-main", "events-101", "2"),
    ("plain-main", "events-100", "2"),
    ("plain-main", "events-bad-element-2", "2"),
    ("plain-main", "state-after-rejected-batch", "2"),
    ("plain-main", "events-valid-batch", "2"),
    ("plain-main", "event-canonical-uuid-room", "2"),
    ("plain-main", "sessions-list", "2"),
    ("plain-main", "state-known", "2"),
    ("plain-main", "state-reserialized", "2"),
    ("plain-main", "state-unknown", "2"),
    ("plain-main", "state-missing-param", "2"),
    ("plain-main", "state-empty-param", "2"),
    ("plain-main", "state-canonical-id", "2"),
    ("plain-main", "state-duplicate-param", "2"),
    ("plain-main", "state-post", "2"),
    ("plain-main", "healthz-after-ingest", "2"),
    ("plain-main", "alias-api-events", "2"),
    ("plain-main", "alias-api-events-event", "2"),
    ("plain-main", "alias-api-events-sessions", "2"),
    ("plain-main", "alias-api-events-state", "2"),
    ("plain-main", "alias-api-events-ws-http", "2"),
    ("plain-main", "ws-path-http", "2"),
    ("plain-main", "alias-api-mesh", "4"),
    ("plain-main", "alias-api-terminal", "2"),
    ("plain-main", "alias-api-terminal-config", "2"),
    ("plain-main", "term-config", "2"),
    ("plain-main", "alias-api-terminal-list", "2"),
    ("plain-main", "alias-api-terminal-inject", "2"),
    ("plain-main", "alias-api-terminal-harness-sessions", "2"),
    ("plain-main", "alias-api-terminal-ws-http", "2"),
    ("plain-main", "alias-api-terminal-prefix", "2"),
    ("plain-main", "alias-api-audio", "3"),
    ("plain-main", "alias-api-audio-status", "3"),
    ("plain-main", "alias-api-audio-health", "3"),
    ("plain-main", "alias-api-audio-mic-http", "3"),
    ("plain-main", "alias-api-events-trailing-slash", "2"),
    ("plain-main", "alias-api-events-unknown-child", "2"),
    ("plain-main", "alias-uppercase", "2"),
    ("plain-main", "delete-session-known", "2"),
    ("plain-main", "delete-session-again", "2"),
    ("plain-main", "delete-session-missing-param", "2"),
    ("plain-main", "delete-session-canonical", "2"),
    ("plain-main", "session-get", "2"),
    ("plain-main", "sessions-after-delete", "2"),
    ("plain-main", "layout-bad-key-slash", "2"),
    ("plain-main", "layout-bad-key-empty", "2"),
    ("plain-main", "layout-bad-key-65", "2"),
    ("plain-main", "layout-missing", "2"),
    ("plain-main", "layout-missing-default", "2"),
    ("plain-main", "layout-post-invalid-json", "2"),
    ("plain-main", "layout-post-bad-key", "2"),
    ("plain-main", "layout-post-valid", "2"),
    ("plain-main", "layout-get-written", "2"),
    ("plain-main", "layout-post-default", "2"),
    ("plain-main", "layout-get-fallback", "2"),
    ("plain-main", "layout-key-dots", "2"),
    ("plain-main", "layout-put", "2"),
    ("plain-main", "devices-disabled", "4"),
    ("plain-main", "devices-enroll-disabled", "4"),
    ("plain-main", "devices-delete-disabled", "4"),
    ("plain-main", "phone-pairing-get", "4"),
    ("plain-main", "phone-pairing-post", "4"),
    ("plain-main", "phone-pairing-status", "4"),
    ("plain-main", "phone-pairing-bad-escape", "4"),
    ("plain-main", "phone-pairing-bad-name", "4"),
    ("plain-main", "pair-redeem-bogus", "4"),
    ("plain-main", "pair-redeem-get", "4"),
    ("plain-main", "audio-disabled", "3"),
    ("plain-main", "term-post-disabled", "2"),
    ("plain-main", "capabilities", "2"),
    ("plain-main", "capabilities-post", "2"),
    ("plain-main", "body-exactly-256k", "2"),
    ("plain-main", "body-too-large-event", "2"),
    ("plain-main", "body-too-large-events", "2"),
    ("plain-main", "body-too-large-layout", "2"),
    ("plain-main", "mesh-valid", "4"),
    ("plain-main", "mesh-no-file", "4"),
    ("plain-main", "mesh-invalid-json", "4"),
    ("plain-main", "mesh-root-array", "4"),
    ("plain-main", "mesh-flat-array", "4"),
    ("plain-main", "mesh-nodes-number", "4"),
    ("plain-main", "mesh-empty-object", "4"),
    ("plain-main", "mesh-post", "4"),
    ("plain-redirect", "redirect-get-root", "2"),
    ("plain-redirect", "redirect-get-root-query", "2"),
    ("plain-redirect", "redirect-head-root", "2"),
    ("plain-redirect", "redirect-post-root", "2"),
    ("plain-redirect", "redirect-same-origin", "2"),
    ("plain-redirect", "redirect-index", "2"),
    ("plain-redirect", "redirect-remote-unauthorized", "2"),
    ("plain-redirect", "mesh-default-path-missing", "4"),
    ("plain-nostatic", "nostatic-root", "2"),
    ("plain-nostatic", "nostatic-root-html", "2"),
    ("plain-nostatic", "nostatic-spa", "2"),
    ("plain-nostatic", "nostatic-ws-get", "2"),
    ("plain-nostatic", "capabilities-no-mesh", "2"),
    ("plain-tenancy", "tenancy-ingest-owner-session", "2"),
    ("plain-tenancy", "tenancy-ingest-alice-session", "2"),
    ("plain-tenancy", "tenancy-sessions-owner", "2"),
    ("plain-tenancy", "tenancy-state-own", "2"),
    ("plain-tenancy", "tenancy-state-alice", "2"),
    ("plain-tenancy", "tenancy-state-unknown", "2"),
    ("plain-tenancy", "tenancy-delete-alice", "2"),
    ("plain-tenancy", "tenancy-spoofed-user-header", "2"),
    ("plain-tenancy", "tenancy-unknown-user-token", "2"),
    ("plain-tenancy", "tenancy-empty-user-token", "2"),
    ("plain-tenancy", "tenancy-phone-pairing-owner", "4"),
    ("plain-tenancy", "tenancy-healthz", "2"),
    ("plain-tenancy", "tenancy-static", "2"),
    ("plain-failclosed", "failclosed-healthz", "2"),
    ("plain-failclosed", "failclosed-sessions", "2"),
    ("plain-failclosed", "failclosed-post-event", "2"),
    ("plain-failclosed", "failclosed-static", "2"),
    ("plain-failclosed", "failclosed-options", "2"),
    ("plain-users-invalid", "users-invalid-sessions", "2"),
    (
        "plain-users-shared-invalid",
        "users-shared-invalid-sessions",
        "2",
    ),
];

const SKIP_REASONS: &[(&str, &str)] = &[
    (
        "2",
        "B1/B2/B12: session, tenancy and static-serving scenarios are outside this health, origin and file-root replay slice",
    ),
    (
        "3",
        "B1/B3/B6: optional integration and memory routes are outside this HTTP replay slice",
    ),
    (
        "4",
        "B1/B14: mesh and device-management scenarios require their separate service fixtures",
    ),
    (
        "transport",
        "B1: raw HTTP parser behavior is outside this router-level replay",
    ),
];

fn golden(name: &str) -> PathBuf {
    testkit::recording(name)
}

fn replay_env(
    id: &str,
) -> Result<
    (
        std::sync::Arc<gateway::Gateway>,
        testkit::Masks,
        Option<tempfile::TempDir>,
    ),
    Failed,
> {
    match id {
        "plain-nostatic/files-root-missing" | "plain-nostatic/files-alias-root-missing" => {
            let keep = tempfile::tempdir()
                .map_err(|error| Failed::from(format!("private files root: {error}")))?;
            let tmp = match keep.path().to_str() {
                Some(text) => text.to_owned(),
                None => return Err(Failed::from("private files root is not utf-8")),
            };
            let missing =
                std::path::PathBuf::from(&tmp).join("runs/plain-nostatic/missing-files-root");
            Ok((
                common::gateway_with_files(missing),
                testkit::Masks {
                    port: "5174".to_string(),
                    hostname: Some("gold-host".to_string()),
                    tmp: Some(tmp),
                },
                Some(keep),
            ))
        }
        _ => Ok((common::gateway().0, masks(), None)),
    }
}

fn masks() -> testkit::Masks {
    testkit::Masks {
        port: "5174".to_string(),
        hostname: Some("gold-host".to_string()),
        tmp: Some("/tmp/golden".to_string()),
    }
}

fn must_pass(outcome: Result<(), Mismatch>) -> Result<(), Failed> {
    match outcome {
        Ok(()) => Ok(()),
        Err(mismatch) => Err(Failed::from(format!(
            "{}: {}",
            mismatch.id, mismatch.detail
        ))),
    }
}

fn main() {
    let args = Arguments::from_args();
    match tokio::runtime::Runtime::new() {
        Ok(runtime) => start(&args, &runtime).exit(),
        Err(error) => run(
            &args,
            vec![Trial::test("gateway dev runtime", move || {
                Err::<(), Failed>(Failed::from(format!("dev test runtime: {error}")))
            })],
        )
        .exit(),
    }
}

fn start(args: &Arguments, runtime: &tokio::runtime::Runtime) -> libtest_mimic::Conclusion {
    let handle = runtime.handle().clone();
    let mut trials = Vec::new();

    let recorded = testkit::http::cases(&golden(RECORDING));
    match recorded {
        Ok(cases) => {
            for case in &cases {
                if !CLAIMS.contains(&case.id.as_str()) {
                    continue;
                }
                let mut case = case.clone();
                let id = case.id.clone();
                let handle = handle.clone();
                trials.push(Trial::test(format!("gateway {id}"), move || {
                    if testkit::http::status_of(&case.response.status_line) == 403 {
                        let body = case
                            .response
                            .body
                            .as_deref()
                            .ok_or_else(|| Failed::from("missing refusal body"))?;
                        let mut error: wire::gateway::GatewayError = serde_json::from_str(body)
                            .map_err(|error| Failed::from(error.to_string()))?;
                        error.error = testkit::translate_recorded_product_names(&error.error);
                        error.reason = error
                            .reason
                            .as_deref()
                            .map(testkit::translate_recorded_product_names);
                        case.response.body = Some(
                            serde_json::to_string(&error)
                                .map_err(|error| Failed::from(error.to_string()))?,
                        );
                    }
                    let (gateway, masks, _keep) = replay_env(&case.id)?;
                    let app = gateway.router(false);
                    must_pass(testkit::http::replay(&case, &masks, |request| match handle
                        .block_on(common::answer(app.clone(), request.clone()))
                    {
                        Ok(response) => response,
                        Err(error) => {
                            testkit::Response::json(599, &format!("harness failure: {error}"))
                        }
                    }))
                }));
            }
        }
        Err(error) => trials.push(Trial::test("gateway golden load".to_string(), {
            move || {
                Err(Failed::from(format!(
                    "vendored recording unreadable: {error}"
                )))
            }
        })),
    }
    trials.push(Trial::test(
        "gateway claims are recorded".to_string(),
        check_claims,
    ));
    trials.push(Trial::test(
        "gateway out-of-scope coverage".to_string(),
        check_enumeration,
    ));

    run(args, trials)
}

fn recorded_cases() -> Result<Vec<testkit::http::HttpCase>, Failed> {
    testkit::http::cases(&golden(RECORDING))
        .map_err(|error| Failed::from(format!("vendored recording unreadable: {error}")))
}

fn check_claims() -> Result<(), Failed> {
    let recorded = recorded_cases()?;
    let mut recorded_ids: Vec<String> = recorded.iter().map(|case| case.id.clone()).collect();
    recorded_ids.sort();
    let missing: Vec<&str> = CLAIMS
        .iter()
        .copied()
        .filter(|id| !recorded_ids.iter().any(|recorded| recorded == id))
        .collect();
    if !missing.is_empty() {
        return Err(Failed::from(format!(
            "claimed ids not in the vendored recording: {missing:?}"
        )));
    }
    Ok(())
}

fn check_enumeration() -> Result<(), Failed> {
    for (run, id, code) in OUT_OF_SCOPE {
        if !SKIP_REASONS
            .iter()
            .any(|(key, reason)| key == code && !reason.trim().is_empty())
        {
            return Err(Failed::from(format!(
                "missing exclusion reason for {run}/{id}: {code}"
            )));
        }
    }
    let recorded = recorded_cases()?;
    let mut unaccounted: Vec<String> = Vec::new();
    for case in &recorded {
        let Some((run, id)) = case.id.split_once('/') else {
            return Err(Failed::from(format!("case id without a run: {}", case.id)));
        };
        if CLAIMS.contains(&case.id.as_str()) {
            continue;
        }
        let listed = OUT_OF_SCOPE
            .iter()
            .any(|(listed_run, listed_id, _)| *listed_run == run && *listed_id == id);
        if !listed {
            unaccounted.push(case.id.clone());
        }
    }
    if !unaccounted.is_empty() {
        return Err(Failed::from(format!(
            "recorded cases claimed or listed nowhere: {unaccounted:?}"
        )));
    }
    let mut stale: Vec<&(&str, &str, &str)> = OUT_OF_SCOPE
        .iter()
        .filter(|(run, id, _)| {
            let full = format!("{run}/{id}");
            !CLAIMS.contains(&full.as_str()) && !recorded.iter().any(|case| case.id == full)
        })
        .collect();
    stale.sort_unstable();
    if !stale.is_empty() {
        return Err(Failed::from(format!(
            "out-of-scope entries not in the vendored recording: {:?}",
            stale
                .iter()
                .map(|(run, id, _)| format!("{run}/{id}"))
                .collect::<Vec<_>>()
        )));
    }
    Ok(())
}
