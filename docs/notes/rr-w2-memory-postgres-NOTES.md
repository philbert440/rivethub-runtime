# Memory Postgres final fix round

The requested shared notes destination conflicts with the final hard rule limiting writes to this checkout. These notes are therefore kept in the repository.

## Implementation

- Embedding failures retain the typed provider error or timeout source. No HTTP status is recovered by parsing an error message. Provider failure notices expose the provider error instead of inventing a network classification. Timeout milliseconds remain `u128`; HNSW configuration remains `u64` after its documented clamp.
- Missing capture channels produce `Error::MissingChannel` before writes. The current B4 message schema requires a channel, so absence cannot be stored honestly as a nullable field without a schema migration. No `unknown` channel is fabricated.
- Append event IDs return a string or a typed error. Supplied metadata IDs and capture settings are deserialized into typed structs. An absent append ID is a SHA-256 digest of a serde tuple, preserving the distinction between absent and empty tool names.
- JSON parameters bind through `sqlx::types::Json`. Metadata uses `JsonObject` insertion. Task enums use their existing `Display` and `FromStr` implementations instead of JSON string round trips.
- Search uses the existing wire `MemorySearchHit`. Invalid roles, tags, UUIDs, and integer conversions propagate errors. Chunk-search database failures propagate rather than silently discarding that search arm. Wiki lookup propagates database failures; the unused availability sentinel is removed.
- Search and browse windows are resolved at HTTP/MCP boundaries with a supplied time and zone through kernel helpers. Postgres rejects unresolved windows. Context preserves absent user-query text as `Option` and stops after recent context when no query exists. Queue health computes age in its SQL snapshot.
- `PostgresQueue` is a connected queue, not a connected-or-absent enum. Missing Graphile schema is a connection error. The unused heartbeat string parser is deleted. Git process launch, UTF-8, and join failures retain their typed sources. Count and character-cap conversions are checked.
- Both Graphile queue adapters share the claim, completion, and failure statements. Claim locks a named queue and its selected job atomically, skips locked queues, and supports the four-hour lease expiry. Completion and failure require the adapter's worker ownership and release only the matching queue lease. Failure uses Graphile's exponential retry delay. Task queue claims honor the same locks as worker claims.

## Integration changes

Kernel adds consuming window-resolution helpers. Gateway and MCP use them before invoking the memory port. The worker composition root loses its obsolete `available()` branch because connecting now returns `QueueAbsent`. The corresponding worker and CLI tests use real connection failures. These are the minimal composition-root changes; route mounts are untouched.

The small shared testkit Graphile fixture gains its missing queue-lock columns so existing task/worker test callers can exercise the new SQL. Memory queue tests additionally use the repository's complete vendored Graphile schema dump, restoring the connection search path after loading it. No test reads or mutates a shared database.

Recorded B13 job names, including `compact-conversation` and `enqueue-stale-compaction`, remain kernel compatibility variants. Worker roles and explanatory text use summarizer.

## Replays and regression coverage

The memory recording replay now deserializes typed scenario, row, and result structs and compares stored typed values. Its named skip list is checked for unique known names and nonempty reasons. Three recorded batches omit the channel and rely on an invented `unknown` value: `duplicate event ids and a new one, no title/settings`, `title change, settings cleared to {}, task id, finalize`, and `owner user id option`. Their production calls are asserted to refuse without writes. Final snapshots affected by those skipped mutations are not treated as an expected final database state.

Other tests cover supplied channel/owner persistence, duplicate capture events, Unicode scalar caps, preservation of nested metadata and full-width integers, typed metadata refusal, append ID stability and absence distinctions, invalid UUID refusal, missing context query, and UTC versus a supplied +05:30 calendar window.

Named-queue tests use an isolated SQLx database per test and cover concurrent claim exclusion, independent and unnamed jobs, row-lock skipping, completion release, failure release and retry metadata, stale lease recovery, stale-worker refusal, and interoperability of task and worker claims. The existing capture/Postgres replays and their checked deviations remain in place.

There is no README in `crates/testkit/tests/fixtures/memory/`; it contains `capture-batch.json`. The related capture recording README and the supplied design/fix list were read. No TypeScript or JavaScript source was consulted.

## Validation

The final required checks both passed against the lane Postgres URLs:

```text
SQLX_OFFLINE=true cargo xtask check --locked
GATE OK: 0 finding(s), cargo xtask check

NEXTEST_TEST_THREADS=2 cargo nextest run --workspace --all-features
Summary [ 193.508s] 1356 tests run: 1356 passed, 20 skipped
```

The affected memory-postgres, gateway, and MCP suite also passed all 497 tests. The final workspace run used two test processes; there were no automatic retries, timeout changes, or additional nextest skips.

Earlier runs encountered unrelated gateway serving/audio and RivetHub process deadline failures. The serving suite passed all 11 cases in isolation. The four RivetHub process failures passed individually. The gateway `disabled_mic_upgrade_returns_no_bytes` failure passed in 33 ms in isolation, and both CLI version binaries started in under one second before the successful full workspace rerun. These tests and their deadlines were not changed.

The initial prompt conflicted over deleting resolved baseline entries. Following the user's instruction to finish both checks and commit in response to the deletion request, only the ten stale memory-postgres FALLBACK entries were removed. The lint had confirmed zero remaining matches at every entry. No lint rule, threshold, runner configuration, or unrelated baseline entry changed.

The requested commit title is `refactor(memory-postgres): the final fix round`. No push is part of this work.
