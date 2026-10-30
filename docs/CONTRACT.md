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
| B1 | Gateway HTTP + WS API (`/api/*`, `/files/*`, `/term`, `/event(s)`, `/sessions`, `/state`, `/layout`, `/healthz`, `/mesh.json`, static SPA, `/ws`, `/v1` OpenAI-compatible; the Topics routes `GET/POST /api/topics`, `GET /api/topics/search?q=`, `GET/PATCH/DELETE /api/topics/{id}` (note 29), `POST /api/topics/{id}/merge`, `GET /api/topics/{id}/sessions`, `GET /api/topics/{id}/page`, `GET/POST /api/topics/{id}/decisions`, `PATCH /api/topics/{id}/decisions/{decision}`, `POST /api/topics/{id}/decisions/{decision}/supersede` (note 27) and `GET/PUT/DELETE /api/sessions/{session}/placement`; `POST /api/harness-sessions` with optional `topicId` and `prompt`, answering with optional `startupContext`, and `GET /api/harness-sessions/{session}/startup-context` (note 24); `POST /term` and `POST /api/terminal` with optional `topicId` (note 25); the tool output route `GET /api/harness-sessions/{session}/tools/{tool}/output`; the hub state route `GET/PUT /api/hub/state` and its `hub.state` notice on `/api/events/ws`) | RivetHub web/Android/Electron, hooks, mesh peers, scripts | gateway-api.md §2–§17 (route table §4, aliases §5) | `golden/gateway/` (28 files, recorded traffic incl. startup refusals) + `golden/term/` (17) | replay every recorded exchange: same status, headers (names case-insensitive), body; WS frames in order |
| B2 | Session event stream v1 (`AgentEvent`, `v: 1`) and the room/session snapshot | RivetHub clients `/ws` clients, `POST /event(s)` producers | gateway-api.md §6, core-domain.md §1.10 | gateway + term recordings (events and snapshots) | accept/reject decisions and emitted snapshots identical on recorded inputs |
| B3 | Capture ingest: hook payloads from every harness → `POST /api/capture` batch body | 10 harness integration kits (claude, codex, grok, kimi, qwen, opencode, hermes, pi, cursor, cowork) | data-layer.md §3.1–§3.6; harness-control-plane.md §3 | `golden/capture/` (22: pg arms), `golden/capture-zod/` (batch validation) | same rows written for the same hook input; same validation errors |
| B4 | Postgres schema and data (`rh_*` tables, migrations 0001–0020 including `0020_tag_removals`, then 0021 renames `ros_*` to `rh_*` with `ros_*` views over them, through 0028 `rh_session_startup` (note 24) and 0029 `rh_session_startup.delivered_via` (note 25), views, triggers, graphile_worker jobs). Migrations are applied only by `rivethub-migrate postgres`, which then requires every migration file name in `_rivetos_migrations`; the runtime verifies one schema version (`memory_postgres::SCHEMA_VERSION`) at connect — equal ok, newer ok with one warning, no `_sqlx_migrations` table a `NotAdopted` error (one `rivethub-migrate postgres` adopts it, including a database whose history is only `_rivetos_migrations`), behind a `SchemaBehind` error naming the migrate command — and runs no DDL | the live datahub, the TypeScript workers during cutover, operators | data-layer.md §1–§2 | `golden/capture/pg/` (30 scenarios), `golden/memory/` | Rust migrates a 0001-era DB to the same schema; writes rows the TypeScript reads and vice versa; enqueues the same jobs with the same keys |
| B5 | SQLite memory file (`memory.sqlite`, schema v7 and its upgrade ladder through cutover; schema v8 adds a sqlite-vec index afterwards). Creation and upgrades run only through `rivethub-migrate sqlite`, which copies the original file to `<file>.v<N>.bak` before any read-write connection and returns `BackupExists` when that path is already there; the runtime reads `user_version` on a read-only connection (0 → `NotInitialised`, below 7 → `SchemaBehind` naming the file, above 7 → `UnsupportedSchema`), a refused open leaves the file unchanged, and it never creates the file | standalone nodes, TypeScript nodes during cutover | data-layer.md §3; sqlite plugin | `golden/memory-sqlite/` (20 files incl. fixture DBs, upgrades, search results) | same `sqlite_master`, same rows, same search ranks; files open both ways |
| B6 | Memory read API: search / browse / stats / get_full / tags (HTTP and MCP); the per-session summary DAG and the gzip memory export (HTTP) | RivetHub clients, MCP clients (Claude Code etc.), agents | data-layer.md §3.7–§3.11; gateway-api.md §11 | `golden/memory-sqlite/` search cases; gateway recordings | same results and ranking on the same data |
| B7 | MCP sidecar: RivetHub's own tools only, exactly nine (`memory_search`, `memory_browse`, `memory_get_full`, `memory_stats`, `memory_tags`, `wiki_search`, `wiki_read`, `delegate_task`, `list_agents`); `echo`, `skill_list`, `skill_manage`, `internet_search` and `web_fetch` are removed (Phil, 2026-10-13). Their schemas, transports (stdio, streamable HTTP, unix socket) | Claude Code, codex, every MCP client | mcp-tools-providers.md §2–§3 | `golden/mcp/` (`tools-list.json`, `calls.json`) | `tools/list` for the nine equals the recording after the RivetOS→RivetHub name translation, compared as parsed values; recorded calls give the same results |
| B8 | Provider wire traffic: the OpenAI-compatible chat (streamed) and embeddings API as spoken to vLLM, SGLang and hosted OpenAI-compatible endpoints through one client (no Ollama, no llama-server type) | the model servers | mcp-tools-providers.md §5.4–§5.5 | the vllm scenarios in `golden/providers/` (request+response) | byte-equal request bodies (headers filtered) except the intentional wire deltas of one typed table (testkit `B8_OMITTED_REQUEST_FIELDS`): the runtime sends NO sampling fields where the TypeScript sent hard-coded `temperature: 0.7`/`top_p: 0.95`, never sends `tool_choice` (the server's own `auto` default applies when `tools` are present), and sends `max_tokens` only when the provider config sets it (the TypeScript's 4096 default is gone); same parsed responses and stream chunks |
| B9 | CLI: command tree, flags, output text, exit codes, prompts | operators, systemd units, scripts, `rh update --mesh` | cli.md §0–§19 | `tests` listed in cli.md §21; (recording to add: CLI golden transcripts) | same stdout/stderr/exit for the same argv on the same state |
| B10 | `config.yaml` keys, defaults, validation messages; environment variables | operators, deploy tooling | config-and-boot.md §1–§2 | `golden/diff/config/` (realistic cases) | same accept/reject and same messages for the same file |
| B11 | On-disk state: `~/.rivethub/` layout, pid and lock files, `mesh.json`, agent presets, device/pairing files, uploads, layouts, wiki pages (markdown + frontmatter), skills `_meta.json`, `codex-threads.json`. `mesh.json` node metadata stores `denUrl` as a URL and `denPort` as a `u16`. `parse_cutover_gateway_port` is the one cutover reader for a TypeScript node that wrote `denPort` as a decimal string; every other port type is a parse error. Rust writes `denPort` as a number | other nodes, backups, humans, TypeScript during cutover | config-and-boot.md §3.9; core-domain.md §1.11; gateway-api.md §8 | fixtures under `golden/` and the inventories' documented formats | files written by Rust are read by TypeScript and vice versa |
| B12 | Harness protocol: ACP (`session/*`) natively or through the codex-acp and claude-code-acp adapter binaries; herdr control; terminal WS framing | the harness binaries, herdr, RivetHub clients terminal | harness-control-plane.md §1–§2; standardize/SESSION-MODEL.md | `golden/term/` (PTY, WS, roster) ; ACP conformance via the `agent-client-protocol` crate's schema | real binaries: new/resume/prompt/cancel work on every installed harness; term replays pass |
| B13 | Workers: graphile job names, payloads, keys, priorities, cron entries; worker role split (the summarizer also claims `sync-topic-page` and runs its hourly `topic-page-sweep`, note 22; `extract-wiki` writes a placed session's summary into its topic page and files people, hosts and tools under category topics, note 23) | the shared Postgres queue, other nodes' workers | data-layer.md §2.3–§2.6 | `golden/capture/pg/` job rows | same jobs enqueued; same job handled with the same result rows |
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
(18) Session categorizer (Phil, 2026-10-26; superseded by (42) on 2026-10-29): when a session's first leaf summary is stored, the summarizer skips a
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
(21) Topic pages (Phil, 2026-10-27): a root topic owns at most one wiki page (`rh_wiki_topics.topic_id`, migration 0027,
bound by exact slug, alias or redirect match; SQLite schema v13 keeps the same binding in `rh_topic_pages` so the legacy
`ros_wiki_topics` shape stays as recorded). `GET /api/topics/{id}/page` returns the topic and its page (a sub-topic
answers with its root's page). `/api/wiki` page bodies gain optional `topicId`. The route is new, with no TypeScript
recording.
(22) Topic page sync (Phil, 2026-10-27): every root topic owns a page; create, rename, reparent and merge keep it in step
via the job `sync-topic-page` (`{topicId?, fold?}`), enqueued in the topic store's own transaction on Postgres (only when
the graphile queue is installed) and SQLite, and claimed by the summarizer role. A new root topic gets an empty page whose
slug is the topic slug, or the slug with `-2`, `-3`... when taken; a rename keeps the page slug, retitles the page and
redirects the new topic slug to it; a merge folds the source page into the target page with the wiki fold and redirects
its slug. Sub-topics are `## <name>` sections of the root's page, recorded in frontmatter `sections: [{topic, heading}]`
(SQLite keeps them in `rh_topic_pages.sections`, schema v14) in sub-topic order; a sub-topic that becomes a root takes its
section's text to its own page, and a root that becomes a sub-topic folds its page into the parent's as its section. A sync
is idempotent. Existing topics get their pages from the summarizer's hourly `topic-page-sweep` (an empty payload syncs
every topic), queued once at start by `rh local`. When a root topic becomes a sub-topic, its page folds into the
parent's section without the `#### Merged from <slug>` line that a merge or consolidation fold still writes.
(23) Wiki builder topic routing (Phil, 2026-10-27): `extract-wiki` looks up the summary's session placement
(`rh_session_placements`; a child session with no placement of its own uses its parent's, by the categorizer's lineage
rule) and, when there is one, gives the model the topic as `topic: {pageSlug, name, section?}` beside the summary. A
patch for that page slug lands on the topic's bound page (resolve reason `topic`, title kept as the topic name); for a
sub-topic it is appended to the sub-topic's section of the root page instead. The alias, identity, entity, stem and
search resolve steps never land a patch on a page bound to another topic: they fall through, and a patch with no other
match creates an unbound page. The bound page always gets a citation row for the summary, even when the model proposes
nothing. Patches carry an optional `kind` (`person`, `host`, `tool`): a kind patch for a known topic (by title, slug
or alias) goes to that topic's page or section; otherwise the builder ensures the root topic `People`, `Hosts` or
`Tools` and a sub-topic named after the patch title under it, and the text lands in that sub-topic's section of the
category page. Patches without a kind, and summaries of unplaced sessions, resolve as before. Existing unbound pages are
not converted. Consolidation never folds a page bound to a topic into another page; a bound page survives its group.
(24) Startup memory (Phil, 2026-10-27): create takes `topicId` (placed by you at create) and `prompt`; rh assembles a
≤4000-token startup context (topic page, recent sessions in the topic, relevant hits) returned as `startupContext` and kept
at `GET …/startup-context`. Delivery into harnesses is note 25. `POST /api/harness-sessions` resolves `topicId` (an
id or slug, following merge redirects) before anything is spawned, so an unknown topic is `404` with no session; the new
session is then placed `by: "you"`, and a failed placement removes it again and answers the error. The context has
`topicId`, `tokens` (the estimate the memory context uses, four characters a token) and `sections` in the order `topic`
(the bound page's summary and article start, and for a sub-topic its own section first; about 1600 tokens), `recent` (up
to four of the topic's sessions by last activity with a latest leaf summary, each with its title; about 1000) and
`relevant` (a hybrid memory search for `prompt`, hits from the topic's sessions first; about 1400, only with a prompt).
Unused budget rolls forward to the next section; an empty part has no section, so a topic with nothing to say answers
`sections: []`. Each section has `kind`, `title`, `text` and `sources` (`{session}` ids or `{wiki}` page slugs; a hit whose session key is not a canonical session id names no session). A
context that cannot be built is logged and the create answers without it. The context is kept per session
(`rh_session_startup`, migration 0028; SQLite schema v15). `{session}` is the base64url session segment; a session
without a kept context is `404`. The bodies are `wire::harness::HarnessSessionCreateRequest` and
`wire::gateway::topics::StartupContext` in `contract/*.schema.json`. The routes are new, with no TypeScript recording.
(25) Startup memory delivery (Phil, 2026-10-27): the startup context reaches the agent as an appended system prompt,
never as a first-turn message. It is rendered as one plain-text block, `## Memory for this session (topic: <name>)`, with
one `### <title>` per section (`Topic page`, then each section's own title); a context with no sections delivers nothing.
An ACP chat sends it on `session/new` in the `_meta` field the harness's adapter reads: claude-code
`_meta.systemPrompt.append` (claude-agent-acp), grok-build `_meta.rules`, codex `_meta.jetbrains.air.customInstructions`
(codex-acp passes it as the thread's developer instructions). opencode, hermes, kimi-code and qwen-code read no such
field and get nothing. A terminal spawn (`POST /term`, `POST /api/terminal`) takes optional `topicId`, resolved like create (an
unknown topic is `404` before anything is spawned), places the session `by: "you"` and answers with
`harnessSessionId`; on a fresh spawn only, never on a resume, the CLI gets claude-code, qwen-code and pi
`--append-system-prompt <text>`, grok-build `--rules <text>`, codex `-c developer_instructions=<TOML string>`; hermes,
opencode, kimi-code and cursor have no append option and get nothing. The kept context gains `deliveredVia`
(`metaAppend`, `cliFlag` or `none`), set when the session starts and served by `GET …/startup-context`; a terminal
resume keeps no context. Kept on `rh_session_startup.delivered_via` (migration 0029, `none` for earlier rows; SQLite
schema v16). The text is never recorded as a user message.
(26) Node version: `/healthz` answers with optional `version` (the rh package version) and `revision` (the short git sha,
present when the build had one), and each `/mesh.json` node carries the `version` and `revision` its own `/healthz`
reported, absent while it is offline. The recorded healthz bodies match them with the `{{VERSION}}` and `{{REVISION}}`
tokens. `rh status` prints the version.
(27) Topic decisions (Phil, 2026-10-27): a topic owns decisions, each with `text`, `status` (`active` or `superseded`),
`by` (the calling user), `createdAt`, `updatedAt`, optional `sessionId` (the session it came from) and, once superseded,
optional `supersededBy`. `GET /api/topics/{id}/decisions` lists the topic's own decisions oldest first; `POST` takes
`{text, sessionId?}` and answers `201` with the decision; `PATCH …/decisions/{decision}` takes `{text}`; `POST
…/decisions/{decision}/supersede` takes `{text?, sessionId?}`, marks the decision superseded and, with `text`, adds its
replacement, answering `{superseded, replacement?}`. Empty text is `400`, editing or superseding a superseded decision is
`409`, and a decision of another topic is `404`. `{id}` follows merge redirects, and a merge moves the source topic's
decisions to the target. Every write queues `sync-topic-page`, which renders the root page's `## Decisions` section (first,
before the sub-topic sections): the root's decisions as `- <text> (<by>, <date>)` lines, a superseded one struck through
and marked `superseded`, then each sub-topic's under `### <name>`. The section is rewritten on every sync, so edits go
through these routes. Kept in `rh_topic_decisions` (migration 0030; SQLite schema v17). Bodies are the
`wire::gateway::topics` decision types in `contract/*.schema.json`. The routes are new, with no TypeScript recording.
(28) Memory search hits (Phil, 2026-10-27): summary hits on `GET /api/memory/search` carry `sessionId`, the session of
the summarised conversation, as message hits already did. The route takes optional `exclude_roles`, a comma-separated
list of roles (`exclude_roles=tool,system`) whose message hits are left out; summaries are unaffected. NUL characters
in the search text and filters are dropped before the query runs, and legacy rows whose `tool_args` is a JSON string
are read as their decoded object when the string holds one, otherwise without arguments.
(29) Topic delete (Phil, 2026-10-28): `DELETE /api/topics/{id}` (an id or slug, following merge redirects) answers `204`
with no body. The topic's placed sessions are unplaced back to Forming, suggestions, kept startup contexts and decisions that name
it are removed, and its aliases and slug redirects go with it. Its bound page stays with its content and becomes a page
without a topic (`rh_wiki_topics.topic_id` set to null; SQLite schema v18 keys `rh_topic_pages` by page slug so the
binding's `topic_id` can be cleared); a deleted sub-topic's section stays on its root's page as a plain section. A topic
with sub-topics, and the category roots `People`, `Hosts` and `Tools`, answer `409`; an unknown topic is `404`. The
route is new, with no TypeScript recording.
(30) Session summary DAG (Phil, 2026-10-28): `GET /api/memory/summaries/{session}` reads the session's one conversation,
the active row first and then the most recently updated, so an ended session (finalized, `active = false`) still answers
its DAG; it was filtered out before. Each node carries `vector` and the body gains `messages`, a tally of that
conversation's messages: `total`, `user`, `assistant`, `system` by role, `tool` (role `tool` or a tool name, as in
stats), and `embedded`, `pending`, `failed`, `skipped` by vector state. A row with an embedding is `embedded`,
`embed_status` `failed` is `failed`, `unembeddable` is `skipped`, anything else is `pending`. An unknown session answers
`200` with no summaries and a zero tally. Topic startup memory reads ended sessions the same way. Bodies are
`wire::gateway::memory::MemorySummaryDagResponse`, `MemoryMessageTally` and `VectorState` in `contract/*.schema.json`.
(31) Harness set (Phil, 2026-10-29): `HarnessId` drops `pi` and `qwen-code` and gains `omp` (oh-my-pi) and `dsh`
(DeepSeek Harness); `RosterCommand` drops `pi` and `qwen` and gains `omp` and `dsh`. omp chats natively over ACP
(`omp acp`) with launch flags `--model` and `--thinking` and models discovered from its ACP model option; the built-in
terminal command `omp` (label `omp`) resumes with `--resume <id>`, has no pin flag, and a fresh spawn gets
`--append-system-prompt <text>`. Its store is the omp agent directory (`PI_CODING_AGENT_DIR`, else `~/.omp/agent`):
`sessions/<encoded-cwd>/<stamp>_<uuid>.jsonl`, listed with the leading `title` line as the title when it is not blank,
else the first user message, and the last `model_change` as the model. dsh chats natively over ACP (`dsh acp`) and is
offered when `dsh` is installed; it has no terminal command and no session store, so its harness session list holds only
the chats live in this gateway. Its transcript has no source (`capability_unsupported`), so the history socket sends no
transcript for a dsh chat and the client keeps what it streamed; a dsh chat starts in the directory of the node's default
terminal command. A `qwen-code` or `pi` entry
under `tasks.harnesses` is now an unknown harness id. Bodies are `wire::harness::HarnessId` and `RosterCommand` in
`contract/*.schema.json`.
(32) Agent plans (Phil, 2026-10-29): `HarnessEvent` gains `plan`,
`{ "type": "plan", "sessionId", "entries": [{ "content", "status" }] }` with `status` one of `pending`, `in_progress`
and `completed`. A harness session's event stream carries it, whole, each time an ACP agent replaces its plan with a
`session/update` `plan`; it is not stored, so a transcript does not replay it. Bodies are `wire::harness::HarnessEvent`,
`PlanEntry` and `PlanStatus` in `contract/*.schema.json`.
(33) Failed turns (Phil, 2026-10-29): `HarnessCode` gains `turn_failed` (HTTP 502, not retryable). When a chat turn
fails, such as a provider refusing the request, the harness session stream carries an `error` event with code
`turn_failed` and the failure as its message, then `turn-complete`, and the session goes idle and takes the next turn;
before, the failure ended the chat. Bodies are `wire::harness::HarnessCode` in `contract/*.schema.json`.
(34) Steering (Phil, 2026-10-29): `POST /api/harness-sessions/{session}/steer` takes `{ "text", "attachments"? }` (the
`/turn` text and uploads, without model, effort or system prompt) and is authorised like `/turn`. While a chat turn is
running it hands the message to that turn instead of queuing a new one and answers `200` with `{ "outcome" }`:
`steered` (the running turn took it) or `startedNewTurn` (the agent began a turn of its own with it); either way the text
is published on the session stream as the user's message, as a turn's is, and its uploads are journalled like a turn's.
With no turn running nothing is sent and it answers `200` with `{ "outcome": "promptRequired" }`: send the message with
`POST …/turn`. A harness that cannot steer answers `501` with code `capability_unsupported`, empty text without
attachments is `400`, and a steer the agent refuses or fails is `503`. Over ACP the gateway sends the agent's
`_session/steering` request (`{ sessionId, prompt, _meta: { steering: { idleBehavior: "promptRequired" } } }`, the
prompt as `session/prompt` content blocks) and reads its `outcome`: `injected` is `steered`, `startedNewTurn` and
`promptRequired` pass through, and omp's `failed` is the `503` with its `reason`. A harness offers steering when its
ACP `initialize` reply carries `_meta.steering.supported: true` (claude-code through claude-agent-acp, and omp):
`HarnessControlCapabilities` gains optional `steering`, `true` for such a harness once discovered, `false` for one
whose agent does not advertise it, and absent where no agent has answered. Bodies are
`wire::gateway::sessions::SessionSteerRequest` and `SessionSteerResponse` and `wire::harness::SteerOutcome` in
`contract/*.schema.json`. The route is new, with no TypeScript recording.
(35) Unstorable text (Phil, 2026-10-29): a memory write that Postgres refuses for its characters (SQLSTATE `22021`
invalid byte sequence, or `22P05` untranslatable character, such as a NUL inside a capture's `tool_args` or
`metadata`) answers `400` with the database's message instead of `500`, so a capture client sets the batch aside as
a bad batch rather than retrying it ahead of every later one. The write still rolls back whole, as the recorded
TypeScript arms expect.
(36) Done state (Phil, 2026-10-29): a topic can be closed and a session marked as history. `TopicRecord` gains
optional `closedAt` (epoch milliseconds, like its other times; absent while the topic is open) wherever it appears:
list, show, search, create and merge replies. `POST /api/topics/{topic}/close` sets the closed time to now (closing a
closed topic moves it to now) and `POST /api/topics/{topic}/reopen` clears it; both answer the `TopicRecord`, take any
topic, category topics included, and answer `404` for an unknown one. Both set the topic's `updatedAt` to now, so a
client that orders topics by their latest change brings a reopened one back to the front. `GET /api/sessions/history` answers
`{ "sessions": [{ "sessionId", "markedAt" }] }`, every marked session, newest mark first.
`PUT /api/sessions/{session}/history` marks the session at now and answers `{ "sessionId", "markedAt" }`;
`DELETE /api/sessions/{session}/history` removes the mark, is idempotent and answers `204`. The session segment is the
canonical id `/api/sessions/{session}/placement` takes, validated the same way. A mark is independent of placement: an
unplaced session can be marked, and placing or unplacing keeps it. rh never clears either by itself: a client shows a
closed topic as open again once one of its sessions is active after `closedAt`, and a marked session as active again
once it is active after `markedAt`. Postgres migration `0031_done_state.sql` adds `rh_topics.closed_at` and
`rh_session_history`; SQLite schema 19 does the same. Bodies are `wire::gateway::topics::SessionHistoryMark` and
`SessionHistoryList` in `contract/*.schema.json`. The routes are new, with no TypeScript recording.
(37) Deeper topics (Phil, 2026-10-29): topics nest up to 4 levels (a root is level 1), replacing the one-level rule of
B1. Setting a parent, at create or update, is refused when the parent's level plus the height of the moved topic's subtree
would pass 4 (`409`, "Topics nest at most 4 levels deep") and when the parent is the topic itself (`400`, "a topic cannot be
its own parent") or anything under it (`409`, "A topic can't move under its own subtopic"). Merging A into B moves A's
subtopics under B, so it is refused when that would pass 4 levels (`409`, the same message) and when B sits below a
subtopic of A (`409`, the same subtree message); merging A into its own direct subtopic still lifts that subtopic into A's
place. A root topic's wiki page keeps a section for every topic under it at any depth, and startup memory, page binding
and routing find a topic's root by walking its parents. Deleting a topic that has subtopics is still refused. The
categorizer drops a named topic when one of its descendants is also named, not only its direct subtopic.
(38) Session previews (Phil, 2026-10-29): `GET /api/harness-sessions/previews?session=<canonical id>&session=…` (at
most 50, else `400`; a non-canonical id is `400` "canonical session required") answers `{ "previews": [ { "sessionId",
"kind", "text", "at" } ] }`, one entry per session with something to show, in no promised order; sessions the caller
can't see, or that have nothing to show, are left out rather than failing the call. `kind` is `running` (the tool in
progress and the gist of its input, as "Bash: cargo clippy --workspace", taken from a later tool call update when the
harness names the command only there, as claude-agent-acp's terminal calls do; or the last line being written),
`asking` (the question an ask tool put to the person), `approval` (the tool waiting on a permission and its gist),
or `said` (the last line of the last assistant reply). `text` is one line of at most 200 characters; `at` is epoch
milliseconds. A session rh is driving shows its live state; any other session shows `said` from its transcript, read
once per update time.
(39) Topic startup preview (Phil, 2026-10-29): `GET /api/topics/{topic}/startup` answers the `StartupContext` a session
started in that topic right now would receive, built by the same code as a real start, without keeping or delivering
it. An unknown topic is `404`; startup memory not configured is `503`.
(40) Topic kinds (Phil, 2026-10-29): `TopicRecord` gains `kind`: `work` (the workspace's topics, and the default),
`type` (an entity type) or `entity` (a person, agent, host, piece of hardware, model, service, software, repo, domain or
database the wiki knows). `GET /api/topics` answers work topics; `?kind=type|entity|all` answers the others, and
`/api/topics/search` takes the same `kind`. `POST /api/topics` takes an optional `kind`, `work` when absent. Work topics nest
only under work topics and types only under types, both within 4 levels; an entity's `parent` is required and must be a
type, and nothing sits under an entity. A wrong parent is `409` ("Work topics nest only under work topics", "Types nest only
under types", "An entity sits only under a type"); an entity without a parent is `400` ("An entity needs a type"). Merging
joins topics of one kind only (`409`, "Only topics of the same kind merge"). Sessions are filed only under work topics:
placing one under a type or an entity is `409`. Slugs and aliases are unique within a kind, so a work topic and an entity can
share a name; a slug in a topic path names a work topic, so types and entities are addressed by id. Any topic can be
deleted, the former People, Hosts and Tools category topics included, and deleting leaves a tombstone so the wiki builder
never recreates that name. The wiki builder writes only to topic and entity pages: a patch about the session's topic lands
on its page as before, a patch whose `type` names one of the types goes to that entity's own page (a type has no page), and
any other patch becomes a note section on the session's topic page. A session that isn't filed yet keeps its notes until it
is filed (a child session until its parent is, a conversation or delegated task until that conversation is), then
they land on its topic page and cite its summary. A session's notes follow it: moved to another topic, by you or
the categorizer, they leave the old topic's page for the new one, and taken out of a topic they wait until it is
filed again; a child follows its parent, and a delegated task its parent unless you placed it; an entity named by one session waits as a candidate until
a second session names it. `WikiPatch.kind` (person, host or tool) is replaced by `type`, and the extraction input lists
the type paths as `types`. Postgres migration `0032_entities.sql` and SQLite schema 20 add the kind, the tombstones and the
candidate and deferred tables; they make People and Hosts types, rename Tools to Software, turn the topics under them into
entities (dropping the filings that pointed at them and unbinding the type pages), and seed the types Agents, Hardware,
Models, Services, Repos, Domains and Databases, with Software's CLIs and tools, Apps, Libraries and frameworks, OS and
platforms, and Plugins and skills.
(41) Wiki edits (Phil, 2026-10-29): `PUT /api/wiki/{slug}` with `{ "currentState"?, "article"? }`
(`wire::gateway::wiki::WikiPageEdit`) replaces what it names on a topic or entity page, commits the page to the wiki
repository as "Edited in Memory" with a `user` source and a dated history entry, and answers the page as
`GET /api/wiki/{slug}` does. A slug that isn't a topic or entity page is `404`; wiki pages not configured is `503`. A page's
title, an entity's name and its type change only through the topic routes, and the builder never renames a topic or an
entity or moves an entity, so what a person sets stays.
(42) Filing (Phil, 2026-10-29): a conversation's first leaf summary enqueues `file-conversation`
(`{conversationId}`), which only the categorizer role runs, on the model of its config's default agent. The tag
suggester (`suggest-tags`) and the folder rule that wrote `project:` tags are retired; tag storage and the tag routes
stay. What is filed: a key that is a session id files that session; a bare uuid on the `claude-code` channel files
`claude-code:<uuid>`; `task:<uuid>` files the conversation under the topic of the conversation whose messages name the
task id (as a conversation when none is found or it is unfiled); any other key files as a conversation. Never filed:
keys starting `grokbot-run-sand-subagent-` or `heartbeat`, and the channels `test-channel`, `env-test-channel` and
`test`. Order: an existing placement is kept; a child session follows its parent on the client and is not filed; a
task follows its parent; a working directory inside a topic's own directory (deepest wins, a tie decides nothing; a
local directory matches by path, since captured conversations do not record their node) files it there; exactly one
topic named in the title or first leaf files it there; otherwise the model answers file, suggest a name (with an
optional parent) or nothing. A suggestion that matches a topic's slug or alias files there; one that repeats the title
or reads like a sentence, path, file, ticket, PR, version, branch or id, or names a deleted topic, is refused; the rest is kept for sessions as the
placement `suggestion` with an optional `parentId` (the parent a new topic would go under; `topicId` keeps its meaning).
Conversations keep no suggestion. Filings are `by: "categorizer"` and never replace yours. `GET
/api/conversations/{id}/placement`, `PUT` (`{topicId}`) and `DELETE` read, set and clear a conversation's placement.
When you move or clear something the categorizer filed, the change is kept as a correction and the newest eight are
shown to the model as examples. `GET /api/topics/{topic}/sessions` answers `sessions` (each with its newest `title` and
`lastActive` across the session's conversation keys, so rows need no harness list) and `conversations`
(`conversationId`, `topicId`, `by`, `at`, `channel`, optional `title` and `lastActive`). Merges move conversation
placements and suggestion parents; deleting a topic drops its conversation placements and the corrections that name it.
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
