# Composed TLS replay

`composed_tls_replay.rs` runs all 42 HTTP cases from
`wire/tests/golden/gateway/http-tls.json` against three fresh `rh gateway`
processes. It uses the shared testkit HTTP recording types and comparator.
The recording remains the expected-response source.

Each test database comes from `#[sqlx::test]`. Homes, configuration, CA material,
static content, users registry, and session-owner map live in a temporary directory
inside the crate. The gateway sees only that home and explicitly supplied test
environment. PostgreSQL URLs in the vendored typed users registry are replaced
with the test database URL; absent URLs remain absent. The registry's `user-b`
key differs from the user named in one historical error response and is preserved.
The `den` configuration key and the recording's legacy user-token header are
contract-pinned B10/B1 compatibility names.

The harness generates a server CA and certificate plus eight client leaf variants:
legacy device, CN-only device, OU client, node, mapped device without a database,
unmapped device, untrusted issuer, and serverAuth-only device. Anonymous requests
present no certificate. The optional run omits the client CA, as recorded.
The server certificate covers loopback and the actual local non-loopback address.
Remote clients explicitly bind that non-loopback address and connect to it; the
recording's 127.0.0.2 shortcut would be treated as loopback by Rust and is not used.
Host and Origin retain their recorded values with only the port substituted.

Readiness comes from the process's listening event. Requests and shutdown have
timeouts; there is no startup polling or synchronization sleep. The gateway binary
owns its multi-thread runtime and cancellation tree. The shared process helper
allows the recording's node name to be injected without inheriting host settings.

Comparisons use the existing testkit HTTP policy: status and application headers,
case-insensitive HTTP header names, and the recording's documented dynamic masks.
Transport-managed Date, Connection, Keep-Alive, Transfer-Encoding and Content-Length
may be absent under that shared policy; present values are still checked. Socket
close timing, chunk counts and raw header ordering are outside this HTTP replay.
JSON bodies also pass an exact `wire::JsonObject` equality check before testkit's
comparison, preserving null versus absent members and array order. Text bodies
are compared directly. Neither actual nor expected JSON is normalized.

`skips.json` contains only cases from this HTTP TLS recording. Every entry has a
reason and a typed expected HTTP status or TLS certificate alert. All skipped
requests still execute: HTTP skips must continue to differ from the recording,
and TLS skips must produce the specific rustls certificate alert. Other request,
fixture, parsing, database or process errors fail the test. A passing skipped case
is a stale-ledger failure. The inventory test rejects duplicate and unrelated skips
and empty reasons. Other recordings, including WebSocket recordings, are not claimed.

Current accounting: 13 matching cases and 29 audited deviations. No production
behavior or historical gateway parity entries are changed. The latter remain an
accurate ledger for the in-process harness; this suite removes its transport and
source-address limitations and records the composed gateway's actual differences.
