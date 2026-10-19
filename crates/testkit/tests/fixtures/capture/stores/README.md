# Harness transcript store fixtures

These files are the replay inputs for the capture store readers. They are not copies of a recorded harness session. The inventory's store section names paths and, for Hermes, a `messages` query. It does not include sample records for Codex, Grok, Hermes, Kimi, OpenCode, Pi, Qwen, or Cursor. Each file below is a typed example built from that path description so the reader has something to parse. Claude Code is not duplicated here; its reader replays `capture/claude-hook/transcript.jsonl`.

`codex-rollout.jsonl` is one Codex rollout. A user turn comes from `event_msg` / `user_message`. An assistant turn comes from `response_item` / `message`. A `function_call` is paired with `function_call_output` into one tool message on the output line; `arguments` is a JSON string. `session_meta`, `agent_message`, `token_count`, a user `response_item`, `turn_context`, and `compacted` are known non-turns. The filename in a store is `sessions/YYYY/MM/DD/rollout-<stamp>-<uuid>.jsonl`.

`grok-updates-open.jsonl` sits at `sessions/<encoded-cwd>/<uuid>/updates.jsonl`. Its lines follow the shape of real Grok Build `session/update` records with sanitized content: `params._meta` carries `agentTimestampMs` and, for turn events, `promptId`; `params.update.sessionUpdate` names the record. A web search result carries a `rawOutput` with no `type` and is kept as its JSON. The first turn is closed by `turn_completed`. The second turn has no `turn_completed` and the file ends in a partly written line, so the reader holds that turn back. `grok-updates-rest.jsonl` is the remainder of that line plus the closing `turn_completed`; appending it releases the held turn.

`kimi-wire.jsonl` and `kimi-index.jsonl` sit in `sessions/<work-dir>/session_<uuid>/`. The transcript is `agents/main/wire.jsonl` (`user`, `assistant`, `tool_call`, `tool_result`). `session_index.jsonl` records the session id and is not a turn. Live deltas are out of slice.

`pi.jsonl` is `agent/sessions/<encoded-cwd>/<stamp>_<uuid>.jsonl`. A `session` header is not a turn. Turns are `message` and `tool_result`.

`qwen.jsonl` is `projects/<encoded-cwd>/chats/<uuid>.jsonl`. A sibling `<uuid>.runtime.json` is not a transcript. Turns are `user`, `assistant`, and `tool`.

`cursor.jsonl` is `projects/<slug>/agent-transcripts/<uuid>/<uuid>.jsonl`. The record tag is `role` (`user`, `assistant`, `tool`). Live turns are out of slice.

Hermes and OpenCode have no JSON fixture. Tests build a sqlite file with these tables:

```sql
CREATE TABLE messages (
    session_id TEXT NOT NULL,
    role TEXT NOT NULL,
    content TEXT NOT NULL,
    timestamp TEXT NOT NULL
);

CREATE TABLE message (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    role TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE part (
    id TEXT PRIMARY KEY,
    message_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    type TEXT NOT NULL,
    text TEXT,
    tool_name TEXT,
    tool_arguments TEXT,
    tool_result TEXT,
    created_at TEXT NOT NULL
);
```

Hermes reads `state.db`: `rowid`, `session_id`, `role`, `content`, `timestamp` for `session_id` and `rowid` above the cursor, ordered by `timestamp`, then `rowid`. Role text is `user`, `assistant`, `tool`, or `system`. Anything else is an unrecognized speaker. The table has no tool columns, so a tool row is a tool-role message without a tool name.

OpenCode reads `opencode.db`. Parts are left-joined to messages. `type` `text` uses the message role. `type` `tool` is a tool message. Any other part type is an unknown record. A part with no message row is an unrecognized speaker.
