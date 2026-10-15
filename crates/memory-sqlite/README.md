# memory-sqlite

`SqliteStore` is the standalone SQLite adapter for the kernel memory ports. It opens the schema v7 file created by `rivethub-migrate sqlite`. Every statement runs on `spawn_blocking` through `with_conn`.

The store implements `kernel::memory::MemoryStore`, `kernel::memory::EmbeddingStore`, and `kernel::memory::SummaryStore`. The job queue is the existing `JobQueue` impl in `port.rs`.

## Storage schema v7 does not have

`EmbeddingStore` accepts messages, summaries, and wiki topics. It refuses `EmbedTable::MessageChunks` and any `StoredEmbedding` whose `chunks` are not empty. Schema v7 has no `ros_message_chunks` table. Those calls return `SqliteError::MissingTable` (`schema v7 has no ros_message_chunks table`), which the port maps to `EmbeddingStoreError::Query`.

`StoredEmbedding.content_hash` is not stored. `ros_messages`, `ros_summaries`, and `ros_wiki_topics` have no `content_hash` column. A successful write still stores the little-endian `f32` vector, sets `embed_status` to `done`, clears `embed_error`, and sets `embed_failures` to 0.

When `ros_meta` has `embed_dims`, a vector of a different width is `EmbeddingStoreError::Width`. When that key is unset, the write accepts the vector and does not insert a meta row.
