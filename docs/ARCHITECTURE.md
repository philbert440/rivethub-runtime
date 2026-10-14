# RivetHub Rust runtime — architecture (text twin of the design artifact, for builders)

Design artifact (visual, authoritative): https://claude.ai/artifact/L7cAvh16cYEZDhzPy4FHCd. Contract: CONTRACT.md. Rules: BUILDER-RULES.md.

## Layers and the dependency rule

| Layer | Crates | May depend on |
|---|---|---|
| foundation | `wire` (serde types for every boundary payload), `config` (config.yaml + env → `Config`) | external crates only; `config` also uses `wire` for the shared closed sets (`HarnessId`, `ThinkingLevel`, effort) |
| kernel | `kernel` (ports = traits, domain types; no I/O), `testkit` (dev-only: replayers, fakes, stubs) | wire, config |
| adapters | `memory-postgres`, `memory-sqlite`, `providers`, `harness-acp`, `terminal`, `mcp`, `capture`, `pki` | kernel, wire, config, externals — NEVER another adapter or a service; `pki` uses kernel/wire and externals only |
| services | `gateway` (with the session registry and mesh), `agent-loop`, `workers`, `tasks` (sessions over the gateway API) | kernel, wire, config — NEVER an adapter |
| operations | `migrate` (every Postgres and SQLite schema change; the `rivethub-migrate` binary) | wire, config, externals (sqlx, rusqlite); depended on only by `cli` as a normal dependency and by tests as a dev-dependency — NEVER directly by a runtime crate (`rivethub` is checked transitively by the xtask MANIFEST rule, except for the direct `cli` → `migrate` edge used for schema verification) |
| composition | `cli`, `rivethub` (binary; one composition root per service process) | everything |

The gate (`cargo xtask check`) fails any Cargo.toml that breaks this.

## Ports (kernel traits) and who implements them

| Port | Methods (shape) | Adapters | Callers |
|---|---|---|---|
| `MemoryStore` | append(batch) · search(query) · browse(filter) · stats · get_full(id) · tags(action) · context(session) | memory-postgres, memory-sqlite | gateway, agent-loop, workers, mcp |
| `EmbeddingStore` | load(target) · write(embedding) · mark_unembeddable(target, reason) · record_null(target) · mark_permanent(target) | memory-postgres (`rh_messages`, `rh_summaries`, `rh_message_chunks`, `rh_wiki_topics`); memory-sqlite is a later slice | workers |
| `JobQueue` | enqueue(job: name, payload, key, key_mode, priority, run_at) · claim(role) · complete/fail | memory-postgres (graphile tables), memory-sqlite | memory adapters, workers, tasks |
| `ModelProvider` | chat(request) → response · stream(request) → chunk stream (text, reasoning, tool-call deltas, usage) · embed(texts) · list_models | providers: ONE OpenAI-compatible client (vLLM, SGLang, hosted OpenAI-compatible APIs; no sampling fields on the wire) + cli-backed over a headless ACP session; no native Anthropic/Gemini/xAI clients | agent-loop, workers, memory search (query embedding) |
| `HarnessAdapter` | new_session(spec) · resume(id) · prompt(id, input) · cancel(id) · answer_permission(id, decision) · set_option(id, model/effort) · events(id) → SessionEvent stream | harness-acp (one ACP client + a per-harness launch table: native ACP, or codex-acp / claude-code-acp) | gateway's session registry |
| `TerminalMux` | spawn(session, argv, cwd, size) · attach(id) → byte stream · input(id, bytes) · resize · kill · list | terminal (herdr over a PTY; the only multiplexer) | gateway |
| `Tool` | definition() → `rmcp::Tool` · call(args, ctx) → `rmcp::CallToolResult` (the MCP tool shape, no translation) | RivetHub's own capabilities (memory, wiki, delegation, skills) in mcp; remote MCP servers' tools via the mcp client | agent-loop, mcp server |
| `TranscriptReader` | sessions(store) · read(session) → turns · tail(session, since) | capture's store readers (one per harness) | capture (incremental + backfill), gateway (transcript route) via the binary's wiring |
| `CertificateAuthority` | issue_token(principal) · sign(CSR, token or verified renewal principal) · chain · crl · revoke(principal) | pki: holder-generated P-256 keys, SPIFFE SAN policy, short-lived certificates, one-time enrollment, standard X.509 CRLs, one-time legacy CA import | gateway `/ca/v1/`, cli `rh ca` and `rh cert renew` |

A port exists only where several implementations exist or where it is the boundary to an external system. Dropped:
`SessionRegistry` (one implementer, one in-process caller — the registry is a `gateway` module; the agent and task processes
reach sessions over the HTTP API), `Env` (config owns the environment), `Fs` (direct I/O; tests use temp directories),
`Clock` (functions take `now: DateTime<Utc>` as an argument where time is data; timers use `tokio::time`, paused in tests).

## Process shape (Phil 2026-10-12)

One process per service, each with one tokio multi-thread runtime built in its own composition root:
`rh gateway` (gateway with its session registry + terminal + the harness adapter), `rh agent` (agent-loop + mesh + tasks),
`rh worker --role <embedder|summarizer|categorizer|wiki-builder|all>`, `rh mcp` (sidecar). `rh start/stop/status`
keep their CLI contract by supervising those processes. Processes communicate only through contract boundaries (HTTP API B1,
Postgres queue B4/B13). Inside a process: supervised tasks under one `CancellationToken` tree; `parking_lot` for short state,
`tokio::sync` across awaits, `spawn_blocking` for blocking work; `tracing` for logs.

## Session model (SESSION-MODEL.md, Phil 2026-10-12)

One `SessionId` (serialized exactly as today's session-id strings). Lifecycle new/resume/prompt/cancel/end. One typed
`SessionEvent` stream mapped to ACP `session/update` kinds and emitted to RivetHub clients clients in today's v1 wire shape. Chat runs over
ACP only: natively (grok, hermes, kimi, opencode, qwen, gemini, copilot) or through the codex-acp and claude-code-acp
adapter binaries; pi and cursor are terminal-only until they have ACP. Terminal = PTY under herdr attached to the same SessionId.
Multi-client harnesses share one agent session; single-client ones have one owner (Idle|Chat|Terminal) and switch by turn-end
handoff (TurnEnded from the protocol or the harness's own Stop hook; queued chat messages delivered after the handoff; never
keystroke injection, never screen scraping). Transcript stores are read only to capture sessions run outside RivetHub.

## Standard crates

axum + hyper + tokio-tungstenite · reqwest (rustls) · sqlx + pgvector · rusqlite (bundled) + sqlite-vec · serde, serde_json, serde_yaml ·
clap · agent-client-protocol (ACP) · rmcp (MCP) · chrono · regress (user-supplied ECMAScript regex) · tokio::process,
portable-pty · rcgen + rustls-webpki · thiserror · tracing · tokio-util (CancellationToken) · parking_lot · schemars (tool schemas).

## Targets (production lines)

wire 3,000 · config 2,000 · kernel 800 · testkit 1,200 · gateway 8,200 · agent-loop 2,600 · workers 4,000 ·
memory-postgres 5,000 · memory-sqlite 4,000 · providers 900 · harness-acp 1,600 · terminal 2,000 · mcp 2,400 ·
capture 4,500 · pki 1,200 · tasks 2,000 · cli 4,900 · rivethub 500 · migrate 1,000. Total ≈ 50,000 against ≈ 120,000 lines of
TypeScript. Not separate crates: hooks (agent-loop), agent presets (gateway), wiki builder (workers),
harness store readers (capture). Certificate authority signing and policy are in the `pki` adapter behind
`kernel::ca::CertificateAuthority`; CLI only hosts commands and holder renewal. No local-db: standalone runs on SQLite (sqlite-vec as schema v8
after cutover). Workflows stays in the monorepo as a separate workspace package with its own binary (`rivethub-workflows`: depends on
`wire` + an HTTP client only, drives sessions and tasks over the public API, never linked into `rivethub`). The RivetHub clients clients and
sites stay under `apps/`. The chat-channel bots and CFO tooling live in their own private repos (rivethub-channels, rivet-cfo). No tools crate: the TypeScript's built-in shell/file/search/interaction/web tools are deleted (unused; every
harness has its own).

## Build order

Wave 0 foundation (wire, config, kernel, testkit; Phil reviews the types) → Wave 1 memory path (memory-postgres, gateway
skeleton, capture with its readers; B3 B4 B6) → Wave 2 sessions (gateway's session registry + session/term routes, harness-acp, terminal; B1 B2
B12) → Wave 3 agent (providers, agent-loop, mcp; B7 B8 B15) → Wave 4 platform (workers, tasks, 
mesh, memory-sqlite; B5 B13 B14) → Wave 5 operations (cli incl. CA, standalone on SQLite, update/doctor/service; B9 B10 B11
B16) → cutover. Each wave's gate is a boundary working end to end against the recordings.

Standalone: `rh local` composes every service into one process on the SQLite store (one composition root, same crates).
Mesh (registry, heartbeat, agent channel) is a gateway module; ssh helpers are in cli; mesh.json types in wire. 19 crates.
Schema changes never run inside the runtime: `rivethub-migrate postgres|sqlite` applies them (Postgres from the
`migrate` crate's `sqlx::migrate!` chain, SQLite through the v7 creation and the v1→v7 upgrade ladder with a
`<file>.v<N>.bak` copy of the original file before any read-write connection), and the stores verify one schema version each
(`memory_postgres::SCHEMA_VERSION`, `memory_sqlite::SCHEMA_VERSION`) and refuse to start behind it.
`migrate`'s line budget is 1,000, not the 600-line file ceiling: the crate now holds the moved ladder and the
`rivethub-migrate` binary. Measured `crates/migrate/src` is 874 lines (schema 215, the binary 210, postgres 155,
upgrade 128, version 91, error 50, lib 25); 1,000 leaves headroom. Every file stays under the 600-line file cap.
No runtime plugin loading: extension points are adapter crates behind ports, the public API, and MCP. `plugins:` and
`channels.*` are accepted-and-ignored compatibility keys.


## Certificate authority (approved 2026-10-14)

The CA service keeps its intermediate key only in its private state directory, mode 0600. Holders generate their own
ECDSA P-256 keys and submit CSRs. Identity is exactly one SPIFFE URI SAN: `spiffe://rivet.mesh/node/<node>`,
`spiffe://rivet.mesh/agent/<node>/<agent>`, or `spiffe://rivet.mesh/device/<user>/<device>`. Node/agent certificates live
seven days; device certificates live thirty. First issuance requires a CSPRNG enrollment token bound to one principal,
valid fifteen minutes, stored hashed and consumed once. Renewal requires the caller's verified current principal and a
matching CSR. Holders schedule renewal at two-thirds of lifetime with a new key/CSR.

The gateway serves `POST /ca/v1/sign`, `GET /ca/v1/chain` and `GET /ca/v1/crl` through the port. Its verifier checks the
chain, validity, SAN and revocation before producing a typed principal; authorization defaults to refusal. During migration,
legacy device CN/OU certificates map through the users registry with a deprecation log. Gateway routes, verification and
CLI commands are later slices, not implemented by the adapter. The offline root ceremony belongs to CLI init; import of an
existing CA requires only root/intermediate certificates, the intermediate key and index/serial state, never the root key.

The adapter reads the legacy layout once at import, then operates independently from its state directory. It persists serials,
hashed tokens and revocations in component-owned SQLite state, performs blocking I/O and crypto on Tokio blocking tasks,
and writes a standard CRL signed by the intermediate. The composition root configures the existing compatibility CRL path for atomic public-file publication; private keys never go to that path. Revocation by principal covers every certificate issued to it; imported serials
can be revoked explicitly without inventing a principal from a legacy CN.

## RivetHub operator and terminal names

B1/B12/B16 operator messages use `[rivethub-gateway]`, `RIVETHUB_TLS_CERT` and `RIVETHUB_TLS_KEY`.
ACP subprocess stderr uses the `rivethub::harness_acp::agent` tracing target. Gateway host and origin guidance
refers to the gateway settings; the `den` YAML section remains a B10 compatibility key.
New shell session IDs are `rivethub-<uuid-v4>`. Herdr config homes are `rivethub-<hash8>` under the existing
runtime directory or temporary directory; the hash inputs and socket layout stay unchanged.

Terminal children receive `RIVETHUB_SESSION_KEY`, `RIVETHUB_SESSION`, `RIVETHUB_URL`, `RIVETHUB_TOKEN`
when configured, and `RIVETHUB_CA` and `RIVETHUB_NAME`. Both current and deprecated user identity/token variables
are removed before spawning. B10 deprecated environment aliases remain owned by `config`, including its warning
and current-name precedence. The B12 `denSession` wire key and B1 legacy identity header remain compatibility
boundaries owned by `wire` and tenancy; this slice does not change their protocol shapes.

Recorded expected text is translated only by `testkit::translate_recorded_product_names`, including only the
environment names changed in this slice; other adapters retain their existing boundaries. JSON is parsed into
typed payloads before translating text fields. Actual results are compared without name normalization. Source
recordings remain unchanged. Gateway session replay exclusions are named with reasons in `session_skips`, with
a test accounting for every scenario. Provider, database, capture, mesh schema and config format changes are outside
this naming slice.
