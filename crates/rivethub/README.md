# Gateway composition

`rh gateway` starts the gateway on one multithreaded Tokio runtime.
`rh gateway --check` loads and validates configuration and TLS material without
connecting the memory database, binding listeners, or moving legacy state.
Configuration uses `~/.rivethub/config.yaml` and the config crate's `RIVETHUB_*`
environment resolution and deprecated aliases. Invoking `rh` without arguments
retains the existing gateway startup behavior.

The composition supplies Postgres memory, the shared session registry, local
owner identity, verified device names, the Claude transcript reader,
configured OpenAI-compatible providers and agent loops, and optional terminal
and mesh services. HTTP routes use the gateway router, including capture,
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
they are never redirected to Postgres. Core/static, agents, devices and files
route modules are absent from this checkout and are not fabricated here. The
users file is a composition configuration boundary, not a devices management
API. CLI-backed providers are also refused until their complete provider port is
available. These are outstanding upstream dependencies for the full requested
composition, not successful standalone-node coverage.

The shared recording README is at
`../testkit/tests/fixtures/gateway/README.md`. Binary integration tests cover TLS,
revocation, identity refusal, websocket snapshots, memory, v1 turns, mesh turns,
configuration checking and signal shutdown. Existing route recordings remain
covered by their workspace replay suites; they are not all replayed through the
binary. Real installed herdr/ACP execution remains in the adapter smoke suites.
