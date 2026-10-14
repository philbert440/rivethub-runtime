# Service root recordings

`rh gateway`, `rh worker`, and `rh mcp` are separate composition roots in the `rh` binary.
`rh start` still launches the gateway. Supervising those processes is a later slice.

The MCP catalog recording is `mcp/tools-list.json` (14 tools, protocol notes in `mcp/README.md`).
The binary test starts `rh mcp` on stdio, calls tools/list, and compares the tool names with that
recording. The worker proof is a live `reap-dead-jobs` job against lane Postgres, not a transcript:
the job is enqueued before `rh worker --role summarizer` starts, and the row is gone after the
runner claims and completes it. sqlx creates that database from template1, which has no
graphile_worker schema, so the proof first applies `crates/rivethub/tests/fixtures/graphile_worker.txt`,
a schema-only snapshot of the lane database. The worker is not pointed at the live lane database.
Each live root is then stopped with SIGTERM and must exit 0.
Gateway coverage starts `rh gateway` with an initialized certificate authority and checks
`/api/phone-pairing` plus the file-backed agent presets. `--check` never connects or binds.

## Omitted

- `delegate_task` — B7: no kernel delegation port
- `list_agents` — B7: no kernel agent registry port
- `internet_search` — B7: harness owns web tools; removed by design
- `web_fetch` — B7: harness owns web tools; removed by design
- `echo` — Product decision: not a sidecar tool
- `skill_list` — Product decision: skills are not sidecar tools
- `skill_manage` — Product decision: skills are not sidecar tools
- `categorizer` — categorizer has no handler in this build
- `wiki-builder` — wiki-builder has no handler in this build
