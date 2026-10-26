# RivetHub Rust runtime — the boundary contract

Decided 2026-10-12 by Phil. This document is the whole definition of "drop-in replacement": the Rust runtime (RivetHub) must match the
TypeScript runtime (RivetOS) (`origin/main`, commit c72e851f and later) at every boundary listed here, and nowhere else. Internals are
designed fresh in idiomatic Rust (see ARCHITECTURE, the design artifact). Nothing in this document describes how the
TypeScript is built; it describes only what other software, people and files observe.

Reference material that survives from the first attempt (read-only; `/rivet-shared/plans/rust-rewrite/`):
- `inventory/*.md` — what the TypeScript does at each boundary, with file:line evidence (3,535 lines, 8 documents).
- `golden/` — recordings of the TypeScript's real inputs and outputs, produced by the recorders in `tools/`.
- `PLAN.md` TypeScript-bugs table — behaviour the owner decided to fix rather than reproduce.

## 1. Boundaries

| # | Boundary | Who observes it | Spec (inventory) | Pinned by (recording) | Acceptance test |
|---|---|---|---|---|---|
| B1 | Gateway HTTP + WS API (`/api/*`, `/files/*`, `/term`, `/event(s)`, `/sessions`, `/state`, `/layout`, `/healthz`, `/mesh.json`, static SPA, `/ws`, `/v1` OpenAI-compatible; the Topics routes `GET/POST /api/topics`, `GET /api/topics/search?q=`, `GET/PATCH /api/topics/{id}`, `POST /api/topics/{id}/merge`, `GET /api/topics/{id}/sessions` and `GET/PUT/DELETE /api/sessions/{session}/placement`; the tool output route `GET /api/harness-sessions/{session}/tools/{tool}/output`; the hub state route `GET/PUT /api/hub/state` and its `hub.state` notice on `/api/events/ws`) | RivetHub web/Android/Electron, hooks, mesh peers, scripts | gateway-api.md §2–§17 (route table §4, aliases §5) | `golden/gateway/` (28 files, recorded traffic incl. startup refusals) + `golden/term/` (17) | replay every recorded exchange: same status, headers (names case-insensitive), body; WS frames in order |
| B2 | Session event stream v1 (`AgentEvent`, `v: 1`) and the room/session snapshot | RivetHub clients `/ws` clients, `POST /event(s)` producers | gateway-api.md §6, core-domain.md §1.10 | gateway + term recordings (events and snapshots) | accept/reject decisions and emitted snapshots identical on recorded inputs |
| B3 | Capture ingest: hook payloads from every harness → `POST /api/capture` batch body | 10 harness integration kits (claude, codex, grok, kimi, qwen, opencode, hermes, pi, cursor, cowork) | data-layer.md §3.1–§3.6; harness-control-plane.md §3 | `golden/capture/` (22: pg arms), `golden/capture-zod/` (batch validation) | same rows written for the same hook input; same validation errors |
| B4 | Postgres schema and data (`rh_*` tables, migrations 0001–0020 including `0020_tag_removals`, then 0021 renames `ros_*` to `rh_*` with `ros_*` views over them, views, triggers, graphile_worker jobs). Migrations are applied only by `rivethub-migrate postgres`, which then requires every migration file name in `_rivetos_migrations`; the runtime verifies one schema version (`memory_postgres::SCHEMA_VERSION`) at connect — equal ok, newer ok with one warning, no `_sqlx_migrations` table a `NotAdopted` error (one `rivethub-migrate postgres` adopts it, including a database whose history is only `_rivetos_migrations`), behind a `SchemaBehind` error naming the migrate command — and runs no DDL | the live datahub, the TypeScript workers during cutover, operators | data-layer.md §1–§2 | `golden/capture/pg/` (30 scenarios), `golden/memory/` | Rust migrates a 0001-era DB to the same schema; writes rows the TypeScript reads and vice versa; enqueues the same jobs with the same keys |
| B5 | SQLite memory file (`memory.sqlite`, schema v7 and its upgrade ladder through cutover; schema v8 adds a sqlite-vec index afterwards). Creation and upgrades run only through `rivethub-migrate sqlite`, which copies the original file to `<file>.v<N>.bak` before any read-write connection and returns `BackupExists` when that path is already there; the runtime reads `user_version` on a read-only connection (0 → `NotInitialised`, below 7 → `SchemaBehind` naming the file, above 7 → `UnsupportedSchema`), a refused open leaves the file unchanged, and it never creates the file | standalone nodes, TypeScript nodes during cutover | data-layer.md §3; sqlite plugin | `golden/memory-sqlite/` (20 files incl. fixture DBs, upgrades, search results) | same `sqlite_master`, same rows, same search ranks; files open both ways |
| B6 | Memory read API: search / browse / stats / get_full / tags (HTTP and MCP); the per-session summary DAG and the gzip memory export (HTTP) | RivetHub clients, MCP clients (Claude Code etc.), agents | data-layer.md §3.7–§3.11; gateway-api.md §11 | `golden/memory-sqlite/` search cases; gateway recordings | same results and ranking on the same data |
| B7 | MCP sidecar: RivetHub's own tools only, exactly nine (`memory_search`, `memory_browse`, `memory_get_full`, `memory_stats`, `memory_tags`, `wiki_search`, `wiki_read`, `delegate_task`, `list_agents`); `echo`, `skill_list`, `skill_manage`, `internet_search` and `web_fetch` are removed (Phil, 2026-10-13). Their schemas, transports (stdio, streamable HTTP, unix socket) | Claude Code, codex, every MCP client | mcp-tools-providers.md §2–§3 | `golden/mcp/` (`tools-list.json`, `calls.json`) | `tools/list` for the nine equals the recording after the RivetOS→RivetHub name translation, compared as parsed values; recorded calls give the same results |
| B8 | Provider wire traffic: the OpenAI-compatible chat (streamed) and embeddings API as spoken to vLLM, SGLang and hosted OpenAI-compatible endpoints through one client (no Ollama, no llama-server type) | the model servers | mcp-tools-providers.md §5.4–§5.5 | the vllm scenarios in `golden/providers/` (request+response) | byte-equal request bodies (headers filtered) except the intentional wire deltas of one typed table (testkit `B8_OMITTED_REQUEST_FIELDS`): the runtime sends NO sampling fields where the TypeScript sent hard-coded `temperature: 0.7`/`top_p: 0.95`, never sends `tool_choice` (the server's own `auto` default applies when `tools` are present), and sends `max_tokens` only when the provider config sets it (the TypeScript's 4096 default is gone); same parsed responses and stream chunks |
| B9 | CLI: command tree, flags, output text, exit codes, prompts | operators, systemd units, scripts, `rh update --mesh` | cli.md §0–§19 | `tests` listed in cli.md §21; (recording to add: CLI golden transcripts) | same stdout/stderr/exit for the same argv on the same state |
| B10 | `config.yaml` keys, defaults, validation messages; environment variables | operators, deploy tooling | config-and-boot.md §1–§2 | `golden/diff/config/` (realistic cases) | same accept/reject and same messages for the same file |
| B11 | On-disk state: `~/.rivethub/` layout, pid and lock files, `mesh.json`, agent presets, device/pairing files, uploads, layouts, wiki pages (markdown + frontmatter), skills `_meta.json`, `codex-threads.json`. `mesh.json` node metadata stores `denUrl` as a URL and `denPort` as a `u16`. `parse_cutover_gateway_port` is the one cutover reader for a TypeScript node that wrote `denPort` as a decimal string; every other port type is a parse error. Rust writes `denPort` as a number | other nodes, backups, humans, TypeScript during cutover | config-and-boot.md §3.9; core-domain.md §1.11; gateway-api.md §8 | fixtures under `golden/` and the inventories' documented formats | files written by Rust are read by TypeScript and vice versa |
| B12 | Harness protocol: ACP (`session/*`) natively or through the codex-acp and claude-code-acp adapter binaries; herdr control; terminal WS framing | the harness binaries, herdr, RivetHub clients terminal | harness-control-plane.md §1–§2; standardize/SESSION-MODEL.md | `golden/term/` (PTY, WS, roster) ; ACP conformance via the `agent-client-protocol` crate's schema | real binaries: new/resume/prompt/cancel work on every installed harness; term replays pass |
| B13 | Workers: graphile job names, payloads, keys, priorities, cron entries; worker role split | the shared Postgres queue, other nodes' workers | data-layer.md §2.3–§2.6 | `golden/capture/pg/` job rows | same jobs enqueued; same job handled with the same result rows |
| B14 | Mesh registry, re-registration, heartbeat and agent channel; `/ca/v1/sign`, `/ca/v1/chain`, `/ca/v1/crl`; SPIFFE URI SAN identity, holder-generated P-256 keys, 7-day node/agent and 30-day device leaves, one-time enrollment through step-ca, verified-principal renewal by minting a fresh token, step-ca's X.509 CRL; the existing intermediate is bootstrapped into step-ca outside this runtime | peer nodes, holders, operators and legacy CRL readers during cutover | PKI-DESIGN addenda 2026-10-15/15b; mesh inventory unchanged | `crates/testkit/tests/fixtures/stepca/` and `crates/pki/tests` | recorded sign refusals, step-ca chain and CRL through the port, shared `rh ca token` minting, `CaError::Unavailable` when step-ca is unreachable; holder mTLS renew/revoke and local SAN-policy cases listed with reasons |
| B15 | Hook events (names and context fields) seen by skills and the safety fence / audit log | skills, the audit log, operators | core-domain.md §1.6 | existing TypeScript test fixtures | the same hook sequence and fields for the same turn (the workflow journal belongs to the `rivethub-workflows` package, not the runtime) |
| B16 | Logs operators read, health endpoint, `doctor` output | operators, monitoring | config-and-boot.md §3.10–§3.11; cli.md §2.5 | documented lines | same lines for the documented events |

## 2. What is explicitly NOT part of the contract

- Internal module structure, class names, file layout of the TypeScript.
- JavaScript runtime behaviour on inputs RivetHub never produces: `Date.parse` legacy forms, NaN/-0, prototype keys,
  implicit coercions, UTF-16 slicing except where a client receives the bytes.
- Key order of JSON objects, number spelling (`1` vs `1.0`), and whitespace — EXCEPT where a stored hash or dedupe key depends
  on them (today: capture event ids, which hash the `JSON.stringify` of tool arguments; keep that one function).
- Hardening, redaction, validation, timeouts and locking the TypeScript does not have. Weaknesses the TypeScript shares are
  logged in the bugs table and fixed after cutover in both.
- Reviewer opinions about internals. A finding against the Rust is valid only if it names a boundary row above and shows the
  observable difference, or shows a defect the Rust itself introduces (crash, hang, data loss, wrong result), or a departure
  from the Rust standards.

## 3. Owner decisions already made (apply, do not re-litigate)

ACP-shaped session model with chat over ACP (see decision 7 for the final per-harness shape; the earlier app-server /
stream-json / headless drivers are superseded) and terminal as a PTY view of the same
session (SESSION-MODEL.md); one multi-thread runtime; no comments anywhere; no den/room naming; standard crates; literal
file-edit replace; delete custom auto-actions; drop the AI-SDK user-agent; one store-timestamp parser; PGlite migration as a
one-shot `pg_dump` at cutover; `en-US` default locale; regex/YAML/JSON engine diagnostic text may differ after the TypeScript
prefix; lone surrogates become U+FFFD except on wire-exact paths.

Decided 2026-10-12 (the design's six questions): (1) switching chat↔terminal is a turn-end handoff with one owner per
session and queued messages, never injection; (2) herdr is the only terminal multiplexer (tmux support is dropped — B12
narrows to herdr control + PTY); (3) CLI-backed model providers stay, as a thin ModelProvider over a headless harness
session; (4) one process per service (`rh gateway`, `rh agent`, `rh worker --role`, `rh mcp`), with
`rh start/stop/status` keeping their CLI contract by supervising those processes; inter-process communication uses the
existing boundaries (B1 HTTP API, B4/B13 Postgres queue) — no new wire; (5) one reviewer, Claude Opus, for Rust idiom and
quality; boundary acceptance is mechanical; (6) the first attempt's branches and PRs are deleted.

Decided 2026-10-19 (after cutover): memory search ranks recency by logarithmic decay from creation,
`1 / (1 + ln(1 + days / 30))` with the same access reinforcement, instead of the 14-day exponential half-life, so B6
no longer ranks like the TypeScript. The `memory-sqlite/search.json` hybrid recording carries the Rust scores, the
older message the fusion gate now keeps, and the access counts that follow from it.

Decided 2026-10-12 (crate review): (7) one harness adapter — every chat-capable harness speaks ACP, natively or through
`codex-acp` / `claude-code-acp`; pi and cursor are terminal-only until they have ACP; no codex app-server, claude stream-json
or headless drivers are built. (8) A crate exists only where a feature is replaced or deployed on its own: hooks live in
agent-loop, skills in mcp, agent presets in gateway, the wiki builder in workers, the certificate authority commands in cli (signing and policy move to the `pki` adapter under the approved 2026-10-14 design), the
harness store readers in capture (behind the `TranscriptReader` port, wired to the gateway's transcript route by the binary).
(9) Standalone runs on SQLite: no embedded Postgres and no local-db crate; existing PGlite installs migrate once at cutover
through the SQLite store's Postgres-shaped import; `rh local` drops `--pg-port` and `--db` (B9 narrows accordingly);
sqlite-vec arrives as schema v8 after cutover while v7 brute-force search holds parity through the transition.
(10) No built-in tool set: the `shell`, `file_*`, `search_*`, `ask_user`, `todo`, `internet_search` and `web_fetch` tools are
deleted (memory shows no RivetHub use of the first five; every harness ships its own shell, file, search and web tools, so the web
tools do not belong in the sidecar either). B7 narrows to the sidecar's RivetHub-owned tools; the `RIVETHUB_MCP_ENABLE_{SHELL,
FILE,SEARCH}` gates and the `plugins/tools/*` plugin entries are accepted and ignored by `config` with a deprecation warning.
(11) One model client: `providers` is a single OpenAI-compatible implementation
(chat with streamed text/reasoning/tool-call deltas, usage, cancellation;
embeddings) plus the cli-backed provider over a headless ACP session. Provider
types are `openai-compatible`, `vllm`, `claude-cli`, `codex-cli`, `grok-cli`,
`hermes-cli`, `opencode-cli` and `kimi-code`; the removed types `ollama`,
`llama-server`, `openai-compat`, `cli`, `pi-cli`, `qwen-code`, `anthropic`,
`google`, `gemini` and `xai` are a config error at `providers.<id>.type` naming
the alternative (a harness, or `openai-compatible` with the server's base URL;
`qwen-code` remains a harness id). Provider auth is the static `api_key` or,
when `token_command` is set, a bearer minted by that argv: stdout has one
trailing newline removed and is cached until `token_ttl_ms` (default 300000)
elapses, the command is killed when `token_command_timeout_ms` (default 5000)
elapses, and `token_command` wins over `api_key`. A minted bearer that is a JWT
whose payload carries an integer `exp` is instead cached until 30 s before that
`exp`, and not cached when that point has passed. Chat-client embeddings use
that same key or token command. `embed_api_key` remains the memory embedder's
static key, and `embed_token_command`, `embed_token_ttl_ms` and
`embed_token_command_timeout_ms` are removed. `memory.postgres.embed_provider`
and `memory.sqlite.embed_provider` name an `openai-compatible` or `vllm`
provider whose `base_url` and `api_key` or `token_command` the memory embedder
then uses with `embed_model`; setting it together with `embed_endpoint` or
`embed_api_key`, or naming an unknown or non-HTTP provider, is a config error
at that key. Sampling and model-server tuning are configured on the model
server, not in RivetHub: the tuning keys `temperature`, `top_p`, `top_k`,
`min_p`, `presence_penalty`, `frequency_penalty`, `seed`, `stop`,
`repetition_penalty`, `min_tokens`, `default_tool_choice`, `mm_processor_kwargs`,
`chat_template_kwargs`, `extra_body`, `models`, `models_ttl_ms`, `models_url`,
`verify_model_on_init`, `num_ctx`, `keep_alive` and `embed_wire_shape` are
removed, a config that still sets one warns
(`<key> is set on the model server, not in RivetHub`), and the runtime sends
no sampling fields, never sends `tool_choice`, and carries no hard-coded
`max_tokens` default on the wire: `max_tokens` goes out only when the
provider config sets it. A cli-backed provider whose harness fails ACP
discovery (bounded by `discovery_timeout_ms`, default 30000; `timeout_ms`
bounds turns only) is degraded, never fatal: the gateway and every other
provider start, a turn on it fails fast naming the provider and the cause,
timeouts and launch I/O retry with exponential backoff (1 s doubling to
60 s) until discovery succeeds, a missing or unverified harness or a
protocol refusal does not retry, and `/healthz` lists it under
`degradedProviders` (`id`, `recovery`, `reason`), which `rh status`
prints. The agent loop stays, minimal, for Discord, the
task engine's API agents and `/v1`.
(12) Chat-channel bots (Discord, Telegram, voice) live in `rivethub-channels`; `channels.discord/telegram/voice` config is
accepted and ignored with a warning. (13) Workflows kept but outside the runtime: the separate `rivethub-workflows` workspace package
(own binary, depends on `wire` + HTTP client only) drives sessions and tasks over the public API; not linked into `rivethub`.
(14) Clients stay in the monorepo under `apps/`; the gateway serves the RivetHub web build from `static_dir`.
(15) Capture redaction is a configuration option, off by default: `memory.capture.redaction.{enabled,builtins,patterns}` and
`RIVETHUB_CAPTURE_REDACTION` are honoured by the hook-side writer; the YAML keys are actually passed to the hook process (the
TypeScript validated them but did not). Nothing is redacted unless enabled.
(16) Memory search (B6, B7) narrows message hits by `role` and `tool_name` on `GET /api/memory/search`, the
`memory_search` tool route and the MCP `memory_search` schema; summaries carry neither and stay in the result unless
`scope` excludes them (Phil, 2026-10-19).
(17) Topics (Phil, 2026-10-25): a topic is a themed group of sessions with a stable UUID, a unique slug folded from
its name, aliases unique across topics (case-insensitive), an optional parent one level deep and an optional directory
(`local` on a node or `shared`). A session has at most one placement, made by `you` or the `categorizer`; the
categorizer never replaces your placement, and unplacing returns the session to Forming. Merging A into B moves A's
placements, sub-topics and aliases to B and keeps A's slug resolving to B. The B1 Topics routes are new, with no
TypeScript recording; `{id}` takes a topic id or slug and `{session}` is the base64url session segment. Their
bodies are the `wire::gateway::topics` types in `contract/*.schema.json`, and `lastActive` is the newest captured
conversation update of the topic's sessions.
(18) Session categorizer (Phil, 2026-10-26): when a session's first leaf summary is stored, the summarizer skips a
session that is already placed or is a child of another session (it inherits its parent's topic on the client), then
looks for the topics named in the conversation title and the leaf summary (slug, aliases and merge redirects, matched
as whole words of three or more characters, a sub-topic winning over its parent). Exactly one named topic places the
session `by: "categorizer"`; otherwise the session stays in Forming with a suggestion: the title as the suggested
name and, when exactly one topic is named in the title, that topic as `topicId`. No model call is made. `GET
/api/sessions/{session}/placement` carries the suggestion as an optional `suggestion` (`name`, optional `topicId`)
while the session is unplaced; any placement clears it, and merges move its `topicId` like placements.
(19) Tool output (Phil, 2026-10-26): harness transcripts keep tool rows small and carry no tool output except for prompt
tools; a client fetches one call's output on demand from `GET /api/harness-sessions/{session}/tools/{tool}/output`, where
`{session}` is the base64url segment of a canonical session id and `{tool}` is the tool call id. The body is
`wire::gateway::sessions::HarnessToolOutput` in `contract/*.schema.json`: the tool result's text read from the harness's own
session file, capped at 64 KiB by keeping its tail, with `truncated` set when capped. An unknown tool call id is `404`;
Claude Code is the only harness with a tool output source, the others answer `501` `capability-unsupported`. The route is
new, with no TypeScript recording.
(20) Hub state (2026-10-21 call with Phil): RivetHub web / Android / Electron share one document per user — projects,
session names, archive, pins, hidden, chat/terminal modes and the last open session. `GET /api/hub/state` answers
`{version, state}` (version 0 and an empty state before the first save); `PUT` takes `{baseVersion, state}` and saves
only when `baseVersion` is the current version, else answers `409` with the current document. The stored document is
normalised (oversized lists refused with `400`, malformed entries and references to unknown projects dropped), and each
save sends `{type: "hub.state", v: 1, version}` to that user's unfiltered `/api/events/ws` sockets. The document lives on
the memory database (`rh_hub_state`, migration 0026) so every node on one datahub serves the same copy; a node without
Postgres keeps it under `<state>/hub-state/`. Bodies are the `wire::gateway::hub_state` types in
`contract/*.schema.json`. The route matches the TypeScript den route on the unmerged rivetOS `feat/rivethub-hub-state`
branch, with no recording.
Naming: no bare "RivetHub clients" anywhere — the clients are RivetHub web / Android / Electron; the product is RivetHub.

## 4. Gaps to close before building the affected slice

- MCP `tools/list` and sample-call recording (B7); CLI golden transcripts (B9); ACP conformance runs against installed
  binaries (B12). Each is a recorder under `tools/` plus files under `golden/`, recorded from the TypeScript runtime.

Naming (Phil 2026-10-12): the single-node install with an embedded database is "standalone" (not "laptop mode") in every
Rust identifier, log line and document. The CLI command `rh local` keeps its name because it is part of the B9 contract.


B14 implementation policy: the PKI adapter limits token issuance and signing separately to sixty attempts per minute per
CA, counts rejected signing attempts, permits only unambiguous unreserved SPIFFE path components, and refuses a certificate
lifetime extending beyond either CA certificate. Principal revocation also invalidates pending enrollment and future renewal
for that principal. Imported legacy CNs confer no authorization; the gateway's later migration verifier owns registry mapping.
The adapter writes its signed CRL inside CA state and atomically publishes the same public output at the configured compatibility path.
