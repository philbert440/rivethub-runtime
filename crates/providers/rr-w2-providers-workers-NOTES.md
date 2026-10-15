# Providers and workers final fix round

The requested shared notes path conflicts with the prompt's final hard rule limiting writes to this checkout. These notes remain in the providers crate.

## Implementation

- Disabled OpenAI model probing lists the configured model without making a discovery request.
- Token command errors retain their original OS or timeout sources in `Arc`; cloning a mint error no longer reconstructs an I/O error from text. Cleanup retains both kill and wait errors when both fail.
- The stream test server holds an unfinished response with `pending()`. Disconnect tests await cancellation with a timeout. The ordinary fixture server returns a finite response without wall-clock pacing.
- Provider replay assertions deserialize the existing OpenAI wire types and embedded tool arguments, comparing typed values without rewriting JSON strings. The CONTRACT B8 omission table remains the authority for the explicitly approved request-field differences.
- Worker startup rejects a requested wiki role without `WikiBuilderDeps`, including `all`, before subscribing or claiming. Role selection no longer filters unavailable roles out.
- The kernel `JobQueue` port supplies an owned notification subscription. Workers subscribe before the first claim and select notifications, task completions, cancellation and the existing poll deadline. A child cancellation token terminates in-flight work when the runner exits on a queue error. The unused public manual wake method is removed.
- PostgreSQL subscriptions own a `PgListener` on the existing Graphile `jobs:insert` channel. SQLite clones share a watch sender, notified after queue insert/replace and capture commits. Independently opened SQLite connections and scheduled/retried jobs remain covered by polling.
- Required call-site changes are confined to the kernel queue port, its two store adapters and testkit fake. No composition or gateway mount files changed. `workers/ARCHITECTURE.md` explains why the worker and task-engine queue contracts remain separate rather than forcing partial adapter implementations.
- The touched summarizer replay file was split into fixture declarations, leaf tests, sweep tests and support, each below 600 lines.

## Fixes already present at the starting revision

Embeddings deserialize the existing typed `EmbeddingResponse`/`EmbeddingRow`; wrong types and absent indices fail. Duplicate, missing and out-of-range indices fail rather than choosing slot zero. `OpenAiCompatible::embed` delegates to `Embedder`. `embed_wire` and the public test-only `normalize_embed_vector` are absent. Both crates already have zero check-10/FALLBACK baseline entries; no gate configuration or baseline edits were needed. Legacy function-diff vector replays are absent.

## Coverage and scope

Existing HTTP, CLI provider, embedding, token, worker job and fixture replay suites remain enabled. New regressions cover missing streamed tool indices, a body transport error after a text event, original OS source identity, disabled discovery, missing dependencies for a single role and `all`, all four roles with dependencies, notification wake-up, fallback polling, and PostgreSQL/SQLite notifications between an empty claim and waiting.

The existing named replay/skip partitions remain checked. There is no `fixtures/workers` directory in this checkout: B13 recordings are under `capture/pg` and `memory-sqlite`. Native Anthropic, Gemini, xAI and Ollama clients remain outside B8; no native-provider skip inventory was imported into this slice. JavaScript function artifacts are not restored. Recorded B13 job names and recorded `ros_*` payload table names remain compatibility names.

The earlier notes' streaming.rs line reference is superseded by `tests/stream_failures.rs`, which directly exercises both translation and transport failures.

## Validation

- `SQLX_OFFLINE=true cargo xtask check --locked`: exit 0. Last line: `GATE OK: 0 finding(s), cargo xtask check`.
- `NEXTEST_TEST_THREADS=2 SQLX_OFFLINE=true cargo nextest run --workspace --all-features`: exit 0. Last line: `Summary [ 316.944s] 1352 tests run: 1352 passed, 20 skipped`.
- Focused providers, workers and queue-adapter run: 152 passed, zero skipped.
- Gateway serving rerun: 11 passed. CLI compatibility-binary rerun: one passed in 0.08 seconds.

The first full run at default concurrency stopped at 497/1352 tests with five gateway serving timeout failures. The second stopped at 976/1352 with one CLI compatibility-binary deadline failure. Both groups passed in isolation. The successful complete run used two concurrent test processes to reduce contention; no test deadlines, runtime thread counts, skip lists or repository configuration were changed. The 20 workspace skips are pre-existing.

The existing CLI build script watches the whole `.git` directory although it only reads HEAD. Index updates can therefore trigger unrelated CLI/RivetHub relinking. This and the unrelated timing-sensitive tests were left outside this slice. Cargo-deny's existing unused-wrapper warnings also remain; the policy checks passed.

Validation logs are retained under `target/rr-w2-validation/`. No gate configuration, baseline, Cargo.lock, gateway mounts or RivetHub composition files changed. No push.

## Runner follow-up

The runner reproduced a ten-second timeout in the read-only gateway-check subprocess at default workspace concurrency. CLI version subprocesses had also timed out in the earlier full run. These tests launched the production runtime with one worker per available CPU (16 here), multiplying threads across concurrently running test processes. The CLI and read-only composition subprocess tests now set Tokio's standard `TOKIO_WORKER_THREADS=2` environment variable for their child runtimes and use `kill_on_drop(true)` so a timed-out command cannot outlive its test. Both workers remain available; production runtime sizing, test deadlines, workspace concurrency and gate configuration are unchanged. These eight test-only lines are the follow-up's sole change outside providers/workers and their original port call sites.

Follow-up validation with the lane `DATABASE_URL`, no nextest concurrency override:

- Focused CLI, composition and PostgreSQL notification tests: `5 tests run: 5 passed, 0 skipped`.
- `SQLX_OFFLINE=true cargo xtask check --locked`: exit 0, `GATE OK: 0 finding(s), cargo xtask check`.
- `SQLX_OFFLINE=true cargo nextest run --workspace --all-features`: exit 0, `Summary [  78.290s] 1352 tests run: 1352 passed, 20 skipped`.

The final run supersedes the earlier two-thread workspace validation. Logs are `target/rr-w2-validation/followup-focused.log`, `followup-gate.log` and `followup-nextest.log`. The existing 20 skips are unchanged. No gate configuration, production composition files, runtime sizing defaults or additional scope changes were needed.
