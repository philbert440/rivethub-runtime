# memory-sqlite goldens (slice rr-2s)

Recorded by `tools/sqlite-memory-goldens.mjs` from the TypeScript SQLite memory backend
(`plugins/memory/sqlite/dist`, schema v7).

```
node tools/sqlite-memory-goldens.mjs <rivetOS checkout with built dist> golden/memory-sqlite
```

## Runtime

- Node v22.22.2, `sqlite_version()` 3.51.2 (node:sqlite). rusqlite 0.37 `bundled` is SQLite 3.50.2: bm25,
  FTS5 tokenization and LIKE run inside SQLite on both sides. Record the Rust-side `sqlite_version()` next to this one.
- Checkout HEAD c72e851fcce336be1bac4d7357323c4f751f7f06; git version 2.43.0 for the wiki writer.

## Determinism

- **Clock.** `Date` is replaced globally: `new Date()` and `Date.now()` return a scripted instant that only moves when a
  scenario calls `advance`. Every scenario starts at 2026-10-10T12:00:00.000Z unless its first step's `at` says otherwise. Each step
  records `at`, the clock when it ran. No `now` option is passed to `SqliteMemory`; every clock read in the TypeScript
  (row stamps, the job queue, `backend.write`, health, hybrid decay, the embed query cache) sees the scripted clock.
- **Ids.** `crypto.randomUUID` returns `00000000-0000-4000-8000-a<11-digit counter>`; the counter restarts at 1 for
  every scenario. In every JSON file except `schema.json`, `upgrade.json` and `fixture-v7.json`, those ids are then
  renumbered in first-seen order of the serialized file to `00000000-0000-4000-8000-<12 digits>` (000000000001, …).
  A Rust test renumbers its own uuids the same way (first-seen order in its serialized output) before comparing.
  Ids that appear in an ORDER BY tie-break (`ORDER BY created_at, id`) and git commit messages (so git shas) depend on
  the raw sequence: to match those exactly, inject the same counter generator. Fixed literal ids in inputs use
  `00000000-0000-4000-8000-1…` (upgrade files), `-2…` (Postgres-shaped dump), `-3…` (task ids), `-4…/-5…/-6…` (raw SQL rows).
- **Temp paths.** The temp root is replaced by `/home/user/.tmp/sqlite-goldens`.
- **Values.** `$blob_hex` is a BLOB as lowercase hex; `$map` is a JavaScript `Map` as entries; an `error` is
  `{ class, message, code? }` of the thrown error. Floats are `JSON.stringify` text: compare ranks and scores to 1e-12
  relative, order exactly.
- **Logs.** Each scenario's `logs` holds every `log` line given to the store, plus `console.warn` lines, in order.
- **Env.** `RIVETOS_*`, `RIVET_*`, `COMPACT_*`, `WIKI_*`, `EMBED_*`, `GIT_*` and `SESSION_TAGGING` are cleared;
  git runs with `GIT_CONFIG_GLOBAL=/dev/null`, `GIT_CONFIG_NOSYSTEM=1` and author/committer date 2026-10-10T12:00:00Z.

## Store configuration

Unless a scenario says otherwise: `new SqliteMemory({ path: ':memory:', workers: false, log, projectRule: null })`.
Jobs only run through `runJobs()` (one `tick`: due sweeps, then up to 10 jobs); "drain" is `runJobs()` repeated until
it returns 0 (at most 12 times). Embed and LLM configs always pass `sleep: async () => {}`; embed passes `maxRetries: 0`.

## Fakes

**Embedder** (`https://embed.example.com`, model `fake-embed-8`, OpenAI wire shape). For each input text: lowercase it, take
every `[a-z0-9]+` token, and add 1 to dimension `fnv1a32(utf8(token)) % 8` of an 8-wide zero vector (FNV-1a 32-bit:
offset 0x811c9dc5, prime 0x01000193). Integers are returned as JSON numbers. A text with no token gives the zero vector.
A request answers HTTP 500 when any input contains `embedfail`, and HTTP `status` for every request while the
scenario has marked the endpoint down. Requests are recorded (`embed_requests`, `query_embed_requests`) so the
query-instruction text and chunking are checkable.

**Chat endpoint** (`https://llm.example.com/v1`, model `fake-chat` unless stated). It answers by the request's system prompt,
identified by exact equality with the memory-core constant (`prompt` in recorded calls; sha256 of each in
`compaction.json` `prompts_sha256`): LEAF → the leaf text below, BRANCH → `Branch summary: several deploy sessions for AcmeKit settled on blue-green releases with a health-check gate.`, ROOT → `Root summary: the whole AcmeKit deploy conversation, from the first release plan to the documented rollback.`,
WIKI_EXTRACT → `JSON.stringify(PATCH)`, WIKI_RECOMPILE → `JSON.stringify(RECOMPILE)`, TAG → `JSON.stringify(TAG_ANSWER)`.
Answers are `{ choices: [{ finish_reason: 'stop', message: { content } }] }`; a scenario may override one call
(a status, `finish_reason: 'length'`, other text) and says so in its name. Constants:

```json
{
  "LEAF": "The team decided to deploy the AcmeKit service with a blue-green rollout. The release branch is cut on Mondays, the deploy script runs from the release branch, and a health check on staging.example.com gates the traffic switch. Rollback is a traffic switch back to the previous colour.",
  "PATCH": [
    {
      "action": "create",
      "slug": "acmekit-deploys",
      "title": "AcmeKit deploys",
      "aliases": [
        "acmekit-rollout"
      ],
      "tags": [
        "deploy"
      ],
      "entities": [
        "project:acmekit",
        "tool:deploy_script"
      ],
      "summary": "AcmeKit is deployed blue-green from the release branch; a health check gates the switch.",
      "article": "## Process\nCut the release branch on Monday, run the deploy script, watch the health check.",
      "history_entry": {
        "date": "2026-10-10",
        "title": "Blue-green adopted",
        "body": "Decided to deploy blue-green."
      }
    }
  ],
  "RECOMPILE": {
    "summary": "AcmeKit deploys are blue-green; rollback is a traffic switch.",
    "article": "## Process\nCut the branch, deploy, watch the health check.",
    "history_entry": {
      "date": "2026-10-10",
      "title": "Recompiled",
      "body": "Rewritten from history."
    }
  },
  "TAG_ANSWER": [
    {
      "key": "project",
      "value": "AcmeKit",
      "confidence": 0.9,
      "reason": "deploy scripts for it"
    },
    {
      "key": "topic",
      "value": "blue-green deploys",
      "confidence": 0.7
    }
  ]
}
```

**Project rule.** Captures use `projectRule: null` except the "project rule tag at capture" scenario and the fixture,
which inject a resolver (`/home/user/work/AcmeKit` → `project:acmekit`, display `AcmeKit`, rule `git-root`).
`importSqliteMemory` replays the rule with the default resolver, which consults the real filesystem: the cwd values in
`portability.json` (`/home/user/work/AcmeKit`) do not exist on the recording host, so a Rust test must give its
`ProjectFs` no such directory.

**Users.** The owner is `alice`; the other registry user is `user-b`. The brief's second example user id is flagged
by the repo privacy scanner in these contexts, so it is not used.

## Files

| File | What |
|---|---|
| `schema.json` | Fresh v7 file: pragmas, full `sqlite_master` (type, name, tbl_name, sql, rowid order), `PRAGMA table_info` of every table, the SCHEMA text. Populated file (`fixture-v7.sqlite`): row counts and `typeof()` per column. |
| `fixture-v7.sqlite` | Small synthetic v7 file written by the TypeScript (capture, append, embeddings, leaf/branch/root summaries, tags, vocabulary, a wiki topic, one queued job). Rust opens a copy and searches it. |
| `fixture-v7.json` | How the fixture was built, and expected `search()` results for it: FTS-only and hybrid, raw bm25 ranks, scores, `degraded`, access counts after. Raw ids (as stored in the file). |
| `fixture-v7.sql` | Text dump of the fixture (for review and scanning). |
| `upgrade-v1.sqlite`, `upgrade-v2.sqlite`, `upgrade-v5.sqlite` | Inputs: the DDL of that schema version from git plus synthetic rows, `user_version` stamped. |
| `upgrade.json` | Per input: the DDL, a SQL dump, the before snapshot, and the file after the TypeScript opened it (sqlite_master, table_info, every row, FTS rows, typeof), plus a search on it. Raw ids. |
| `capture.json` | `backend.capture` edges from backend.test.ts and more (dedup, cowork rewrite, pending tool fill, caps, timestamps, finalize, request errors, owner stamp, project rule), and `memory.append` / history / settings / other-user refusals. |
| `search.json` | Helper outputs; FTS-only `memory.search` (with raw ranks) and `backend.search` (tags, tag filter, errors); tag pool widening; hybrid search (literal arm, trigram fallback, degraded query embedding). |
| `context.json` | `getContextForTurn` text: Recent, Wiki, Relevant, dedup, budget, heartbeat exclusion, wiki failure log. |
| `jobs.json` | Queue rows across enqueue/claim/fail/dead/revive/requeue/recover; runner ticks, sweeps and log lines; embed-target failures to dead and the sweep revival; unembeddable, zero vector, model and width changes. |
| `compaction.json` | Leaf, leaf/branch/root tree, truncation shrink, LLM failure, dead-job rest, idle/stale buckets. Chat calls include the full user prompt. |
| `tags.json` | Tag store lifecycle, vocabulary edits and merges, tagger suggestions (chat and native shapes), removal suggestions and the protected rule. |
| `wiki.json` | Index operations, extraction on the job loop (page files, git log), page update by alias, failure and backfill retry, consolidate and recompile. |
| `tools.json` | Tool definitions (write and read sets) and every tool's text output, including `Error:` answers and thrown errors. |
| `stats-health.json` | `stats(agent?)` and `health()` JSON across states. |
| `export.ndjson` | Gunzipped export stream of a seeded store (header + rows). |
| `portability.json` | `--since` exports, round-trip import, merge, a Postgres-shaped dump import, refusals. |
