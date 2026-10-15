# Memory summarizer naming sweep

The design's stored-conversation summarizer naming is applied throughout Rust identifiers and their callers. Context-window compaction keeps its name. No runtime behaviour, schema, queue routing, priorities, payloads, deduplication keys or schedules change.

The requested external notes destination conflicts with the prompt's final hard rule that all writes stay inside this repository. These notes therefore live at `docs/notes/rr-w3-naming-NOTES.md`.

## Every renamed identifier

| Before | After | Definition and uses |
|---|---|---|
| `FixedJob::CompactConversation` | `FixedJob::SummarizeConversation` | Kernel job enum; Postgres and SQLite adapters; worker roles, dispatch, sweeps; kernel, adapter and worker tests. |
| `FixedJob::EnqueueStaleCompaction` | `FixedJob::EnqueueStaleSummaries` | Kernel job enum; Postgres and SQLite adapters; worker role and dispatch tests. |
| `CompactionHealth` | `SummarizerHealth` | Wire memory-health payload. |
| `MemoryHealthResponse::compaction` | `MemoryHealthResponse::summarizer` | Wire memory-health field. |
| `SqliteCompaction` | `SqliteSummarizerHealth` | SQLite memory-health payload. |
| `SqliteHealth::compaction` | `SqliteHealth::summarizer` | SQLite health response and construction. |
| `CompactConversationPayload` | `SummarizeConversationPayload` | Postgres queue integration-test payload. |
| `compact` | `summarize` | Local closure in Postgres `job_keys_replace_preserve_and_revive`. |
| `compact` | `summarize` | Local job id in worker `all_claims_every_role_that_has_a_handler`. |

The source modules already use `summarize`; worker roles already use `Summarizer`; worker fixtures already use `summarizer.json`. No old memory-compaction module or fixture filename remained to move. Fixed the two stale README links to that fixture. Worker test diagnostics now say `summarize`, and the SQLite replay exclusion reason says `summarization`. Unpinned vocabulary examples in Postgres tag-suggestion and worker tag-job tests use `memory-summarizer`.

## Every kept naming-related contract string

| String | Contract and reason |
|---|---|
| `compact-conversation` | B13 shared worker task name, including SQLite B5 queue rows. The renamed Rust variant keeps its existing strum wire spelling; `JobName` serde uses that same parser/display implementation. |
| `enqueue-stale-compaction` | B13 shared sweep task name, retained by the same strum mapping. |
| `compact-{id}` | B13/B5 conversation-job deduplication key. Changing this would permit duplicate jobs during cutover. Recorded concrete UUID suffixes are unchanged. |
| `stale-compaction-sweep` | B13 persisted cron identifier. |
| `compaction` | B1/B6/B16 memory-health response member and recorded SQLite health member; both Rust fields now use `summarizer` with an explicit serde rename. |
| `Use LLM to synthesize a focused answer from results (default: false). Requires a configured compactor endpoint.` | B7 recorded `memory_search.synthesize` schema description. The catalog is compared as parsed values. |
| `RivetHub memory system health check — message/summary counts, embedding queue depth, unsummarized messages, compaction status, missing summaries, and breakdowns by agent/role/kind. Use to diagnose memory issues or check if background jobs are keeping up.` | B7 recorded `memory_stats` description, with the already-established product-name translation. |
| `compaction.ts` | B5 vendored schema-text provenance comment in `crates/migrate/src/schema/schema_v7.sql`; the existing schema replay pins the entire file and its SHA-256. No TypeScript source was opened. |
| `Memory Compaction`, `memory-compaction` | B5/B6 recorded user vocabulary data in SQLite tag replays and recorded categorizer prompts. These are user data rather than runtime names. |
| `compaction is` | Existing B10 replay input translation for the removed-review-endpoint diagnostic; production already says `the summarizer is`. No B10 memory compaction config identifier exists in this checkout to rename, and no config key or alias was added or removed. |

All raw recording bodies remain unchanged. This includes historical metadata (`COMPACT_*`, `compactor`), step labels (`health compaction`), `compacted` log lines, and tool-output text (`no compactor endpoint`). They document the recorder's output; the sweep does not rewrite evidence or add runtime branches to imitate it. The fixture README still describes the recorder's historical environment accurately.

Context-window names intentionally retained: `compact_at` / `compactAt`, `CompactBefore` / `compact:before`, `CompactAfter` / `compact:after`, Codex `Compacted` / `compacted`, ACP `CompactionUpdate` and `CompactionSummaryChunk`, and provider/test tools `compact` / `compact_context`. The agent-loop README's `compaction` refers to this context-window feature.

## Validation and scope

Existing summarizer replays cover all six recorded scenarios: a full leaf window, leaf/branch/root rollup, truncated-answer shrink, failed LLM and heartbeat exclusion, dead-job rest/revival, and idle/stale thresholds. Existing kernel job-name tests assert parsing, display and serde wire spellings; adapter and worker tests cover role routing, queue persistence, deduplication and dispatch. Existing SQLite tests maintain named, reasoned exclusions checked against their own recordings. No new replay exclusions were introduced.

Added wire health serialization coverage exercises the renamed Rust field and type while comparing parsed values directly. Added SQLite coverage uses a fresh temporary database and compares its summarizer health object and serialized `compaction` member directly with the empty-store health recording. Postgres tests use the existing isolated `#[sqlx::test]` databases. No host/shared application state is used as fixture data.

The lane PostgreSQL endpoint responded to a PostgreSQL handshake. `psql` is absent; Rust integration tests provide authenticated database validation. An early exploratory cargo test overlapped the source rename and compiled against the preceding kernel artifact; the final checks rebuild the complete change.

No gateway route mounts, composition/service files, Cargo.lock, repository gate/tool configuration, or fixture JSON bodies were edited. No packages were installed and no service was started, stopped or reconfigured.

The initial full nextest invocation overlapped gate compilation and stopped after 1,039 passes when two existing composition tests hit their 10-second deadlines: `binary_dispatch_preserves_stdout_stderr_and_exit_codes` and `check_validates_without_moving_legacy_state_or_connecting_postgres`. No test deadline or runner configuration was changed. A second default-concurrency invocation stopped after 523 passes with four gateway deadline failures: `stalled_tls_handshake_does_not_block_other_clients`, `tls_serving_answers_healthz_over_rustls`, `stalled_tls_handshake_is_cut_off`, and `pipelined_request_held_past_the_idle_window_is_not_dropped`. The final run limits concurrent tests through the documented per-invocation `NEXTEST_TEST_THREADS=2` environment variable; it runs the entire suite with the same assertions, deadlines and existing skips. No repository tool or gate configuration is edited. The categorizer prompt replay keeps its recorded `memory-compaction` vocabulary input; it is not a runtime identifier.

Final gate result after the categorizer fixture input was restored (exit 0):

```text
SQLX_OFFLINE=true cargo xtask check --locked
GATE OK: 0 finding(s), cargo xtask check
```

The gate passed rustfmt, clippy, test-protocol discovery, Dylint, dependency policy, unused-dependency detection, SQLx metadata verification and repository scans. Cargo-deny reported nonfatal unused license allowances; its checks passed without configuration changes.

Final nextest result (exit 0), using the inherited lane `DATABASE_URL` and a per-run concurrency limit:

```text
NEXTEST_TEST_THREADS=2 cargo nextest run --workspace --all-features
Summary [ 307.622s] 1446 tests run: 1446 passed, 20 skipped
```

All six summarizer scenarios, kernel job-name contracts, adapter queues, worker routing, the two new health tests, categorizer prompt recordings and the previously timed-out CLI/gateway tests passed. The 20 skips are existing suite configuration; this change adds none.

One commit: `refactor: memory compaction is the summarizer`. No trailers and no push.
