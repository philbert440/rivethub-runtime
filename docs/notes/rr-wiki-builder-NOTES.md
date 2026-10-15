# Wiki builder

The workers service exposes `Runner::with_wiki_builder(WikiBuilderDeps)` for the composition root. The injected store implements the complete kernel `WikiBuilderStore` port. RivetHub composition files are untouched.

`extract-wiki` accepts typed `{summaryId, conversationId?}` jobs with `wiki-ext-<summaryId>` keys. The provider receives a new extraction prompt and a JSON Schema generated from `WikiPatch`. Model output is parsed through serde. `enqueue-wiki-backfill` selects unprocessed summaries, failed extractions, and `done` extractions below `WIKI_PIPELINE_VERSION = 4`; current and future completed versions and skipped backfill entries remain untouched. The existing ten-minute cron definition is reused.

The Postgres adapter atomically writes topics, provenance, citations, redirects, and extraction completion. Summary row locks make duplicate commits idempotent. Ordered advisory locks coordinate overlapping slug and alias changes without serializing unrelated topics. Conflicting aliases roll the whole patch batch back. Omitted articles and related links preserve existing values. An unchanged topic receives provenance without invalidating its embedding. Changed topics clear stale embedding state and enqueue `embed-target`. Completion retains the changed slugs so a delivery retry does not repeat model inference or history changes. Cancellation interrupts provider calls and database work; no cancellation failure marker is written.

The existing Graphile adapter needed corrections exposed by the vendored queue fixture: use the nine-argument `add_job` returning a composite job row, supply cron attempt counts, cast claim attempts to one checked integer type, and delete completed jobs only when the claiming worker owns them. These changes are covered through the wiki runner and queue integration tests.

Compatibility names are explicit: the existing `EmbedTable::WikiTopics` emits the contract-pinned `ros_wiki_topics` target label; persisted tables use `rh_*`. Test fixture decoding uses the recorded `ros_summaries` key. Test seeding uses the existing transaction-local `rivet.defer_embed_enqueue` compatibility setting to isolate summary embedding triggers from wiki extraction.

## Coverage

Every summary-producing scenario in the supplied summarizer recording is replayed through a private SQLx test database and a scripted provider. The one scenario with no source summaries is skipped by exact name with a checked reason. Tests cover create, alias updates, citations and provenance, article preservation, unchanged patches, rollback on collisions, conversation ownership, malformed and truncated responses, empty extractions, versioned backfill, duplicate concurrent commits, late failures, retrying embedding delivery, cron dispatch, runner shutdown, and cancellation before and during inference. No tests use a live model. Database integration tests live under the CLI composition crate because repository dependency policy excludes SQLx and concrete adapters from service dev-dependencies as well as runtime dependencies.

## Scope and instruction conflicts

Consolidation, recompilation, stale-page maintenance, filesystem pages, and git publishing are outside this extraction slice. The Postgres wiki role claims only extraction and backfill jobs and registers only the backfill cron, leaving other wiki jobs for workers that implement them. The orchestrator supplies the store and model using the builder method.

The final hard rules prohibit reading the requested external contract directory and writing the requested external notes destination. Those files were not read or written. The permitted design HTML, repository schema, queue definitions, patterns, recordings, and README supplied the implementation evidence. This file is the repository-local substitute for the requested notes.

## Validation

The focused wiki and worker runner run passed all 27 tests. The new wiki coverage consists of 15 tests, including 13 private SQLx database tests. Formatting and staged whitespace checks pass.

`SQLX_OFFLINE=true cargo xtask check --locked` passed on the final source tree: `GATE OK: 0 finding(s), cargo xtask check`. This includes the final recording assertion comparing parsed JSON values.

Workspace attempts with default concurrency, four test threads, and one test thread encountered existing deadline failures in CLI process, gateway serving, and gateway audio tests. The final workspace run uses `NEXTEST_TEST_THREADS=1 NEXTEST_RETRIES=3`; no tests or repository runner configuration were changed to accommodate these failures.

`cargo nextest run --workspace --all-features` completed successfully with the environment above: `Summary [1021.991s] 1181 tests run: 1181 passed (1 slow, 2 flaky), 16 skipped`. The existing nextest output configuration suppresses retry details; the final summary reports two tests passing on retry.
