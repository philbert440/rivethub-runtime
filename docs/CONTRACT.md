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
| B17 | Plug-in contract: the manifest `rivethub-plugin.json` (note 51); the plug-in process routes, its socket and environment (note 52); the plug-in API socket and its token (note 53); the permissions (note 54); the management routes `GET`/`POST /api/plugins`, `GET`/`DELETE /api/plugins/{id}`, `POST /api/plugins/{id}/enable`, `/disable` and `/reload`, `GET /api/plugins/{id}/logs`, `GET`/`PUT /api/plugins/{id}/settings` and `POST /api/plugins/{id}/jobs/{job}/run`, and the proxy `/api/plugins/{id}/{views,actions,canvas,tools,panel,http}/…` (note 55); event delivery and jobs (note 56); plug-in tools in `rh mcp` and the node-local tools socket `<state>/plugins/tools.sock` (note 57); views, canvases and web panels (notes 58–60); the `plugin.changed` and `plugin.views` notices on `/api/events/ws` and `plugin.notification` on `/api/notifications/ws` | plug-in processes and their TypeScript and Rust SDKs, RivetHub desktop / Android / web, `rh mcp` | notes 50–60 | none: new, with no TypeScript recording; bodies are the `wire::plugin` types in `contract/*.schema.json`, and `crates/wire/tests/fixtures/plugin/` holds a workflows and a file-browser plug-in's bodies | every fixture body parses as its `wire::plugin` type and writes back unchanged; the host's route behaviour is pinned by the gateway's tests against a fixture plug-in process |

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
(50) Plug-in host (Phil, 2026-10-29; notes 51–60 are proposed and await his review): RivetHub's core is the workspace
and memory, plus delegation, notifications and tasks; everything else is a plug-in, a program that runs in its own
process on one node, is described by a manifest and reaches rh only over B17. No plug-in code is loaded into rh, so
ARCHITECTURE's "no runtime plugin loading" still holds. The gateway process (`rh gateway`, and `rh local`) is the host:
it keeps the registry, supervises each enabled plug-in's process, proxies clients to it, delivers its events, runs its
jobs and answers its API calls. A plug-in is installed on one node and runs there. `GET /api/plugins` on a node lists
only that node's plug-ins, each carrying the node's name, so a client connected to several nodes shows every node's
plug-ins labelled by node and reaches each through its own node; a mesh-wide plug-in is not part of this contract. Each
plug-in owns `<state>/plugins/<id>/` (B11): `data/` for its own files and databases (kept across reloads and disables;
what is worth remembering goes into memory through `memory.write`), `run/` for its socket and `logs/`. The registry is
`<state>/plugins/registry.json`, mode 0600, holding each plug-in's path, last valid manifest, enabled flag, granted
permissions and install time. The `plugins:` key of `config.yaml` stays accepted and ignored (decision 10): plug-ins are
managed only through B17. Trust: a plug-in runs as the node's user. Its token and permissions bound what it can do
through rh, and on Linux its process is confined with Landlock to what note 54 lists. Every plug-in process is started
by one launch function in the gateway's `plugins` module, from a thread of its own that confines itself first, so the
process inherits the confinement and rh does not. Where the kernel has no Landlock, a plug-in that asks for a `host.*`
permission cannot be enabled (`409` `sandbox_unavailable`, saying so) and any other runs unconfined; each start's `host`
log line says whether the process is confined. Bodies are the `wire::plugin` types in `contract/*.schema.json`. The routes are new, with no
TypeScript recording.
(51) Plug-in manifest (proposed 2026-10-29): a plug-in is a directory holding `rivethub-plugin.json`, a
`wire::plugin::manifest::PluginManifest`. `manifest` is 1. `id` is 2 to 24 characters of `a-z`, `0-9` and single
hyphens, starting with a letter and not ending with a hyphen, unique on the node; it names the plug-in's routes, folder
and tools. `name`, `version` and `description` are text; `icon` (optional) is a Material Symbols name, as is every icon
in B17, and a client draws its generic plug-in glyph for a name it lacks; `$schema` (optional) is ignored, for editors.
`run` is `command` (an argv, never a shell line: a first element without a slash is looked up on `PATH`, one with a
slash is relative to the plug-in directory, which is also the working directory), `env` (extra variables; a `RIVETHUB_`
name is refused) and `readyTimeoutMs` (default 10000). `permissions` lists what it asks for (note 54). `settings` is a
list of fields (note 58) other than `file`, keys unique; rh keeps the values and each client draws them as a native form
on the plug-in's entry in Settings. `tools` are agent tools (note 57): `name` (1 to 24 characters of `a-z`, `0-9` and
single underscores, starting with a letter and not ending with an underscore), `title` (optional), `description`,
`inputSchema` (a JSON Schema object) and `hints` (`readOnly`, `destructive`, `idempotent`, `openWorld`). `contributes`
holds `pages` (`{id, title, icon?, view}`: a sidebar entry opening that view as a page), `topicCards` (`{id, title,
view}`: the view drawn as a card in the topic rail with `context.topicId`) and `sessionActions` (`{id, title, icon?,
action}`: a session-menu entry running that action with `context.sessionId`). `jobs` are `{id, title, schedule?,
timeoutMs?}` and `events` the kinds it subscribes to (note 56). Lists default to empty. View, action, job, tab, form,
canvas and contribution names are 1 to 64 characters of `a-z`, `0-9`, `-` and `_`, starting with a letter or digit.
Unknown fields are refused here and in every other B17 body. Install also refuses an empty `command`, a `file` setting,
duplicate tool, setting, job or contribution ids, an event whose permission is not asked for, and a `schedule` that is
not a five-field cron expression, answering `400` with the field's path.
(52) Plug-in process (proposed 2026-10-29): enabling starts `run.command` in its own process group with a scrubbed
environment: `PATH`, `HOME`, `USER`, `LANG`, `LC_ALL` and `TZ` from rh's own, then `run.env`, then
`RIVETHUB_PLUGIN_ID`, `RIVETHUB_PLUGIN_NODE`, `RIVETHUB_PLUGIN_DATA` (the data folder), `RIVETHUB_PLUGIN_SOCKET`,
`RIVETHUB_API_SOCKET`, `RIVETHUB_PLUGIN_TOKEN` and `TMPDIR`, its `run/` folder. rh reaches the plug-in over HTTP/1.1 on the unix socket
`RIVETHUB_PLUGIN_SOCKET` (`<state>/plugins/<id>/run/plugin.sock`, in a 0700 folder rh creates and empties before each
start), where the plug-in listens; a unix socket rather than a localhost port, because nothing has to be allocated, two
plug-ins cannot collide and another local user cannot connect. The plug-in is ready when `GET /health` answers 2xx
within `readyTimeoutMs`. It serves `GET /health`, `POST /views/{view}`, `POST /actions/{action}`, `POST
/canvas/{canvas}/events`, `POST /tools/{tool}`, `POST /events`, `POST /jobs/{job}`, `GET /panel/{path}` and any method
on `/http/{path}`, only those its manifest needs; an unknown route is its own `404`. Disabling, removing, reloading or
stopping rh sends `SIGTERM` to the process group and `SIGKILL` 5 s later. A process that exits while enabled, or is not
ready in time, goes to `backoff` and is started again after 1 s, doubling to 60 s; ten minutes running resets the delay.
Its stdout and stderr lines are kept, with rh's own lines about it (start, ready, exit status, refused answers) as
stream `host`, in `logs/plugin.log`, rotated at 5 MiB with one previous file kept.
(53) Plug-in API (proposed 2026-10-29): rh mints a token of 32 random bytes (64 hex characters) at every start of a
plug-in's process and keeps only its hash; the token stops working when the process stops. It is accepted only on the
plug-in API socket `RIVETHUB_API_SOCKET` (`<state>/plugins/api.sock`, HTTP/1.1, one per node), never on the gateway's
own listeners, and every request there carries `Authorization: Bearer <token>`: a missing or unknown token is `401`, and
the loopback rule that makes a local caller the owner does not apply on this socket. The token names one plug-in; each
request is checked against that plug-in's granted permissions when it arrives, so disabling or narrowing takes effect at
once, and a route outside them is `403` with code `permission_required` and the permission as `detail`; a route no
permission names (the management routes among them) is `403` with the same code and no `detail`. The socket serves the
B1 routes each permission names (note 54) with their B1 bodies, acting as the node's owner, and three routes of its own:
`GET /api/plugin/self` answers `PluginSelf` (id, node, version, granted permissions, settings values including secrets,
and the data folder); `POST /api/plugin/views/changed` takes `{views}` and publishes `{type: "plugin.views", v: 1,
plugin, views}` on every `/api/events/ws` socket, so a client showing one of those views fetches it again (clients poll
nothing); and `POST /api/plugin/notify` (`notifications.send`) takes `{title, text?, tone?, action?}` and sends `{kind:
"plugin.notification", plugin, node, title, text?, tone?, action?, ts}` on every `/api/notifications/ws` socket, a
client running `action` when the person opens it. Memory a plug-in writes through `POST /api/capture` and the
`memory_append` tool route is attributed to agent `plugin:<id>`: rh rewrites the body's `agent` before the route sees
it.
(54) Plug-in permissions (proposed 2026-10-29): a plug-in asks for permissions in its manifest; a person approves them
at enable (note 55), and any later increase is approved again before the process runs with it. rh checks the B1
permissions on every plug-in API call: `memory.read` (`GET /api/memory/*`, `POST /api/memory/tool/memory_search`,
`memory_browse`, `memory_stats` and `memory_get_full`, `GET /api/wiki*`), `memory.write` (`POST
/api/memory/tool/memory_append`, `POST /api/capture`), `sessions.read` (`GET` on `/api/harness-sessions*`,
`/api/harnesses/*` and `/api/catalog`), `sessions.control` (`POST`, `PATCH` and `DELETE` on `/api/harness-sessions*`:
create, turn, steer, cancel, answer an approval, retitle, delete), `topics.read` (`GET /api/topics*` and `GET
/api/sessions/*`), `topics.write` (the other methods on those), `tasks.read` (`GET /api/tasks*`), `tasks.delegate`
(`POST /api/tasks`), `agents.read` (`GET /api/agents*`) and `notifications.send` (`POST /api/plugin/notify`). Events
need the permission of what they reveal (note 56). On Linux the process is confined (note 50): it reads and writes its
own `data/` and `run/` folders; reads its install directory, the program it runs (and the installation around it when
that program sits in a `bin` folder), `/usr`, `/bin`, `/sbin`, `/lib`, `/lib32`, `/lib64`, `/etc`, `/proc`, `/sys`,
`/dev/null`, `/dev/zero`, `/dev/random` and `/dev/urandom`; writes `/dev/null`; connects to the plug-in API socket; and
opens no TCP connection or listener, reaches no abstract unix socket and signals no process outside its own. The host
permissions widen that: `host.files.read` reads every file and `host.files.write` writes every file (files outside the
plug-in's own folder, still within the node user's rights), and `host.network` allows TCP, abstract unix sockets and
reading the resolver configuration. `host.exec` (running other programs) is shown for consent and recorded; a program
the plug-in runs inherits its confinement, so it is not checked apart. Landlock does not confine UDP, and confines
connecting to pathname unix sockets only from its ABI 9. The tools and contributions a manifest
declares need no permission; clients show them beside the permissions when asking for approval. The approval is the
owner's `enable` call, and rh cannot tell a person from an agent using the owner's credentials, so an agent that builds
a plug-in installs it and a client asks the person to enable it.
(55) Plug-in routes (proposed 2026-10-29): every route but listing and showing answers only the node's owner (`403`
`owner_required` for any other user); every authenticated user of the node can list plug-ins and use their views,
actions, canvases, panels and tools, and rh names the caller to the plug-in in `x-rivethub-user`. `GET /api/plugins`
answers `PluginList` `{node, plugins}`, and `GET /api/plugins/{id}` one `PluginRecord`: `{id, node, path, manifest,
enabled, state, granted, pending, error?, lastExit? {at, code?, signal?}, restarts, startedAt?, installedAt, jobs: [{id,
lastRun?}]}`, `state` one of `disabled`, `awaitingApproval`, `starting`, `running`, `backoff` and `invalid` (the
manifest at `path` is gone or no longer valid; `error` says why, and `manifest` is the last valid one). `POST
/api/plugins` takes `{path}`, an absolute directory on the node, reads and checks its manifest (note 51) and registers
the plug-in in place, disabled, with every permission it asks for `pending`, answering `201` with the record; it runs
and copies nothing, so an agent building a plug-in edits that directory and reloads. An invalid manifest is `400` naming
the field, and an id already installed is `409` (from the same directory the message says to reload it). `POST
/api/plugins/{id}/enable` takes `{approve}` (`{}` when nothing is pending), which must name exactly the `pending`
permissions (`409` `permissions_unapproved` listing them, `400` `not_pending` for one not pending, `409`
`sandbox_unavailable` for a `host.*` permission on a kernel without Landlock, note 50); it grants them, starts the
process and answers the record. While anything is pending, only a caller presenting a paired device
certificate (a device certificate the node maps to its owner) can enable: a node or agent certificate, a bearer token or
a loopback caller is `403` `device_certificate_required`. This one check sits apart from the owner check
(`routes/plugins/approval.rs`) until the approval identity is decided. `POST …/disable` stops it and keeps its grants,
settings and data. `POST …/reload` reads the manifest again: an invalid one is `400` and changes nothing; fewer
permissions narrow the grant at once; more stop the process in `awaitingApproval` with the new ones `pending` until
enable approves them; otherwise an enabled plug-in restarts. `DELETE /api/plugins/{id}` stops and unregisters it,
removes its folder unless `?keepData=true`, never touches the source directory, and answers `204`. `GET …/logs?tail=<n>`
(default 200, at most 2000) answers `{lines: [{at, stream, text}]}`, oldest first, `stream` one of `stdout`, `stderr`
and `host`. `GET …/settings` answers `{values, secretsSet}`: each field's kept value, else the manifest's `value`, a
secret's value left out and its key in `secretsSet`; `PUT` takes `{values}`, where `null` clears a key back to the
manifest's `value`, and refuses with `400` naming the key a key the manifest lacks, a value that does not fit its
field's input (its type, `min` and `max`, a select's options) or a required field left without a value; otherwise it
keeps them in the registry, answers as `GET` does and delivers `settings.changed` to the plug-in when it runs (one that
starts later reads them from `PluginSelf.settings`, which holds the same values with secrets). `POST …/jobs/{job}/run` is note 56. Every change to a record
publishes `{type: "plugin.changed", v: 1, plugin}` on `/api/events/ws`. The proxy forwards
`/api/plugins/{id}/{views,actions,canvas,tools,panel,http}/…` to the plug-in socket with the `/api/plugins/{id}` prefix
removed, adds `x-rivethub-user`, `x-rivethub-node` and `x-rivethub-request-id`, and drops the caller's `Authorization`
and cookies. On `views`, `actions`, `canvas` and `tools` rh parses the request and the answer as their `wire::plugin`
types and sends the client the answer as it parsed it: an answer that does not parse is `502` `plugin_invalid_response`
with the field path as `detail` and a `host` log line; an action's `422` reaches the client as it is, and any other
status but `200` is `502` `plugin_error`. `panel` and `http` pass status, headers (less hop-by-hop headers and
`Set-Cookie`) and body through, streamed. An unknown plug-in is `404`, one not running `503` `plugin_unavailable`
(retryable), and no answer in time `504` `plugin_timeout` (30 s; 120 s for tools; 60 s to the first byte on `http`).
Request bodies are capped at 16 MiB and parsed answers at 4 MiB.
(56) Plug-in events and jobs (proposed 2026-10-29): a plug-in receives the event kinds its manifest lists as `POST
/events` with a `PluginEventDelivery` `{seq, at, missed, event}`, one at a time and in order; `seq` grows by one per
delivery to that plug-in, so a repeat can be dropped. The kinds (`event.type`) are `session.started` (`sessionId`,
`topicId?`, the topic it was started in), `session.idle` (a turn ended and the session waits for input), `session.ended` and
`session.placed` (`sessionId`, `topicId`, `by`), which need `sessions.read`; `summary.written` (`summaryId`,
`sessionId?`, the summarized conversation's session key when it is a canonical session id, `kind`, `level`, its depth),
which needs `memory.read` (every summary write announces itself: in Postgres with `NOTIFY rh_summary_written` in the
write's transaction, which the gateway listens for, so summaries from worker processes reach it; in SQLite, where
`rh local` runs its workers, directly); and `task.done` (`taskId`, `status`), which needs `tasks.read`.
`settings.changed` (`values`) goes to every plug-in whether or not it lists it. A 2xx answer acknowledges a delivery;
anything else, or no answer in 10 s, is tried again after 1 s, doubling to 30 s, five tries in all. Each plug-in has a
queue of at most 1000 events, kept while it is starting or in backoff and dropped when it is disabled; events lost to
overflow or after the last try are counted in the next delivery's `missed`. Delivery is at least once within one gateway
process; events queued when rh stops are lost, so a plug-in that sees `missed` reconciles through the API. A job with a
`schedule` (five-field cron, the node's local time) runs at each tick, and any job runs on `POST
/api/plugins/{id}/jobs/{job}/run` (`202` with the run, `409` while one runs). A run is `POST /jobs/{job}` with `{runId,
trigger, scheduledAt?}` (`trigger` `schedule` or `manual`), answered by `{outcome, message?}` (`succeeded` or `failed`)
within `timeoutMs` (default 600000; else the run failed, "timed out"). A job runs once at a time, so a tick while it
runs is skipped, and ticks missed while rh was down are not made up. Each job's last run is on the plug-in record.
(57) Plug-in tools in MCP (proposed 2026-10-29): `rh mcp` lists, after B7's nine and in manifest order, the tools of
every running plug-in on its node, named `<id>__<tool>` (neither part can hold a double underscore, and the name is at
most 50 characters so harness prefixes stay inside 64), with the manifest's `title`, `description` and `inputSchema`,
and the `hints` as MCP `annotations` (`readOnlyHint`, `destructiveHint`, `idempotentHint`, `openWorldHint`, each `true`
when listed and `false` otherwise). B7's nine tools are unchanged. `rh local` serves MCP in the gateway process and
reads the set from the host directly; a separate `rh mcp` reaches the node's gateway on the local socket
`<state>/plugins/tools.sock` (mode 0600): `GET /tools?after=<version>` answers `PluginToolList` `{version, tools}`,
waiting up to 60 s while the version still equals `after`, and `POST /tools/<id>__<tool>` takes a call. Either way the
server sends `notifications/tools/list_changed` when the version moves. A call carries `{arguments, sessionId?}` (the
calling harness session when rh can tell it: a `tools/call` whose
`_meta` holds `claudecode/toolUseId`, as Claude Code sends, names the session rh is driving whose running tool call has
that id, waited for up to 1 s; a separate `rh mcp` passes that id to the tools socket as `x-rivethub-tool-use`) and goes on to the plug-in as `POST /tools/{tool}`; clients
call the same tool through the proxy as `POST /api/plugins/{id}/tools/{tool}`, answered by `{text, isError}` within 120
s; the MCP result is one text content with that `isError`. A plug-in that is not running, a timeout or a refused answer
is an MCP result with `isError: true` naming the cause. Every harness whose MCP server is the node's `rh mcp` thus gets
a plug-in's tools as it gets the memory tools.
(58) Plug-in views (proposed 2026-10-29): a plug-in's UI is views it answers and each client draws with its own native
widgets and theme, so a plug-in keeps the app's style on desktop, Android and web. A client asks for one with `POST
/api/plugins/{id}/views/{view}` `{params?, context}` (`context` is `{topicId?, sessionId?}`, filled from the
contribution that shows it) and gets a `View`: `title`, `subtitle?`, `toolbar` (buttons beside the title) and `blocks`,
drawn top to bottom. Blocks (`type`): the containers `section` (`title?`, `subtitle?`, `actions`, `blocks`,
`collapsed?`, absent for a section that does not collapse), `split` (`primary` and `secondary` blocks side by side on a
wide window and stacked on a narrow one, `ratio?` the primary share) and `tabs` (`tabs` `[{id, title, badge?, blocks}]`,
switched locally, `selected?`); the content blocks `text` (`text`, `tone?`, `monospace`), `markdown` (CommonMark with
tables, no raw HTML, links opened in the browser), `code` (`text`, `language?`, `follow`: keep the end in view, for
logs), `image` (`path` on the plug-in's `http/` route, `alt`), `properties` (`items` `[{label, value}]`, `value` an
inline), `progress` (`value?` from 0 to 1, absent for indeterminate; `label?`), `notice` (`tone`, `title?`, `text`,
`actions`), `list` (`items` `[{id, title, subtitle?, icon?, trailing, action?, menu}]`, `empty?`), `table` (`columns`
`[{title, align?, sortable}]`, `rows` `[{id, cells, action?, menu}]` with one inline per column, `empty?`; a sortable
column sorts locally by its cells' values) and `breadcrumbs` (`items` `[{label, action?}]`); and the controls `actions`
(`buttons`), `form` (`id`, `fields`, `buttons`), `canvas` (note 59) and `webPanel` (note 60). `empty` is `{title, text?,
button?}`. Inlines (`type`): `text` (`text`, `tone?`, `icon?`, `monospace`), `badge` (`text`, `tone`), `time` (`at` in
epoch milliseconds, `style?` `relative` or `absolute`), `duration` (`ms`) and `bytes` (`value`), each formatted by the
client in its own locale. Tones are `neutral`, `info`, `success`, `warning` and `danger`. A button is `{label, icon?,
style?, action, confirm?, disabled}`, `style` one of `primary`, `secondary` (the default) and `danger`, and `confirm`
`{title, text, confirmLabel?}` asked before the action runs; a menu is a list of buttons. A field is `{key, label,
input, value?, required, help?, placeholder?}`, `input` (`type`) one of `text` (`multiline`), `secret`, `number`
(`min?`, `max?`, `step?`), `toggle`, `select` (`options` `[{value, label}]`, `multiple`) and `file` (`multiple`,
`accept`: media types); a value is a boolean, a number, a string, a list of strings or a list of files `{name, mimeType,
data}` (`data` in base64). Actions (`type`): `call` (`action`, `params?`: `POST /api/plugins/{id}/actions/{action}` with
`{params?, values?, context}`, `values` being the form's when the button belongs to a form), `navigate` (`view`,
`params?`, `replace`: open that view as a page, or in place of the current one), `dialog` (`view`, `params?`: fetch it
and show it modally), `back` (close the dialog, else go back a page), `refresh` (fetch every view of this plug-in on
screen again), `toast` (`text`, `tone?`), `copy` (`text`, to the clipboard), `openSession` (`sessionId`), `openTopic`
(`topicId`), `openUrl` (`url`, `http` or `https`, in the browser) and `download` (`path` on the plug-in's `http/` route,
`filename?`). A call answers `200` with `{actions}`, run in order (a `call` among them is refused as an invalid answer),
or `422` with `{message, fields}`, which the form shows beside the named fields. Pages keep a back stack; a `navigate`
from a topic card or a session action opens the page in the main area.
(59) Plug-in canvas (proposed 2026-10-29): a `canvas` block is `{id, scene, height?, readOnly, menu}`; the client draws
the scene natively, handles pan, zoom, selection and dragging itself, and tells the plug-in only the results. A scene is
`{items, grid?: {size, snap}, viewport?: {x, y, zoom}}` in canvas units (logical pixels at zoom 1, y downwards), its
items drawn in order. An item (`type`) is a `node` (`id`, `x`, `y`, `width`, `height` (its box, used for hits, selection
and connector routing), `shapes`, `anchors`, `tooltip?`, `menu`, `locked`: not draggable, `decorative`: not selectable,
draggable or deletable and has no menu, for lanes and labels) or a `connector` (`id`, `from` and `to` `{node, anchor?}`,
without an anchor meeting the node's box; `route` `straight`, `curved` or `orthogonal`; `stroke?`, `strokeWidth?`,
`dashed`, `arrow` `none`, `end`, `start` or `both`, `label?` at its middle, `tooltip?`, `menu`, `decorative`). Shapes
(`type`) are placed relative to their node's top-left: `rect` (`x`, `y`, `width`, `height`, `radius?`, `fill?`,
`stroke?`, `strokeWidth?`), `ellipse` (the same without `radius`), `text` (`x`, `y`, `text`, `size?` (default 13),
`weight?` `regular`, `medium` or `bold`, `color?`, `align?`, `maxWidth?` past which it ends in an ellipsis,
`monospace`), `path` (`d`, SVG path data; `fill?`, `stroke?`, `strokeWidth?`, `dashed`) and `icon` (`x`, `y`, `name`,
`size?`, `color?`). An anchor is `{id, x, y, side, direction}`: `side` (`top`, `right`, `bottom`, `left`) is where a
connector leaves it and `direction` is `in`, `out` or `both`. Colours are theme roles, never literal values, so a canvas
follows the light and dark themes: `surface`, `surfaceContainer`, `surfaceContainerHigh`, `primary`, `primaryContainer`,
`onPrimary`, `onPrimaryContainer`, `secondaryContainer`, `onSurface`, `onSurfaceVariant`, `outline`, `outlineVariant`,
the tones `info`, `success`, `warning` and `danger`, and `category1` to `category6` to tell kinds apart; an unset fill
is `surfaceContainer`, stroke `outlineVariant`, text `onSurface`, icon `onSurfaceVariant` and connector `outline`.
Locally the client pans (dragging empty space, scrolling), zooms (pinch, or ctrl or cmd with scroll, from 0.1 to 4, with
a fit control), selects (click; shift or cmd click to toggle; shift-drag on empty space for a box; Escape clears), drags
the selected nodes that are not locked, snapping their top-left to the grid when `snap`, with attached connectors
following, draws a connector from an anchor to an anchor of another node (`out` or `both` to `in` or `both`), deletes
the selection on Delete or Backspace, activates an item on double-click or Enter, and opens an item's `menu`, or the
block's `menu` on empty space, on right-click or long press; the selection, hover and provisional connector are drawn in
`primary`. `readOnly` turns off dragging, connecting and deleting. Each result goes to `POST
/api/plugins/{id}/canvas/{canvas}/events` as `{event, params?, context}` (the view's params and context), one at a time
per canvas and in order, consecutive `moved` and `selected` events merged: `selected` (`items`), `moved` (`items` `[{id,
x, y}]`, at the end of a drag), `connected` (`from`, `to`), `deleted` (`items`), `activated` (`item`) and `menu`
(`item?`, absent on empty space, `choice` the menu item's id, and `x`, `y` in canvas units). The plug-in answers `{ops,
actions}`: `ops` (`type`) are `upsert` (`item`, added or replacing the item with its id) and `remove` (`ids`), applied
to the scene before `actions` (note 58) run, so a selection can `navigate` with `replace` to show an inspector beside
the canvas. Until the answer arrives the client shows its own result: moved nodes stay where they were dropped unless an
upsert places them; a new connector is drawn provisionally and dropped when the answer comes, the plug-in's upsert
adding the real one; deleted items are hidden and come back unless the answer removes them. When the view is fetched
again, a canvas with the same id keeps its viewport and selection (less items that are gone); `viewport` is used only
the first time, and without one the client fits the content. A scene holds at most 5000 items.
(60) Plug-in web panel (proposed 2026-10-29): a `webPanel` block (`{id, entry, height?, params?}`) shows the plug-in's
own HTML, `GET /panel/{entry}`, for what blocks and canvases cannot draw. It is specified here and built after the web
panel test build on Wayland. The client loads it in a web view under an opaque origin of its own for each plug-in and
node, serving `/http/…` from the plug-in's `http/` route and every other path from its `panel/` route through the proxy
with the client's own credentials, so the page never holds a token or cookie of the app and reaches nothing but its own
plug-in. Its content security policy allows only that origin (`default-src 'self'`, inline styles, `img-src 'self' data:
blob:`), with no top-level navigation, pop-ups or downloads of its own. The client posts `{type: "init", theme, params?,
context, plugin, node}` once the page has loaded and `{type: "theme", theme}` when the theme changes, `theme` being
`{scheme, colors, fontFamily, fontMono, fontSize, radius}` (`scheme` `light` or `dark`, `colors` a CSS colour per colour
role of note 59), and sets the same theme as CSS variables on the document: `--rh-color-<role>` with the role in kebab
case (`--rh-color-surface-container`), `--rh-font-family`, `--rh-font-mono`, `--rh-font-size`, `--rh-radius` and
`color-scheme`. The page posts `{type: "ready"}`, `{type: "resize", height}` and `{type: "action", action}`, which runs
any action of note 58 as a button would. A panel has no permissions of its own: what it can do is what its plug-in's
routes do.
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
