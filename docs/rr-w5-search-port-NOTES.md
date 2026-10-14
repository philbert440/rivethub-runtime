# Search port

The kernel search request carries typed since/before bounds, a calendar window, and optional expansion/synthesis flags. Explicit bounds take precedence over the window; window resolution reuses the kernel resolver in the server local timezone. Postgres filters messages, summaries, and chunk parents before ranking and limiting, using inclusive since and exclusive before bounds.

The port returns results, a degraded reason and effect, a fallback marker, expanded source messages, and optional synthesis. Gateway HTTP and tool routes and MCP forward every argument. Degradation is reported by the adapter, including runtime embedding failures, rather than inferred from gateway configuration.

Expansion defaults to enabled and reads the selected summaries' source links, with duplicate messages eliminated and the query's time and agent filters retained. Expansion has a 200-message cap and deterministic chronological ordering. Synthesis defaults to disabled; when requested it uses an injected kernel ModelProvider, respects its CancellationToken, has a 60-second timeout, and propagates missing configuration, provider, empty-response, serialization, and timeout errors. It does not set sampling or output-token defaults.

Coverage includes every search mode over both tables, time boundaries before limits, explicit-bound precedence, local calendar/DST resolution, chunk-only parent filtering, expansion enabled/disabled/default, synthesis success/cancellation/absence/empty output, embedding failures through the port, and typed gateway/MCP argument and outcome round trips. Existing capture and gateway/MCP cases remain in the test suite. The memory recording skip list is empty and a test checks that any future entries name unique existing scenarios and include a reason.

The specified memory fixture directory contains only capture-batch.json with six capture scenarios. Its promised search recordings and README are absent. The MCP recording README explicitly says memory calls were not recorded. No missing recordings were fabricated, and exact search/expansion/synthesis recording parity cannot be claimed. SQLite search is excluded as requested. No services or databases were started, stopped, or reconfigured.

Notes are kept here because the prompt's final hard rule permits writes only inside this repository, conflicting with its earlier external notes destination.

Validation completed with DATABASE_URL and RIVETHUB_PG_URL pointing to the lane Postgres:

- `SQLX_OFFLINE=true cargo xtask check --locked`: `GATE OK: 0 finding(s), cargo xtask check`.
- `cargo nextest run --workspace --all-features`: `Summary [  29.391s] 836 tests run: 836 passed, 9 skipped`.

The workspace skips are pre-existing; this slice adds none. One full test attempt encountered SQLx test databases disappearing between creation and connection (Postgres reported that they had just been dropped or renamed). An unchanged rerun passed, consistent with concurrent test-cleanup interference. No runner or gate configuration was changed. The gate emitted existing cargo-deny unused-wrapper warnings but passed all policies.

Runner follow-up: renamed the MCP transport result binding from `content` to `block` to satisfy Clippy similar_names. The earlier gate overlapped the transport assertion edit and did not validate the final tree. After this correction, the full gate and then the full workspace nextest run both passed sequentially with no intervening Rust source changes.
