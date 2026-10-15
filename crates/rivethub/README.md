# Service composition

`rh gateway`, `rh agent`, `rh worker`, and `rh mcp` are the composition roots. Each one
runs on the multithreaded Tokio runtime built in `rh`'s `main`, with one
cancellation tree and a task tracker, and stops on SIGTERM or SIGINT.
`rh start` remains the gateway launcher. Supervising the roots as separate
processes is a later slice. Invoking `rh` without arguments prints help.

`rh gateway --check`, `rh agent --check`, `rh worker --role <role> --check`, and `rh mcp --check`
validate configuration without connecting the memory database, binding
listeners, or moving legacy state. Configuration uses `~/.rivethub/config.yaml`
and the config crate's `RIVETHUB_*` environment resolution and deprecated aliases.

The gateway composition supplies Postgres memory, the shared session registry,
local owner identity, verified device names, the Claude transcript reader,
configured OpenAI-compatible providers and agent loops, the file-backed agent
presets, the device runtime with the node's certificate authority, and optional
terminal and mesh services. HTTP routes use the gateway router, including capture,
memory/tags, sessions, terminal/harness sessions, wiki and v1. Every request to
the composed router requires a resolved identity. A verified device leaf is
looked up before loopback ownership, so an unknown device cannot inherit the
owner identity merely by connecting over loopback.

Under the compatibility-pinned `den` configuration section:

- `tls_cert` and `tls_key` select the server identity.
- `tls_ca` selects the client CA chain; when present, client certificates are
  required and verified by rustls.
- `tls_crl` selects a PEM CRL bundle. Revocation and CRL expiration fail closed.
- `users_file` (overridden by `RIVETHUB_USERS_FILE`, with the deprecated
  `RIVETOS_USERS_FILE` alias) selects the B11 JSON registry: `ownerUserId`, `unmappedIsOwner`,
  and `users`, a map of user IDs to records with optional `devices`, `pgUrl`,
  `envFile` and `persona`. Device names accept the compatibility `device:` prefix.
  SPIFFE device identities must match both the registry user and device. Legacy
  device leaves use the registry mapping and emit a deprecation warning. An
  explicit missing or invalid file refuses startup. Without a registry, the local
  identity is `owner` and no device identities are accepted.
- `terminal.enabled: true` enables the herdr-backed terminal service. It requires
  `commands_file` and a filesystem root (`files_root` or `runtime.workspace`).
  The command file contains `default` and `commands`; each command has `id`,
  `label`, `argv`, `cwd`, optional `harness`, and `session`. Direct PTY fallback
  is not selected by the composition. Missing herdr refuses terminal startup.

Mesh requires `enabled`, `node_name`, `advertise_host`, `agent_channel_port`,
and TLS with a client CA. It writes `mesh.json` beneath the configured shared
root. Every configured agent receives a bounded mailbox and independent turns.
The default agent provides `/v1`; all agents use the common audit pipeline.
Registry heartbeats, listeners, turns and terminal maintenance belong to the
shutdown tree. SIGTERM/SIGINT cancel and join these services.

Before moving the compatibility state directory, startup acquires existing
root-level `.pid`/`.lock` files and checks their PID owners. A live PID, held lock,
unreadable file or malformed PID refuses the move. Successful stale-file checks
retain the locks through rename. `--check` never performs the move.

## Current checkout limits

The SQLite crate does not implement the complete kernel `MemoryStore` port.
Explicit SQLite/PGlite selections therefore fail with a typed startup error;
they are never redirected to Postgres.

`rh gateway` opens the certificate authority at the state directory's `ca`
path, the same directory `rh ca` uses, after the memory database connects.
The authority must already be initialized. Devices use that authority. A phone
issuer is enabled only when `den.devices.gateway_url` is set. Enrollment stays
off: the configuration has no embed URL, WireGuard address, or device Postgres
URL. CRL publication stays on `rh ca --crl-out`. Agent presets use the file
store the gateway already opens. The users file remains a composition
configuration boundary.

`rh worker --role` runs the workers runner for roles that have handlers
(embedder, summarizer, and `all`, which keeps those two). Categorizer and
wiki-builder fail closed. The runner uses the Postgres job queue, embedding
store, and summary store, plus the default agent's OpenAI-compatible provider.
An absent queue fails closed.

`rh agent` runs the task engine: it claims `run-task` jobs, executes each
session through the gateway HTTP API, and finishes under the process shutdown
token. `rh agent --check` validates the node, worker, pricing, gateway URL, and
Postgres URL without connecting. `tasks.enabled: false` refuses the process.
The gateway still serves `/api/tasks`, `/api/catalog`, and `/api/outcomes`.

`rh mcp` serves the memory and wiki tools on stdio, or on streamable HTTP and
a Unix socket when `transports.mcp` selects them. Stdio also serves
`delegate_task` and `list_agents`; a network listener serves `list_agents`.
The sidecar does not claim jobs. It does not serve web, echo, or skills.

The task engine uses the kernel task ports. `rh agent` runs the job loop,
`rh mcp` exposes delegation, and `rh gateway` serves the task HTTP API.
CLI-backed providers are refused until their complete provider port is
available.

Recording notes are at `../testkit/tests/fixtures/roots/README.md` and
`../testkit/tests/fixtures/gateway/README.md`. Binary integration tests cover TLS,
revocation, identity refusal, websocket snapshots, memory, v1 turns, mesh turns,
configuration checking and signal shutdown. Existing route recordings remain
covered by their workspace replay suites; they are not all replayed through the
binary. Real installed herdr/ACP execution remains in the adapter smoke suites.
