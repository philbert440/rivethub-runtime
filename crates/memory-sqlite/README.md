# memory-sqlite

`SqliteStore` is the standalone SQLite adapter for the kernel memory ports. It opens the schema v8 file created by `rivethub-migrate sqlite`. Every statement runs on `spawn_blocking` through `with_conn`.

The store implements `kernel::memory::MemoryStore`, `kernel::memory::EmbeddingStore`, and `kernel::memory::SummaryStore`. The job queue is the existing `JobQueue` impl in `port.rs`.

## Storage schema v7 does not have

`EmbeddingStore` accepts messages, summaries, and wiki topics. It refuses `EmbedTable::MessageChunks` and any `StoredEmbedding` whose `chunks` are not empty. Schema v7 has no `ros_message_chunks` table. Those calls return `SqliteError::MissingTable` (`schema v7 has no ros_message_chunks table`), which the port maps to `EmbeddingStoreError::Query`.

`StoredEmbedding.content_hash` is not stored. `ros_messages`, `ros_summaries`, and `ros_wiki_topics` have no `content_hash` column. A successful write still stores the little-endian `f32` vector, sets `embed_status` to `done`, clears `embed_error`, and sets `embed_failures` to 0.

When `ros_meta` has `embed_dims`, a vector of a different width is `EmbeddingStoreError::Width`. When that key is unset, the write accepts the vector and does not insert a meta row.

## Search

Schema v8 adds `rh_messages_trigram` and `rh_summaries_trigram`, with backfill and insert/update/delete triggers. The existing `ros_*` tables remain the B5 compatibility schema. Apply the migration with `rivethub-migrate sqlite` before opening the store; the migration preserves a versioned backup.

Inject `QueryEmbed` through `SqliteStore::with_query_embed`. It accepts the kernel `ModelProvider`, a query instruction, a timeout, and a shutdown token. Queries use the kernel input composer and normalize the returned vector before scoring little-endian float32 BLOBs. Malformed stored vectors return an error. Missing or failed query embedding produces an explicit degraded notice; cancellation stops the search.

Hybrid search interleaves message and summary candidates within each arm, preserving SQLite's recorded source balance. The kernel supplies reciprocal-rank fusion, the summary bonus, recency/importance boost, and relevance gate. Exact literals use the trigram index as an additional arm. Short prose is excluded from hybrid candidates; tool results remain eligible. Explicit vector mode ranks by dot product and uses the shared composite score. Unconfigured search retains the recorded BM25 behavior and degraded notice.

Explicit trigram mode uses word-padded, case-insensitive set similarity with the Postgres `> 0.3` threshold. It scans the trigram virtual tables to preserve short-word and word-boundary matches that an internal-trigram MATCH alone would miss. Vector search likewise scans stored vectors; neither path is an approximate nearest-neighbor index.

REGEXP uses the Rust regex crate with a 1 MiB compiled-expression limit and bounded DFA cache, case folding, and newline matching. Invalid or oversized expressions are typed errors. PostgreSQL-specific regex extensions outside the regex crate's syntax are rejected; Unicode case folding follows the regex crate rather than the database server's locale.

`search_at` receives its calendar zone explicitly. The `MemoryStore` port uses UTC. The composition root can wire the query embedder through the builder without coupling store adapters.
