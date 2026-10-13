# Claude capture pg arm goldens (rr-2c)

These files record the TypeScript direct-Postgres capture path, `ingestTranscript` and `ingestHookEvent` from
`plugins/providers/claude-cli/src/transcript-capture.ts`, called on the `pg` transport against a scratch database. They
are the byte-level reference for the Rust slice 2c port. All content is synthetic.

## Re-running

```
url=$(/home/rivet/work/.tools/rr/scratch-db.sh rr2c_capture_pg_golden ts)
node /rivet-shared/plans/rust-rewrite/tools/capture-pg-goldens.mjs /home/rivet/work/w-rust "$url" \
  /rivet-shared/plans/rust-rewrite/golden/capture/pg
/home/rivet/pg16/bin/psql postgres://rivet@127.0.0.1:5441/postgres -c 'DROP DATABASE rr2c_capture_pg_golden WITH (FORCE)'
```

The tool imports the compiled `plugins/providers/claude-cli/dist/transcript-capture.js` (build the clone first if
`src` has changed), resolves `pg` from that package, and strips every `RIVETOS_*`, `RIVET_*` and `HERDR_*` variable
from its own environment before the import, so `CAPTURE_AGENT` is `rivet-claude`. Two runs give identical output.

## Schemas

Each scenario names the schema it ran in:

- `public` has the full TypeScript migrations (0001-0020, so `task_id` and `owner_user_id` both exist) plus
  graphile-worker. Inserts fire `trg_embed_message`, and the resulting `embed-target` jobs are dumped.
- `pre0011` is the `SCRATCH_DDL` from `task-association.test.ts`, with no `task_id` and no `owner_user_id`.
- `m0011` is `SCRATCH_DDL` plus the real `0011_conversation_task_id.sql`. It has `task_id` but no 0013.
- `half0013` is `m0011` plus `owner_user_id` on `ros_conversations` only, so the owner probe answers `n = 1`.

Scratch schemas are reached with `?options=-c search_path=<schema>` on the URL, as the TypeScript test does. Tables are
truncated (and graphile jobs deleted) before every scenario.

## File layout

There is one file per group: `transcript.json`, `task.json`, `routed.json`, `hook.json` and `errors.json`. Each file
is `{group, generator, captureAgent, scenarios[]}`, and each scenario is `{name, schema, schemaDescription, note?,
steps[]}`. A step contains:

- `kind`: `ingestTranscript`, `ingestHookEvent` or `sql` (a setup statement run by the recorder).
- `input`: for a transcript step, `transcriptPath`, `transcriptLines` (the file is `lines.join('\n') + '\n'`, and `null`
  means the file was never written) and `options`. For a hook step, the `HookEventOptions` as passed. `pgUrl` and
  `env.RIVETOS_PG_URL` are shown as `<pg-url>`. A step with `env` passed `{...env, RIVETOS_CAPTURE_TRANSPORT: 'pg',
  RIVETOS_PG_URL}` plus the same `pgUrl`.
- `result` or `error` (`{name, message, code}`): what the function returned or threw.
- `sql`: every statement the capture client issued, in order, with node-pg parameter values. It includes the two `SET`
  guards, `BEGIN`, the lock, the probes, inserts, updates and `COMMIT`/`ROLLBACK`.
- `rows`: the state after the step. `ros_conversations` is `SELECT *` ordered by `session_key, agent`.
  `ros_messages` is `SELECT *` in insertion (`ctid`) order, without `content_tsv` and `embedding`. `public` also
  includes `graphile_jobs` and `ros_summaries_count`. jsonb, json and timestamptz columns are Postgres's own text output
  (session `TimeZone=UTC`), so jsonb shows Postgres key order and spacing. A column that does not exist in the schema is
  absent from the row.

## Normalization (apply the same to Rust output before comparing)

Normalization runs per scenario, walking the scenario JSON in key order and rewriting every string:

1. The fixture directory becomes `__FIXTURES__`. The same path without its leading slash becomes `__FIXTURES_REL__`,
   which shows up in the path-derived session key.
2. A UUID that appears in no step input (session ids and task ids are kept, compared case-insensitively) becomes
   `conv-N` if it is a `ros_conversations.id` in some dump, `msg-N` if it is a `ros_messages.id`, and `id-N` otherwise
   (a rolled-back conversation id seen only in `sql`). Each prefix is numbered in first-seen order.
3. A timestamp no earlier than 5 s before the run started (a `now()` from Postgres or a `toISOString()` from JS, such as
   `last_ingest_at`) becomes `<now-N>`, numbered in first-seen order and keyed by the exact text. Equal placeholders
   therefore mean the same transaction's `now()`. Fixture timestamps (2026-01-01, and 2026-10-10T12:00 in the slice-1
   fixture) are kept literally.

Long contents are stored whole.

## Scenarios

- **transcript.json**:
  - `ingest-twice` uses the slice-1 fixture `../claude-hook/transcript.jsonl`.
  - `grows-by-one-turn`.
  - `identical-tool-calls-multiset`.
  - `jsonb-key-reordering`: a hook-stored tool row is deduped against a transcript call with a different key order,
    integer-like keys, a non-ASCII key and a big number.
  - `mark-inactive`.
  - `ai-title-upgrade`: the fallback title is sliced inside a surrogate pair, and later the AI title wins unsliced.
  - `ai-title-at-create`.
  - `misc-shapes`: no timestamps, transcript-only session id, `pr-link`, sidechain, thinking-only, `isMeta`, herdr, a
    truncated prompt, a no-uuid assistant and a bad line.
  - `path-derived-key`.
  - `hook-rows-then-transcript`.
  - `early-returns`.
- **task.json**: task id present, backfill without repoint, uppercase UUID, non-UUID, legacy `task:<id>` override,
  and the absent column (`pre0011`).
- **routed.json**: routed with 0013; adoption of an unowned conversation (trimmed id, no repoint, whitespace-only id);
  without 0013 (`m0011`); and the half migration (`half0013`).
- **hook.json**: UserPromptSubmit without a key and with a repeated key, including the same key in another session;
  PostToolUse without a key (object, null and long responses, missing name and input) and with a repeated key; a
  long prompt; and the pre-transport skips.
- **errors.json**: a `\u0000` in `tool_args` (jsonb 22P05) mid-transaction, on a new conversation (no rows remain)
  and on an existing one (rows and settings unchanged); the same for a hook event.
