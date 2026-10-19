# Search eval fixture

`search-eval` (a test-support binary of the `cli` package) scores memory search against a query set and gates the
result on a saved baseline. This directory holds the committed, synthetic eval set that CI runs through
`crates/cli/tests/search_eval.rs`.

- `corpus.json` holds the conversations, messages and summaries seeded into a freshly migrated SQLite memory store.
- `queries.json` is the query set: a pinned `clock`, the result `limit` (k), an optional absolute `tolerance`
  (default 0), and per query an `id`, the `query` text, an optional search `mode` (default `hybrid`; with no embedding
  endpoint it answers keyword-only and reports the degradation), the `relevant` hit ids, and `must_contain`
  substrings of which at least one must appear in a top-3 result.
- `baseline.json` is the saved result: recall@k and MRR over the queries that list relevant ids, and the
  must_contain pass rate.

```sh
F=crates/cli/tests/fixtures/search-eval
cargo run -p cli --bin search-eval -- --corpus $F/corpus.json --queries $F/queries.json --baseline $F/baseline.json --check
cargo run -p cli --bin search-eval -- --corpus $F/corpus.json --queries $F/queries.json --baseline $F/baseline.json --save-baseline
```

`--check` exits 1 when any metric falls more than the tolerance below the baseline, or when the baseline was saved
for a different query count or limit. Without `--check` or `--save-baseline` the run exits 1 when any query misses
its expectations. Errors exit 2. `--sqlite FILE` replaces `--corpus` to score a pinned memory database at the current
schema; the file is copied to a scratch directory first, so the access counts a search records never touch it.
A change that moves ranking on this fixture regenerates `baseline.json` with `--save-baseline` in the same commit.

## Through an MCP server

Everything after `--` is an MCP stdio server command. The harness starts it, checks that it serves `memory_search`, sends
one warm-up call, then calls the tool once per query with `query`, `mode`, `scope: both` and the set's `limit`, reads the
hits back from the rendered listing in display order, and scores them with the same metrics, baseline and exit codes.

```sh
T=target/debug
cargo build -p cli --bin search-eval --bin search-eval-server
$T/search-eval --corpus $F/corpus.json --queries $F/queries.json --baseline $F/baseline.json --check -- $T/search-eval-server {sqlite}
$T/search-eval --queries $F/queries.json -- rh mcp --config config.yaml
```

A `{sqlite}` token in the server command is replaced by the path of the scratch database that `--corpus` seeded or
`--sqlite` copied, so the server searches the same rows the harness knows. `search-eval-server FILE` serves the RivetHub
MCP memory tools over stdio from a SQLite memory database. A listed hit counts as a relevant id when the listing names that
id, or, when the harness holds a database, when the hit shows that row's content from its start (a cut preview ending
in `…` included). Without `--corpus` or `--sqlite` only ids the listing names identify hits. The server ranks against its
own clock rather than the set's `clock`, it lists summaries before messages, and it cuts long messages to a preview, so
its numbers can sit below the in-process ones on the same set: a needle past the cut is not shown, and a relevant
message ranked above a summary in process is listed after it.
