# MCP sidecar recording: memory, wiki and agent tools (B7)

This recording captures the output text of the TypeScript sidecar's memory, wiki and agent-listing tools, so the Rust
sidecar can be checked against it. It complements `../mcp/`, which skipped these tools because a real store would put
private data in the output. Here the sidecar reads a throwaway database that holds only invented rows.

It was recorded 2026-10-16T03:55Z by `record-mcp` (rivethub-runtime 780aa2b plus this directory and
`xtask/data/mcp-script-memory.json`). The server was the TypeScript sidecar in `/opt/rivetos`, started by
`integrations/claude-code/rivet-memory/bin/rivet-memory-mcp.sh` over stdio. It reported itself as `rivetos-mcp-server`
2.0.0, protocol `2025-11-25`, with Postgres transport.

| file | content |
|---|---|
| `meta.json` | recorder version, program name, time, and the server's `initialize` result |
| `tools-list.json` | `tools/list`, sorted by name: 14 tools (memory ×5, wiki ×2, `delegate_task`, `list_agents`, skills ×2, web ×2, `echo`) |
| `calls.json` | 73 calls from `xtask/data/mcp-script-memory.json` (see below) |
| `seed.txt` | the complete seed. Every row is synthetic. |
| `/rivet-shared/plans/rust-runtime/mcp-memory-recording/embed.mjs` | fills the embeddings the way the RivetOS embedding worker does |
| `shared/` | the sidecar's `RIVETOS_SHARED_DIR`: `mesh.json` (synthetic roster) and `wiki/topics/*.md` (the pages `wiki_read` serves) |

## The seed

The seed has three conversations across two agents. All timestamps are fixed, between 2026-09-02 and 2026-09-08 UTC.

- **`opus`, "Kiwi build cache migration"** (`fixture:kiwi-cache`, 8 messages). It includes a tool row whose
  `run_terminal_command` `tool_result` is 7642 characters long (`a0000000-…-000000000003`). That is long enough to be
  display-truncated and to be split into two `ros_message_chunks`. It also includes a capture-truncated tool row
  (`…-000000000007`). Its metadata has `truncated: true`, `full_tool_result_length: 16384` and a
  `session_jsonl_path` that does not exist on the recording host.
- **`opus`, "Lighthouse dashboard latency"** (`fixture:lighthouse-p95`, 7 messages, including a `sql_query` tool row
  and a `system` row).
- **`grok`, "Pelican recipe parser"** (`fixture:pelican-parser`, 5 messages, with non-ASCII `½ ¼ ¾`). It is left
  unsummarized.

There are four summaries. Conversation 1 has a chain: leaf `b…01` → branch `b…02` → root `b…03`, with the leaf sourced
from messages 1–8. Conversation 2 has one legacy `condensed` summary `b…04`, sourced from messages 9–15.

Tags:

- Accepted: `project:kiwi` (rule), `topic:ci-cache` (user) on conversation 1, and `project:lighthouse` on conversation 2.
- Accepted but with a suggested removal: `topic:ci-cache` on conversation 2.
- Suggested: `project:pelican` on conversation 3, and `topic:build-performance` on the leaf summary.
- The taxonomy has four entries.

There are two wiki topics, `kiwi-build-cache` and `lighthouse-dashboard`, with three citations to summary ids. There
are also two `ros_agent_presets`. In `shared/mesh.json`, `fixture-node` is online and hosts `opus` and `grok`, and
`fixture-spare` is offline.

## How it was recorded

The schema is `crates/migrate/migrations` 0001–**0020**, applied in order to a new database `mcp_fixture` on the test
Postgres (pgvector 0.8.0 and pg_trgm). Migration 0020 (`ros_tags.removal_state`) is a TypeScript-era migration that the
deployed sidecar already queries. With 0001–0019 only, `memory_tags` pending fails on a missing column. The seed sets
`rivet.defer_embed_enqueue` because the database has no `graphile_worker` schema.

```sh
for f in crates/migrate/migrations/00{01..20}_*.sql; do psql -h 127.0.0.1 -p 5442 -U rivet -d mcp_fixture -v ON_ERROR_STOP=1 -q -f "$f"; done
psql -h 127.0.0.1 -p 5442 -U rivet -d mcp_fixture -v ON_ERROR_STOP=1 -f crates/testkit/tests/fixtures/mcp-memory/seed.txt
RIVETOS_ROOT=/opt/rivetos PG_URL=postgres://rivet@127.0.0.1:5442/mcp_fixture \
  EMBED_URL=<embed-endpoint> EMBED_MODEL=qwen3-embedding-0.6b node /rivet-shared/plans/rust-runtime/mcp-memory-recording/embed.mjs

cargo build -p xtask --bin record-mcp
F=crates/testkit/tests/fixtures/mcp-memory
env -i PATH=/usr/local/bin:/usr/bin:/bin HOME="$(mktemp -d)" TZ=UTC LANG=C.UTF-8 \
  RIVETOS_ROOT=/opt/rivetos RIVETOS_MCP_TRANSPORT=pg RIVET_DEN_CA=/nonexistent \
  RIVETOS_PG_URL=postgres://rivet@127.0.0.1:5442/mcp_fixture \
  RIVETOS_EMBED_URL=<embed-endpoint> RIVETOS_EMBED_MODEL=qwen3-embedding-0.6b \
  RIVETOS_SHARED_DIR="$PWD/$F/shared" RIVETOS_NODE_NAME=fixture-node \
  target/debug/record-mcp --out "$F" --script xtask/data/mcp-script-memory.json \
  -- /opt/rivetos/integrations/claude-code/rivet-memory/bin/rivet-memory-mcp.sh
```

The environment is isolated from production in four ways:

- `env -i` and an empty temporary `HOME` mean no `~/.rivetos/.env`, `config.yaml`, users registry or skills are read.
- `RIVETOS_MCP_TRANSPORT=pg` and a missing `RIVET_DEN_CA` stop the launcher's default den URL from being used.
- `RIVETOS_SHARED_DIR` points the mesh roster, users registry and wiki directory at `shared/` here.
- `TZ=UTC` makes the sidecar's local-time rendering come out in UTC.

`<embed-endpoint>` was the lab's OpenAI-compatible `qwen3-embedding-0.6b` server, which returns 1024 dimensions. The
sidecar embeds queries itself, using the Qwen3 instruct prefix. So the vector and hybrid arms ran for real, and no call
shows the "vector arm unavailable" banner. `record-mcp` masked nothing.

## The calls

- **`memory_search`** (32 calls):
  - every mode: hybrid, fts, trigram (miss and hits), regex (hit and invalid pattern), and vector (including a
    `…[chunk 2/2 · chars 5990-7670 → memory_get_full id=…]…` chunk hit);
  - the long tool hit, display-truncated, with a `memory_get_full` handle;
  - the capture-truncated row with its `⚠ truncated at capture` line;
  - expanded summaries (children and source messages), and unexpanded ones with `expand: false`;
  - `scope` set to messages or summaries;
  - a tag filter (accepted, suggested-only, and a malformed `tag` that is not key:value);
  - since/before windows, with hits and with an empty result, and `window` returning empty;
  - agent filters;
  - limit 1 and 50, and the validation errors for 0, 51, 2.5, an unknown mode and a missing query;
  - a no-match query.
- **`memory_browse`** (10 calls): the default view, one conversation with `include_tools`, an agent filter with a limit
  hint, since/before, a tag filter, an empty window, an unknown id, a malformed id, and limit validation.
- **`memory_get_full`** (8 calls):
  - a complete long tool row;
  - the capture-truncated row, which gets the "not readable from this host" guidance;
  - a short message;
  - a summary id (the tool reads `ros_messages` only, so this gives "No message with id");
  - an unknown id, a malformed id, an empty id, and a missing id.
- **`memory_stats`** (2 calls): the whole store and `agent=grok`.
- **`memory_tags`** (10 calls): pending (default, and with a limit); list by entity, by key and value, and by state;
  counts; lookup; taxonomy; a refused `decide` (the surface is read-only); and an unknown action.
- **`wiki_search`** (4 calls), **`wiki_read`** (6 calls: verbatim page, `section=summary`, `section=citations`, red
  link, invalid slug, unknown section), and **`list_agents`** (1 call).

`delegate_task` is listed but was never called.

## What varies between recordings

Some text depends on when or where the recording ran:

- Relative ages in search hits and stats freshness (`43d ago`, `Newest message: 37d ago`) depend on the recording date.
- Browse output with `window=today` shows that day's UTC midnight.
- Scores in `vector` and `hybrid` results come from the embedding model. They and the order of near-tied hits can move
  slightly between runs.
- The "Search runtime" counters in `memory_stats` (query-embed cache hits and misses, and arm hit totals) count every
  search earlier in the same session. They depend on call order.

Some details are TypeScript behaviour rather than seed artifacts:

- Hybrid search with `agent=grok` still returns opus summaries, because summaries are not filtered by agent.
- Explicit `trigram` matches whole-string `similarity > 0.3`, so a token buried in a long row does not hit.
- `wiki_search` never prints its "No wiki topics match" text while topics have embeddings: the vector arm always ranks
  every topic.
