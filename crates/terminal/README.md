# Terminal adapter

The adapter implements kernel's `TerminalMux` using herdr control RPC and PTY byte streams. Each RPC reply is decoded with serde. Missing result fields remain optional; an error acknowledgement is an error even when its code is absent. Background subscription failures reach subscribers as typed errors.

The vendored B12 recordings are `crates/testkit/tests/fixtures/term/node-pty.json` and `ws-pty-frames.json`. Their descriptions specify the recording setup and observable behavior. Exit-code replays use equivalent native shell processes rather than running the recording generator. Signal deaths retain the B12 exit-code-zero convention; failures to read or wait for a process are errors.

`exits_replay` covers every raw PTY case. `ws_replay` covers terminal-owned output, late attachment, dimensions, and exits; `settings` checks the environment case with the contract's product-name translation. The checked `SKIPPED` list in `ws_replay.rs` is empty. New scenarios must be covered or entered there by name with a reason. Gateway WebSocket routing and close timing belong to the gateway.

The TTL tests live in `tests/manager_ttl/mod.rs`, included as crate tests so they can exercise the private reaper with a paused clock. PTY readiness and detachment are awaited as events with a timeout. RPC tests use isolated Unix sockets, and filesystem tests use temporary directories.

Direct PTY selection and the crate size target remain subject to the decisions listed in the final-fix brief. Compatibility names in recordings are translated in tests. Child processes strip the deprecated `RIVETOS_USER_ID` and `RIVETOS_USER_TOKEN` identity variables as well as their current spellings. The `denKey` field is a recorded foreign marker; `rivet-ca/intermediate/chain.pem` is the existing shared CA layout.

The opt-in herdr binary conformance tests remain ignored in normal runs. Listing tests does not probe installed programs or read their state; explicit execution uses an isolated temporary herdr namespace.
