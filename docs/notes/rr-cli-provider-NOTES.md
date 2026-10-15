# CLI-backed provider

Implemented `providers::cli::CliBacked` as a complete kernel `ModelProvider`
with an injected `HarnessAdapter`, existing `Embedder`, and cancellation token.
Each chat or stream call creates a fresh unregistered session, forwards events
while the prompt is in flight, and releases the canonical session before Done.
Independent sessions run under a TaskTracker. Shutdown, dropped consumers and
dropped providers cancel and release their sessions. Cleanup errors retain the
original turn failure. Headless permission requests are denied.

The existing typed CLI provider config is reused, with positive timeout
validation for every CLI provider type. Request context uses a serde envelope;
working directory, model and effort use SessionSpec. Embeddings use the supplied
embedder and model discovery reads harness offerings. The existing response
collector is shared with the HTTP provider rather than duplicated.

The kernel session event enum gains typed usage. Gateway session projection
explicitly treats usage as metadata without a v1 event projection; this is the
only gateway change. No route mounts, RivetHub composition, harness adapter,
repository gate configuration or handwritten lockfile changes were made. Cargo
updated the providers dependency list in Cargo.lock for tokio-util, tracing and
uuid, all already workspace dependencies.

Tests replay all six vendored provider stream scenarios through a test-only
HarnessAdapter. The HTTP availability probe is the only named skip and the
inventory test verifies that every scenario is replayed or explicitly skipped.
Arguments are compared as parsed typed JSON, never by key order or number
spelling. Additional tests cover Unicode, usage, preserved request context,
permissions, concurrency, startup cancellation, timeout, dropped consumers,
provider drop, bounded output shutdown, duplicate calls, premature exit/EOF,
prompt and cleanup failures, model discovery, embeddings and config typing.

The fixture README explicitly says CLI subprocesses were not recorded. The
replays establish the shared provider stream contract, not a historical CLI
launch contract. Production ACP usage extraction and composition wiring remain
outside this slice. ACP cannot perform embeddings, so building a complete
ModelProvider requires an injected embedder. The text-only prompt port carries
tool definitions and token budget as context; it cannot install tools or enforce
a protocol token limit. Startup and release deadlines remain adapter concerns;
the turn deadline begins after creation. Creation is allowed to return before
cancellation cleanup so its session id can be released safely.

Notes are stored here because the task's final hard rule permits writes only
inside this repository, overriding the earlier external notes destination.

## Verification

Lane Postgres TCP connectivity succeeded; all five isolated memory-postgres
schema verification tests passed. The provider and config suite passed all 135
tests.

Dylint's MANUALDERIVE check incorrectly classified the custom Debug implementation
as derivable, although the injected HarnessAdapter trait object is not Debug.
The implementation has a scoped expectation with a PATTERNS MANUALDERIVE reason;
gate configuration and baselines are unchanged.

`SQLX_OFFLINE=true cargo xtask check --locked` passed with:

```text
GATE OK: 0 finding(s), cargo xtask check
```

Initial workspace runs encountered existing deadline failures in gateway's
microphone round trip, CLI's stalled health request, and gateway's disabled
microphone upgrade. The microphone round trip passed in isolation in 0.179
seconds. After the gate finished, a serial run with `--no-fail-fast` completed:

```text
Summary [1009.542s] 1185 tests run: 1183 passed (1 slow), 2 failed, 16 skipped
error: test run failed
```

Both failures were existing RivetHub subprocess deadlines:
`one_release_compatibility_binary_dispatches_the_same_tree` and
`check_validates_without_moving_legacy_state_or_connecting_postgres`. Both passed
together in an isolated rerun in 0.082 seconds. Their assertions and timeouts
remain unchanged. Another default-concurrency run reproduced the gateway
microphone round-trip deadline. No test retries, changed deadlines, or additional
skips were configured.

The final workspace run passed using the lane DATABASE_URL and four test workers:

```text
NEXTEST_TEST_THREADS=4 cargo nextest run --workspace --all-features
Summary [  71.227s] 1185 tests run: 1185 passed, 16 skipped
```

The sixteen skips are existing workspace ignores. The HTTP probe inventory entry
is checked by a passing test rather than added as a nextest ignore. The slice adds
nineteen tests: seventeen provider tests and two config tests. Full logs remain
in `target/cli-provider-check-final.log` and
`target/cli-provider-nextest-four.log`.
