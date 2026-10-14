# Wiki gateway lane

## Implemented

The wiki module serves topic list/search at `/api/wiki`, `/api/wiki/topics`, and
`/api/wiki/search`; page reads at `/api/wiki/page?slug=...` and
`/api/wiki/page/{slug}`; gaps at `/api/wiki/gaps`; and HTML at `/wiki`, `/wiki/`,
and `/wiki/{slug}`. HTML includes search, pagination, summaries, article text,
tags, related links, supplied history, and citations. All text is escaped and
HTML responses have a restrictive content security policy. Markdown is shown
as escaped text rather than interpreted as HTML.

All routes use the existing gateway guard. No auth, tenancy, origin, TLS, or
preflight policy was reimplemented. Query types use serde. Validated slugs
reject traversal segments, separators, empty values, and control characters;
spaces, Unicode, quotes, and substitutions remain legitimate input. Redirects
use URL-encoded internal wiki paths and refuse corrupt targets and self loops.

The memory port now exposes complete topic, page, and gaps reads. PostgreSQL
implements all three with bound parameters and typed rows. Pages include
citations, provenance, related topics, verification time, and a Markdown
projection of stored fields. Topic totals and pagination use one repeatable-read
snapshot. Six static queries have offline SQLx metadata. The existing complete
memory fake implements the added operations with separate state and short locks.

## Integration prerequisites completed

The original checkout lacked wiki methods on `MemoryStore`. Completing the
follow-up required adding the methods and their PostgreSQL implementation.
SQLite does not implement `MemoryStore` in this checkout; no partial
implementation was added. The PostgreSQL/HTTP integration tests live in the
composition crate; the gateway has no dependency on concrete adapters.

Router composition originally lived in `Gateway::router`. One line in
`gateway/src/lib.rs`, `.merge(routes::router())`, connects the new hook; the wiki
mount is in `routes/mod.rs`. No other shared production gateway code changed.

## Boundary evidence and limitations

The permitted gateway fixtures have no wiki request exchanges or recording
README. `gateway/tests/golden/wiki/skips.jsonl` lists all 303 HTTP/WS cases by
name and reason. A test checks exact coverage, uniqueness, nonempty reasons,
and refuses to skip future wiki HTTP cases. The two users files are auth
configuration inputs, not traffic recordings. The added README distinguishes
new tests from historical recordings.

The hard prohibition on reading the rewrite plans tree was followed; the
inventory was not opened. Historical wiki boundary parity remains unverified
without accessible wiki recordings or the boundary specification. The read
surface follows the existing wiki wire types and database schema.

Local B1 defaults: absent query lists all topics; absent limit is 50; limits
clamp to 1–200; absent offset selects the first page; excerpts contain at most
240 Unicode characters; missing-page suggestions contain at most five topics.
These are explicit choices, not claims about unavailable recordings. The
statement-level Clippy expectation documents the absent-offset default;
invalid offsets remain serde errors.

No filesystem wiki reader was added, so directory traversal, shared-prefix
siblings, and symlinks cannot reach files. PostgreSQL has no history body or
original Markdown/frontmatter column: page history is empty and Markdown is
projected from title, current state, and article. Wiki editing/generation,
filesystem history, rich Markdown rendering, and new auth functionality are
outside this read-only slice.

## Validation

Lane PostgreSQL connectivity and SQL query validation succeeded. Twelve focused
route and PostgreSQL tests initially passed. Coverage includes typed payloads,
search/pagination, Unicode excerpts, citations/provenance, redirects, missing
pages, gaps, escaping, invalid parameters, SQL binding, absent tables, store
failures, and the existing auth guard. Tests await requests with timeouts.

Final validation with the lane PostgreSQL URL:

- `SQLX_OFFLINE=true cargo xtask check --locked` exited 0. Last line:
  `GATE OK: 0 finding(s), cargo xtask check`
- `cargo nextest run --workspace --all-features` exited 0. Last line:
  `Summary [  16.466s] 579 tests run: 579 passed, 9 skipped`
- All 13 new wiki tests passed: eight route tests, four PostgreSQL/composition
  tests, and the recording-scope coverage check. The nine nextest skips are
  pre-existing tests; the separate manifest accounts for 303 out-of-slice
  recorded exchanges.

The full gate initially rejected a gateway test-only dependency on the concrete
PostgreSQL adapter. The integration tests were moved to the composition crate,
the dependency removed, and the complete gate rerun successfully without any
policy changes. Temporary files for the final commands were directed inside
the repository's ignored target directory.

Notes stay in the repository because the hard rule prohibits the requested
external write. No package installation, service reconfiguration, prohibited
external reads, or Git branch/configuration/remote changes were performed.
