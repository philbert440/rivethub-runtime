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

## memory-sqlite: kernel memory ports (2026-10-14)

`SqliteStore` implements `MemoryStore`, `EmbeddingStore`, and `SummaryStore` on the v7 file `rivethub-migrate sqlite`
creates, plus the existing `JobQueue`. Inherent methods remain: `capture`, `append_entries`, `browse_at`, `get_full` /
`message_entry`, `stats_for`, `search_at`, `with_owner`. Every SQLite call runs in `with_conn` (spawn_blocking). The trait
`context` uses the `now` argument; tag and taxonomy writes stamp with the clock chosen when the store is built (`Utc::now`, or `with_clock`). No `unwrap_or`
defaults: an absent channel, settings or metadata column is left out of the INSERT so the v7 schema default applies
(messages take their conversation's stored channel), and an absent source timestamp is the typed `Stamp::CaptureTime`.

Schema v7 has no `ros_message_chunks` table and no `content_hash` column on messages, summaries, or wiki topics.
`EmbeddingStore` refuses chunk targets and non-empty chunk lists, and ignores `content_hash`. See the crate README.

Capture rules replayed from `capture.json`: dedupe by `metadata.event_id` within (session_key, agent); a stored
`pending_result: true` row is filled in place by the same event with a result; a `cowork-transcript` message claims an
unclaimed `cowork-hook` row with the same role and content (rewrites its metadata, counts as skipped); title, settings and
task id change only when supplied; `updated_at` = max(stored, supplied or now); finalize clears `active`; content and tool
result cap at 16000 chars with `truncated`/`full_*_length` metadata; bad timestamps are `InvalidRequest` before any write.
Append: no dedupe, title `Session <key>` / `Heartbeat <agent>`, task id from `task:<uuid>` keys or `metadata.taskId`.

Deviations (in the replay tests as data): `title: null` cannot clear a title (wire `Option<String>`, and B3 rejects null);
lengths are chars, not UTF-16 units. `decide_tags` returns a count where the recording returns ids. Taxonomy list order
is key then value. Context has no `maxTokens` argument. Chunk storage and `content_hash` are absent from schema v7.
Still skipped, with a reason a test checks: project-rule tagging, health, history/settings reads, the hybrid search
scenario, summarizer/wiki/tagger/embedder job loops, `get_full` of summaries, and the MCP tool layer.
Gotcha: the recordings do not carry inputs; the replay tests rebuild them from labels and the recorded rows, and match
generated ids by role (a bijection), never by text.

## harness-acp: degraded headless providers (2026-10-16)

A CLI-backed provider whose harness fails ACP discovery no longer stops startup. `HeadlessLaunch::prepare(..).start(Supervisor)`
returns a `HeadlessHarness` (a `HarnessAdapter`) that is ready or degraded; transient failures (timeout, launch I/O, closed
pipes) retry under the factory's `TaskTracker` and child `CancellationToken` with 1 s → 60 s backoff, permanent ones
(`AcpError::recovery()`) do not. Turns on a degraded provider fail fast with `AcpError::ProviderDegraded`. Readiness is a
kernel `ProviderHealth` watch the gateway shows under `/healthz` `degradedProviders` and `rh status` prints.
`providers.<id>.discovery_timeout_ms` bounds discovery (default `harness_acp::DEFAULT_DISCOVERY_TIMEOUT`, 30 s);
`timeout_ms` bounds turns only.

## capture listing: on-disk harness sessions (2026-10-17)

`kernel::transcript::SessionLister` (list / describe / store_cwd) is implemented by `capture::listing::StoreSessionLister`
over `StoreRoots` (resolved in `rivethub::composition::store_roots` from `config::Environment`); the gateway holds it via
`Gateway::with_session_lister` / `session_lister()`. No routes yet. Rows are the kernel's `StoreSession` (native id, harness,
title, epoch-ms times, labels); `StoreSession::summary(cwd, now)` maps one row to `HarnessSessionSummary`, and
`ListLimit::from_query` parses `?limit=`. `list` runs harnesses in the fixed claude, grok, hermes, kimi, codex, opencode, pi,
qwen, cursor, cowork order, a failing harness lists as empty, then a stable global `updatedAt` sort and `with_ancestors`.
Disk JSON is read key by key through `listing::field::Field` (a `Box<RawValue>`), so a wrong-typed key never fails a line.
Walks skip unreadable or refused entries instead of failing the harness. Grok keeps a per-file `updates.jsonl` cursor.
Reader fixes: kimi reads `session_<uuid>` listing ids, pi lists flat `sessions/*.jsonl`, pi ids need a hyphenated UUID.
Not here: delegated-task nesting (5.4), `capabilities.listSessions`, the codex protocol exclusion (routes slice).

## session history routes (2026-10-17)

Served from `gateway::routes::history` over the `SessionLister` port, which now also has `transcript(harness, native)`
(`TranscriptRead::Read(StoreTranscript)` or `NoSource`). `capture::listing::history` holds the Claude JSONL folder (tail
window, sidechains, compact markers, tools, usage) and text-only `{role,text}` turns for the other stores through
`StoreReader`; cowork has no source (501). `gateway::history` does `:enc` resolve (bare UUIDs probed in offering order and
memoized), `ensure_nested_owner`, the store→summary mapping and the context stamp (learned compactAt per window, in memory).
`GET /api/harnesses` forces `listSessions` when a lister is wired. Live chats publish `HarnessEvent`s per native key through
`TerminalService::publish_harness` (fan-out in `sessions::fanout`), and their summary status follows active/idle/ended/error.
`HarnessSessionSummary` times serialize as ISO millis `Z` (`wire::time::Millis`). The owners file is
`<state_dir>/den/session-owners.json` (`config::paths::session_owners_file`).
Not here: resume/turns/interrupt (b6-b8), the legacy `/api/sessions/ws` (b9), the registry WS (b11), the codex
protocol exclusion (inert), `installed`. The old `/term/harness-sessions/{session}/transcript` still uses the Claude-only
`TranscriptReader` (its golden replays pin the old shape).
Gotcha: test files that use `expect` in helpers start with `#![cfg(test)]`; non-wire tests cannot use `json!`/`Value`.

## gateway: agent presets from the memory database (2026-10-17)

`/api/agents` runs over the kernel port `kernel::agent_presets::AgentPresetStore`. The gateway `Registry`
(`routes/agents/registry.rs`) keeps every validation, directory and `rivet-shared` link rule and calls the port; the
agents.json thread is now one adapter (`FilePresets`), `memory_postgres::PostgresPresets` the other. Composition passes
`rivethub::composition::agent_presets(&PostgresMemory)` into `Composition::build` on a Postgres node: Postgres when
`rh_agent_presets` exists, otherwise (and on SQLite nodes) the file store. The list `backend` field names the store.
Gotcha: the unique name index kept its pre-0021 name `idx_ros_agent_presets_name`; its violation maps to the same 409 the
file store gives. The task agent roster (`rivethub/src/agent/assemble.rs`) still reads agents.json.
