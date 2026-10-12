# RivetHub Rust runtime — architecture (text twin of the design artifact, for builders)

Design artifact (visual, authoritative): https://claude.ai/artifact/L7cAvh16cYEZDhzPy4FHCd. Contract: CONTRACT.md. Rules: BUILDER-RULES.md.

## Layers and the dependency rule

| Layer | Crates | May depend on |
|---|---|---|
| foundation | `wire` (serde types for every boundary payload), `config` (config.yaml + env → `Config`) | external crates only |
| kernel | `kernel` (ports = traits, domain types; no I/O), `testkit` (dev-only: replayers, fakes, stubs) | wire, config |
| adapters | `memory-postgres`, `memory-sqlite`, `providers`, `harness-acp`, `terminal`, `mcp`, `capture` | kernel, wire, config, externals — NEVER another adapter or a service |
| services | `gateway` (with the session registry and mesh), `agent-loop`, `workers`, `tasks` (sessions over the gateway API) | kernel, wire, config — NEVER an adapter |
| composition | `cli`, `rivethub` (binary; one composition root per service process) | everything |

The gate (`tools/gate.py`) fails any Cargo.toml that breaks this.

## Ports (kernel traits) and who implements them

| Port | Methods (shape) | Adapters | Callers |
|---|---|---|---|
| `MemoryStore` | append(batch) · search(query) · browse(filter) · stats · get_full(id) · tags(action) · context(session) | memory-postgres, memory-sqlite | gateway, agent-loop, workers, mcp |
| `JobQueue` | enqueue(job: name, payload, key, key_mode, priority, run_at) · claim(role) · complete/fail | memory-postgres (graphile tables), memory-sqlite | memory adapters, workers, tasks |
| `ModelProvider` | chat(request) → response · stream(request) → chunk stream (text, reasoning, tool-call deltas, usage) · embed(texts) · list_models | providers: ONE OpenAI-compatible client (vLLM, SGLang, llama-server, Ollama, hosted OpenAI-compatible APIs) + cli-backed over a headless ACP session; no native Anthropic/Gemini/xAI clients | agent-loop, workers, memory search (query embedding) |
| `HarnessAdapter` | new_session(spec) · resume(id) · prompt(id, input) · cancel(id) · answer_permission(id, decision) · set_option(id, model/effort) · events(id) → SessionEvent stream | harness-acp (one ACP client + a per-harness launch table: native ACP, or codex-acp / claude-code-acp) | gateway's session registry |
| `TerminalMux` | spawn(session, argv, cwd, size) · attach(id) → byte stream · input(id, bytes) · resize · kill · list | terminal (herdr over a PTY; the only multiplexer) | gateway |
| `Tool` | definition() → `rmcp::Tool` · call(args, ctx) → `rmcp::CallToolResult` (the MCP tool shape, no translation) | RivetHub's own capabilities (memory, wiki, delegation, skills) in mcp; remote MCP servers' tools via the mcp client | agent-loop, mcp server |
| `TranscriptReader` | sessions(store) · read(session) → turns · tail(session, since) | capture's store readers (one per harness) | capture (incremental + backfill), gateway (transcript route) via the binary's wiring |

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
capture 4,500 · tasks 2,000 · cli 4,900 · rivethub 500. Total ≈ 50,000 against ≈ 120,000 lines of
TypeScript. Not separate crates: hooks (agent-loop), skills (mcp), agent presets (gateway), wiki builder (workers),
certificate authority (cli), harness store readers (capture). No local-db: standalone runs on SQLite (sqlite-vec as schema v8
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
Mesh (registry, heartbeat, agent channel) is a gateway module; ssh helpers are in cli; mesh.json types in wire. 17 crates.
No runtime plugin loading: extension points are adapter crates behind ports, the public API, and MCP. `plugins:` and
`channels.*` are accepted-and-ignored compatibility keys.
