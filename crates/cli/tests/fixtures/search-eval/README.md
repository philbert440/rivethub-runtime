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
