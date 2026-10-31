pub(crate) const EVENTS: &[(&str, &str)] = &[
    (
        "plain-tenancy/tenancy-ws-owner-snapshot",
        "B1 hardening: explicit credentials and trusted certificate fingerprints replace implicit identity mapping; tested separately",
    ),
    (
        "plain-tenancy/tenancy-ws-attach-alice-session",
        "B1 hardening: explicit credentials and trusted certificate fingerprints replace implicit identity mapping; tested separately",
    ),
    (
        "plain-tenancy/tenancy-ws-alice-event-withheld",
        "B1 hardening: explicit credentials and trusted certificate fingerprints replace implicit identity mapping; tested separately",
    ),
    (
        "plain-tenancy/tenancy-ws-owner-event-delivered",
        "B1 hardening: explicit credentials and trusted certificate fingerprints replace implicit identity mapping; tested separately",
    ),
    (
        "plain-failclosed/failclosed-ws",
        "B1 hardening: explicit credentials and trusted certificate fingerprints replace implicit identity mapping; tested separately",
    ),
    (
        "ws-main/connect-c-filtered-unknown",
        "B1 hardening: unowned sessions are refused and unknown upgrade routes return HTTP errors",
    ),
    (
        "ws-main/post-reserialize",
        "B2: snapshot depends on the excluded duplicate-key, fractional timestamp and lone-surrogate producer event",
    ),
    (
        "ws-main/connect-d-alias",
        "B2: snapshot depends on the excluded duplicate-key, fractional timestamp and lone-surrogate producer event",
    ),
    (
        "ws-main/connect-e-canonical",
        "B2: snapshot depends on the excluded duplicate-key, fractional timestamp and lone-surrogate producer event",
    ),
    (
        "ws-main/connect-f-same-origin",
        "B2: snapshot depends on the excluded duplicate-key, fractional timestamp and lone-surrogate producer event",
    ),
    (
        "ws-main/connect-g-app-bundle",
        "B2: snapshot depends on the excluded duplicate-key, fractional timestamp and lone-surrogate producer event",
    ),
    (
        "ws-main/upgrade-origin-denied",
        "B1: legacy raw-socket destruction and process shutdown framing differ from Axum; serving and cancellation tests cover the boundary",
    ),
    (
        "ws-main/upgrade-origin-null",
        "B1: legacy raw-socket destruction and process shutdown framing differ from Axum; serving and cancellation tests cover the boundary",
    ),
    (
        "ws-main/upgrade-host-denied",
        "B1: legacy raw-socket destruction and process shutdown framing differ from Axum; serving and cancellation tests cover the boundary",
    ),
    (
        "ws-main/upgrade-remote-unauthorized",
        "B1: legacy raw-socket destruction and process shutdown framing differ from Axum; serving and cancellation tests cover the boundary",
    ),
    (
        "ws-main/upgrade-unknown-path",
        "B1 hardening: unowned sessions are refused and unknown upgrade routes return HTTP errors",
    ),
    (
        "ws-main/upgrade-healthz",
        "B1: legacy raw-socket destruction and process shutdown framing differ from Axum; serving and cancellation tests cover the boundary",
    ),
    (
        "ws-main/upgrade-term-disabled",
        "B1: legacy raw-socket destruction and process shutdown framing differ from Axum; serving and cancellation tests cover the boundary",
    ),
    (
        "ws-main/upgrade-alias-terminal-ws",
        "B1: legacy raw-socket destruction and process shutdown framing differ from Axum; serving and cancellation tests cover the boundary",
    ),
    (
        "ws-main/upgrade-audio-mic-disabled",
        "B1: note 90 removed the mic bridge",
    ),
    (
        "ws-main/upgrade-alias-audio-mic",
        "B1: note 90 removed the mic bridge",
    ),
    (
        "ws-main/upgrade-missing-key",
        "B1: legacy raw-socket destruction and process shutdown framing differ from Axum; serving and cancellation tests cover the boundary",
    ),
    (
        "ws-main/shutdown-close-frames",
        "B1: legacy raw-socket destruction and process shutdown framing differ from Axum; serving and cancellation tests cover the boundary",
    ),
    (
        "tls-main/tls-ws-remote-no-cert",
        "B1 hardening: explicit credentials and trusted certificate fingerprints replace implicit identity mapping; tested separately",
    ),
    (
        "tls-main/tls-ws-remote-wrong-ca",
        "B1 hardening: explicit credentials and trusted certificate fingerprints replace implicit identity mapping; tested separately",
    ),
    (
        "tls-main/tls-ws-remote-device",
        "B1 hardening: explicit credentials and trusted certificate fingerprints replace implicit identity mapping; tested separately",
    ),
    (
        "tls-main/tls-ws-loopback-no-cert",
        "B1 hardening: explicit credentials and trusted certificate fingerprints replace implicit identity mapping; tested separately",
    ),
    (
        "tls-main/tls-ws-live-event",
        "B1 hardening: explicit credentials and trusted certificate fingerprints replace implicit identity mapping; tested separately",
    ),
    (
        "tls-tenancy/tt-ws-alice-snapshot",
        "B1 hardening: explicit credentials and trusted certificate fingerprints replace implicit identity mapping; tested separately",
    ),
    (
        "tls-tenancy/tt-ws-alice-attach-owner-room",
        "B1 hardening: explicit credentials and trusted certificate fingerprints replace implicit identity mapping; tested separately",
    ),
    (
        "tls-tenancy/tt-ws-stranger",
        "B1 hardening: explicit credentials and trusted certificate fingerprints replace implicit identity mapping; tested separately",
    ),
    (
        "tls-tenancy/tt-ws-alice-live-own",
        "B1 hardening: explicit credentials and trusted certificate fingerprints replace implicit identity mapping; tested separately",
    ),
    (
        "tls-tenancy/tt-ws-alice-live-owner-withheld",
        "B1 hardening: explicit credentials and trusted certificate fingerprints replace implicit identity mapping; tested separately",
    ),
];
