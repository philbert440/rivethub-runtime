# MCP sidecar recording (B7)

Recorded 2026-10-13T16:29Z on a mesh node by `cargo xtask record-mcp` (rivethub-runtime c33cc52) against the TypeScript
sidecar as harnesses launch it (`integrations/<harness>/rivet-memory/bin/rivet-memory-mcp.sh`, stdio, era-negotiating;
server `rivetos-mcp-server` 2.0.0, protocol `2025-11-25` as negotiated by rmcp's client).

```
cargo xtask record-mcp --out <dir> --script xtask/data/mcp-script.json -- <path>/rivet-memory-mcp.sh
```

| file | content |
|---|---|
| `meta.json` | recorder version, program name, time, the server's `initialize` result |
| `tools-list.json` | `tools/list`, sorted by name: 14 tools (the node's environment enables den memory, wiki, delegate, skills and web) |
| `calls.json` | the privacy-safe script: `echo` ×2 (ASCII, unicode), `echo` missing its argument (isError result), an unknown tool (protocol error −32602), `skill_list` |

Scope (CONTRACT B7): the Rust sidecar keeps RivetHub's own tools only — `internet_search` and `web_fetch` appear here
because the node enables web tools; they are dropped by design and are not part of the parity claim.
No memory/wiki/delegation/agent-listing calls were recorded (their results carry private data); their schemas are in
`tools-list.json`.
