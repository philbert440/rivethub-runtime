# Terminal final fix round

Compared the terminal slice with local main `b0dd4582bb20b79c5d1195280d80b20f4c11e4c1` before editing; the checkout initially matched main. Read the design HTML, final-fix brief, repository patterns, contract, and terminal recordings. The terminal crate and terminal fixture directory had no README on this main; the new crate README documents their replay ownership.

## Sweep and review disposition

| Item | Main finding | Change |
| --- | --- | --- |
| Uncoded error acknowledgements (both review follow-ups) | `AckError.code` required; a decoding failure was discarded | Optional typed code; any error object yields `Error::Ack`, including an empty object. Wrong types yield a serde error. |
| Undecodable control/event frames dropped | RPC loop ignored decode errors; event pump ignored read/decode errors | Propagate serde/I/O errors immediately. Subscription receivers receive typed failures; subscription tasks have a tracker and cancellation token. Verify acknowledgement IDs. |
| `AckResult::Unrecorded` | Both unknown and absent results lost information | Results remain optional; typed result kind and flattened parsed fields retain extension payloads. No replacement sentinel. |
| Unreadable version | Process errors became the invented string `unreadable`; empty first line defaulted to an empty string | Typed probe execution, metadata, status, encoding, and version errors. Standard filesystem/process APIs; no error-to-version replacement. |
| Six dead CLI helpers | Five unused helpers remain; `attach` already has a production caller in `manager/port.rs` on main | Remove snapshot, agent-list/read, pane-list/read helpers. Keep the already-used attach helper. |
| Test-only public items | Unused classifiers/accessors and parser wrappers are public | Remove `AgentKind::is_bare`, manager plan/settings accessors and run-tasks wrapper. Make binary-search/hash helpers private, scrollback internal and test accessors test-only; request decoding is a test helper. Production request serialization uses `RequestFrame` itself. Remove the foreign-marker parser whose success and failure both refused ownership; the marker fixture type now lives in tests. |
| Duplicated errors and stringly errors | Separate unavailable-message formatter plus stringified I/O/backend failures | Format through typed errors; retain underlying sources. Dedicated errors for missing session/server/argv, invalid PID, signal failure, worker/lifecycle channel failures, JSON, timeout, and socket closure. |
| List drops invalid session keys | Failed `SessionId` parsing silently filters a terminal | Return the typed parse failure through the kernel port. Plain PTYs without a session remain outside the session roster. |
| Port-zero sentinel | Missing gateway config was replaced with a default section, then port zero meant no URL | Read the optional gateway section directly. Missing gateway config remains absent; configured values are not replaced by a default section. |
| “den pins” operator text | Already renamed on main by `3d236de` | Not re-done. |
| Hand duration conversion | Seconds multiplied by 1,000 into `u64` | Standard `Duration::as_millis()` and `u128` timeout fields. |
| Child wait/read failures become success or EOF | Child wait errors and lost exit channels became exit zero; reader errors ended the stream | Typed worker outcomes, retained failure state, and typed wait/attach errors. Input awaits the writer's actual I/O result. Only actual signal exits use the B12 code-zero convention. |
| Flaky manager TTL / direct PTY tests | Wall-clock sleeps, short read windows, and repeated state probes | Await output/detach/exit events with generous timeouts. Exercise TTL/reaper transitions with paused time through private crate tests. |
| Namespace safety | Ancestor symlink could redirect the session tree | Resolve filesystem paths before adoption/ownership checks; reject paths outside the state root and empty owners. Open session stamps with `O_NOFOLLOW` so a stamp symlink is neither read nor overwritten. Existing sibling-prefix, parent traversal, foreign-owner and socket-symlink tests remain. |

## Scope and compatibility

PTY workers and forwarding tasks are tracked; cancellation releases idle or backpressured viewers, and shutdown escalates child termination before joining workers and rejects new starts. PTY sizes reuse kernel `TermSize`. Invalid UTF-8 environment names produce a typed error instead of silently disappearing.

No other crate, composition root, gateway route mount, lockfile, service, database configuration, or gate policy was changed. The only repository-wide edit removes the four obsolete terminal FALLBACK exceptions explicitly requested by this cleanup. There are no cross-crate signature call-site changes. `tokio`'s filesystem feature is enabled only in terminal's manifest.

Direct PTY selection and auto-mode behavior remain unchanged pending Phil's Direct PTY decision. Crate size targets remain unchanged pending Phil's threshold decision; all terminal Rust files remain under 600 lines. Zod JSON is a gateway/Phil decision, outside this slice. The gateway `contained_directory` error is also outside this terminal-only edit scope.

Compatibility retained: B12 signal deaths expose exit code zero; legacy identity environment spellings are removed from child environments; the foreign-marker `denKey` field and existing `rivet-ca/intermediate/chain.pem` layout are contract-pinned. Fixture-only product-name translation occurs on expected recording data, not on both sides of comparisons.

## Recording and regression coverage

- Replay all eleven raw PTY exit scenarios from `term/node-pty.json` with native shell/PTY processes. No JavaScript generator is executed.
- Replay terminal-owned WebSocket bytes, dimensions, late attachments and exit frames from `term/ws-pty-frames.json`; the existing settings replay covers the environment case. A checked name/reason skip list is empty.
- RPC regression tests cover uncoded and empty errors, malformed JSON, wrong field types, mismatched IDs, absent results, event-stream errors and failed probes.
- Paused-clock lifecycle tests cover linger/GC deadlines, detached/idle TTLs, idle-disable behavior, input resetting activity, eviction and failed child waits.
- Namespace tests cover valid ownership and refusal of sibling-prefix, parent traversal, symlink escapes, missing ownership and corrupt stamps.

## Instruction conflicts

The prompt requests a shared notes file but also forbids every write outside this repository. These notes are kept inside the terminal crate to comply with the explicit hard write boundary; the clarification request received no response.

The explicit requirement to shrink terminal check-10 exceptions is applied narrowly: remove the four stale FALLBACK entries, without adding or weakening any gate rule. Entries removed: `herdr/client.rs` (`Err(..) => value`), `herdr/mod.rs` (`Err(..) => value` and `unwrap_or`), and `manager/mod.rs` (`Err(..) => value`), each count 1. This specific cleanup authorization resolves the general prohibition on gate edits used to bypass failures.

## Validation

The first `SQLX_OFFLINE=true cargo xtask check --locked` completed with exit 0. A final rerun is checking the frozen source because late edits occurred during early phases of that first run.

The complete serial diagnostic run (`NEXTEST_TEST_THREADS=1 cargo nextest run --workspace --all-features --no-fail-fast`) ran all 1,178 tests: 1,174 passed, four failed, 16 skipped. Every terminal test passed. The four failures were deadline errors outside terminal:

- `harness-acp::release::release_closes_the_owner_and_allows_resume_on_the_same_adapter`
- `rivethub::cli::one_release_compatibility_binary_dispatches_the_same_tree`
- `rivethub::composed::check_validates_without_moving_legacy_state_or_connecting_postgres`
- `rivethub::composed::composed_binary_serves_tls_memory_and_websocket`

Earlier fail-fast attempts stopped on the already-noted CLI PID-file timing test and two gateway audio deadlines. No outside-slice test or timeout was edited. The final required-command run used `SQLX_OFFLINE=true NEXTEST_TEST_THREADS=2 NEXTEST_RETRIES=2`, with existing lane database environment. `SQLX_OFFLINE` controls compile-time query metadata; runtime database tests retained the lane URLs. No repository test configuration changed.

Final frozen-source validations both exited 0:

```text
SQLX_OFFLINE=true cargo xtask check --locked
GATE OK: 0 finding(s), cargo xtask check

SQLX_OFFLINE=true NEXTEST_TEST_THREADS=2 NEXTEST_RETRIES=2 cargo nextest run --workspace --all-features
Summary [ 372.351s] 1178 tests run: 1178 passed (1 flaky), 16 skipped
```

One test needed a retry in the successful final run. The standard fail-only output does not identify that recovered test. The 16 skips are existing workspace ignored tests, including the two explicitly opt-in real-herdr tests; the terminal recording scenario skip list is empty. The final gate includes rustfmt, workspace pedantic/nursery Clippy, custom lints, dependency checks, SQLx metadata verification, and baseline checks. The full nextest run includes all terminal fixtures and regression tests. No lane database or service was started, stopped, or reconfigured.

Commit: `refactor(terminal): the final fix round`, without trailers. No push.
