# AGENT.md — RivetHub runtime

## State (2026-10-12)

Repository created from the design in `docs/`. On `main`: the guardrails, and `config`, `wire`, `kernel`, `testkit` (Wave 0).
The repository is Rust only: the per-harness hook kits and the Node installer stayed in the RivetOS monorepo (LANGUAGES
allow-list). The gate is `cargo xtask check`; the design rules that need names, types or whole-program knowledge are Dylint
lints in `lints/` (run by `cargo dylint`, pinned nightly), everything that is a property of Rust code lives in
`[workspace.lints]` and `clippy.toml`, dependency direction is `deny.toml` bans with wrappers, and everything else is an
`xtask` check.

## How work happens

1. One crate = one lane = one builder brief (briefs live outside this repo with the recordings). The builder reads
   `docs/CONTRACT.md` row(s), the inventory section and the recording for the crate — never the TypeScript source.
2. Done means `cargo xtask check` green (it runs fmt, clippy `-D warnings`, `cargo dylint --all`, `cargo deny check` and the
   xtask checks), tests, replays, `coverage-floor.txt` not lowered, one Conventional Commit under a house identity, no
   attribution trailers.
3. Reviewer: Claude Opus, Rust idiom and quality only; boundary acceptance is mechanical.

## Gotchas

- The layer map lives in `deny.toml` `[bans]` deny entries with wrappers. A new crate is a design change first
  (docs/ARCHITECTURE.md), then a ban entry whose wrappers are exactly the crates allowed to depend on it.
- The lint policy is `xtask/lints-policy.toml`; `[workspace.lints]` must equal it (xtask `LINTPOLICY`). Only a reviewed
  policy change may touch either.
- Git hooks are the xtask binary, not scripts: `cargo xtask install-hooks` copies the built binary to
  `target/git-hooks/{pre-commit,commit-msg}` and sets `core.hooksPath`; it dispatches on the name it was invoked as.
- `clippy.toml` `disallowed-methods` blocks `std::env::var` everywhere; the `config` crate reads the environment through
  `std::env::vars_os()` once.
- Privacy and secret scans use hashed denylists in `xtask/data/`; add a hash, never the plaintext.
- New Dylint lint: write it in `lints/src/`, register it in `register_lints`, add the UI test (`lints/ui/<name>/` with one
  firing and one passing case), add it to `xtask/src/patterns.rs` and give it a `docs/PATTERNS.md` entry.

## memory-sqlite: write and read slice (2026-10-13)

Built on the v7 file `rivethub-migrate sqlite` creates: `SqliteStore::capture(batch, now)`, `append_entries(entries, now)`,
`browse_at(filter, now)`, `get_full(id)`, `stats_for(agent)`, `with_owner(..)` — inherent methods only. `impl MemoryStore`
waits until search, context and tags exist (never implement a trait partially). Every SQLite call runs in `with_conn`
(spawn_blocking); time is an argument. No `unwrap_or` defaults: an absent channel, settings or metadata column is left out
of the INSERT so the v7 schema default applies (messages take their conversation's stored channel), and an absent source
timestamp is the typed `Stamp::CaptureTime`.

Capture rules replayed from `capture.json`: dedupe by `metadata.event_id` within (session_key, agent); a stored
`pending_result: true` row is filled in place by the same event with a result; a `cowork-transcript` message claims an
unclaimed `cowork-hook` row with the same role and content (rewrites its metadata, counts as skipped); title, settings and
task id change only when supplied; `updated_at` = max(stored, supplied or now); finalize clears `active`; content and tool
result cap at 16000 chars with `truncated`/`full_*_length` metadata; bad timestamps are `InvalidRequest` before any write.
Append: no dedupe, title `Session <key>` / `Heartbeat <agent>`, task id from `task:<uuid>` keys or `metadata.taskId`.

Deviations (in the replay tests as data): `title: null` cannot clear a title (wire `Option<String>`, and B3 rejects null);
lengths are chars, not UTF-16 units. Skipped: project-rule tagging, embedding jobs, health, history/settings reads,
search, tags, summaries in get_full, and the MCP tool layer (text rendering, argument checks, event-id hashing).
Gotcha: the recordings do not carry inputs; the replay tests rebuild them from labels and the recorded rows, and match
generated ids by role (a bijection), never by text.
