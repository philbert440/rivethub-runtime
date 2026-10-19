# Operator CLI: B9, B10, B16

`Config::load` with `Profile::Node`, and `rh config validate`, still require `runtime`, `agents`, and `providers`. `rh local` never invents an agent, a provider, or an endpoint. When no config resolves it writes only `memory.backend: sqlite`, the database path under the state directory, and the gateway bind, then loads that file with `Profile::Standalone`, which does not require those sections. `rh agent` needs Postgres. `rh local` runs the task engine on the SQLite store unless `tasks.enabled` is false; it serves `/api/tasks`, `/api/catalog` and `/api/outcomes` and backs MCP delegation with the same engine. Its directory holds the configured agents and the presets in `agents.json`, read at start.

`rh` dispatches through the `cli` composition crate. Bare invocation, `rh help` and `rh --help` print the derived clap
command list, and `rh <command> --help` prints one command's arguments. One typed `cli::Exit` owns exit codes, including
the existing service startup codes. Every visible command:

| Command | Arguments | Does |
|---|---|---|
| `rh version` | | Print the RivetHub version |
| `rh config path` | | Print the resolved configuration path |
| `rh config show` | | Show a configuration summary without secrets |
| `rh config validate` | `[PATH]` | Validate configuration and print findings |
| `rh doctor` | `[--json]` | Check this node without applying migrations |
| `rh status` | | Report gateway health |
| `rh stop` | | Signal the running instance to stop |
| `rh gateway` | `[--check] [-c, --config FILE]` | Run the composed gateway service |
| `rh agent` | `[--check] [-c, --config FILE]` | Run the task engine |
| `rh worker` | `--role embedder\|summarizer\|categorizer\|wiki-builder\|all [--check] [-c, --config FILE]` | Run a memory worker for one role |
| `rh mcp` | `[--check] [-c, --config FILE]` | Run the MCP sidecar |
| `rh local` | `[--port N] [--no-lan] [--ensure [--wait SECONDS]]` | Run a standalone node on SQLite, or start one in the background with `--ensure` |
| `rh update` | `[--channel stable\|beta\|nightly] [--version TAG] [--no-restart] [--mesh] [--ssh-user USER]` | Update this node or the mesh |
| `rh db migrate` | `[-c, --config FILE] [--url URL \| --path FILE]` | Apply database migrations |
| `rh db status` | `[-c, --config FILE] [--url URL \| --path FILE]` | Report schema status read-only |
| `rh service init` | | Write the systemd unit (system unit as root, user unit otherwise) |
| `rh service start`, `rh service stop`, `rh service restart` | | Control the systemd unit |
| `rh service status` | | Show the systemd unit status |
| `rh service logs` | | Print the unit's journal |
| `rh ca token node` | `NAME` | Mint a step-ca token for a node in the operator roster |
| `rh ca token agent` | `NAME` | Mint a step-ca token for an agent in the operator roster |
| `rh ca token device` | `NAME` | Mint a step-ca token for one device name |
| `rh ca revoke` | `SERIAL` | Revoke one certificate by decimal serial |
| `rh ca crl` | | Print the certificate revocation list |
| `rh ca status` | | Show the certificate authority roots |
| `rh cert renew` | `--principal P --ca-url URL --trust-root FILE [--token-file FILE] [--state DIR] [--once]` | Renew this holder's certificate |
| `rh mesh list` | | List mesh nodes |
| `rh mesh ping` | `[--timeout MS]` | Probe mesh nodes (default timeout 5000 ms) |
| `rh mesh enroll` | `[TARGET] [--name NAME] [--advertise ADDR] [--hub-cmd CMD] [--state DIR]` | Enroll this node through step-ca and record it |
| `rh mesh sync` | `[TARGET] [--hub-cmd CMD]` | Merge a hub mesh registry |
| `rh mesh renew` | `[TARGET] [--name NAME] [--hub-cmd CMD] [--state DIR]` | Renew this node's holder certificate |
| `rh mesh join` | `[HOST] [--manual] [--port N] [--name NAME] [--advertise ADDR]` | Join a mesh |
| `rh mesh status` | | Show the mesh registry |
| `rh mesh forget` | `[TARGET] [--force]` | Remove a mesh node |
| `rh memory` | `queue-status`, `retry-failed`, `requeue`, `backfill-tool-synth`, `export`, `import`, `tags backfill-rule`, `backfill-titles` | Administer memory; `rh memory --help` lists each command's flags |
| `rh logs` | `[agent] [-f, --follow \| --no-follow] [-n, --lines N] [--since TIME] [--level NAME] [-g, --grep TEXT] [--json] [--bare-metal]` | Read the service journal |

The `rh ca` commands also take `--ca-url URL`, `--root FILE`, `--provisioner-password-file FILE`, `--roster FILE` and
`--crl-out FILE`; `rh mesh enroll` and `rh mesh renew` take `--ca-url URL` and `--root FILE`, `rh mesh enroll` also takes
`--provisioner-password-file FILE` and `--roster FILE`, and every `rh mesh`
command takes `--json` and `--ssh-user USER`. `--check` on a service root validates its configuration and exits
without serving.

`rh local --ensure` starts a standalone node on demand for launchers and capture hooks that run without a service manager. It adopts legacy state as `rh local` does, resolves the address `rh local` would serve with the same `--port` and `--no-lan` (the resolved config, or the bind it would generate), and exits 0 when anything answers that address's `/healthz`, a degraded 503 included. Otherwise it takes a non-blocking lock at `~/.rivethub/run/local-start.lock`, starts `rh local` with the same flags in its own process group with stdin closed and output appended to `~/.rivethub/logs/local.log`, and waits up to `--wait` seconds (default 2, 1 to 300) for health; a caller that finds the lock held only waits. When nothing answers in time it exits 1 naming the address and the log, so a capture hook can spool its batch. The node still refuses a second instance through its pid-file claim.

`rh gateway` serves the gateway. `rh agent` runs the task engine. `rh worker --role <role>` runs one worker role, claiming up to four jobs at a time, or `RIVETHUB_WORKER_CONCURRENCY` (a positive count) when set. `RIVETHUB_SUMMARY_LEAF_TOKENS`, `RIVETHUB_SUMMARY_BRANCH_TOKENS` and `RIVETHUB_SUMMARY_ROOT_TOKENS` cap each summary request (defaults 7000, 14000, 20000), and `RIVETHUB_EMBED_TIMEOUT_MS` bounds each embedding request (default 30000). `rh mcp` runs the MCP sidecar.
`rh start` stays hidden from help and is the systemd unit's `ExecStart`. It supervises one child of this binary per
configured service root (`gateway`, `agent` when Postgres memory would start, each sqlite worker role, and `mcp` when that transport has a host or socket),
restarts an unexpected exit with backoff, and forwards SIGTERM. Deployments previously invoking bare `rh` to launch the gateway must use `rh gateway`. The
one-release compatibility binary dispatches through the same entry point.

Browser origins: the gateway answers a request that carries an `Origin` header only when that origin is the
gateway's own origin, the desktop app's built-in origin, or is listed in the gateway's `allowed_origins` setting (the `den` section of `config.yaml`, including the file
`rh local` writes). Origins compare by scheme, host and port, so `localhost` and `127.0.0.1` are separate entries. A
web client served by a Vite dev server on port 5273 that talks to the gateway needs that origin listed:

```yaml
den:
  allowed_origins:
    - http://127.0.0.1:5273
    - http://localhost:5273
```

An unlisted origin is refused with `origin "<origin>" is not allowed (add it to the gateway allowed_origins setting)`.

Platforms: `rh` targets Linux and macOS (Apple Silicon). CI builds `rh` and runs the
`cli` tests on `macos-latest`. `rh service` and the restart step of `rh update` manage systemd units only. On macOS
running `rh start` under launchd (a LaunchAgent or LaunchDaemon plist) is the operator's job, and `rh update` refuses
any platform other than Linux x86_64 because that is the only published release asset. The PKI commands refuse a
state, holder or authority path that passes through a symlink; on macOS `/tmp` and `/var` are symlinks into
`/private`, so keep those paths under the home directory (the default `~/.rivethub` is fine).

Doctor calls `migrate`'s read-only schema status library APIs in-process. Postgres uses the async status API;
SQLite runs on a blocking task. The typed status stays in `migrate`. Doctor never runs migrations or opens a store
for writing. An unreachable backend and an old or unsupported schema fail the memory check explicitly.

Configuration discovery, parsing, validation, defaults, and compatibility notices remain in `config`. Show prints only
agent/provider mappings, default agent, and workspace. Status uses the gateway binding configuration and the shared
health response, with a three-second HTTP timeout and a diagnostic before PID fallback. Stop reads the configured
state directory's PID file, rejects group/system PIDs and out-of-directory symlinks, and sends SIGTERM.

The supplied `testkit` CLI fixture directory contains only its README, with no executable transcripts. Tests exercise
that README's scenarios and check its complete deferred-command list against named reasons. The fixture inventory
check fails if transcripts are added, requiring their integration instead of silently skipping them. Tests also cover
real binary dispatch, the compatibility alias, the existing gateway's health route and graceful shutdown, invalid
PID files and symlink boundaries, real signal delivery, and read-only doctor checks against both backends. Schema
tests assert unchanged database state. Timing tests await events with deadlines.

`rh update [--channel stable|beta|nightly] [--version TAG] [--no-restart] [--mesh] [--ssh-user USER]`
updates Linux x86_64 installations. Stable is the default and resolves the latest published stable GitHub release,
or the exact tag supplied with `--version`. Both `rh-linux-x86_64` and `rivethub-migrate-linux-x86_64` must match
`SHA256SUMS` before either is executed. HTTPS redirects are supported; HTTPS downloads cannot downgrade to HTTP.
Version tags are URL path components, never shell expressions. The release workflow builds both binaries when a
version tag is pushed. Releases use the workspace package version and require a matching `v<version>` tag.

Beta and nightly require `RIVETHUB_INSTALL_ROOT` to name an absolute, clean source checkout on `beta` and `main`,
respectively. The deprecated install-root alias remains in `config` and emits its existing warning. The updater runs
`git pull --ff-only origin <branch>` then `cargo build --release --locked` for both binaries into an isolated staging
directory. With a source channel, `--version` asserts that the updated checkout resolves to that commit or tag;
a different revision is refused before building. The updater never changes the checkout's branch or discards edits.

Updates require a configured memory backend. The staged migration binary applies that backend's migrations before
replacement or restart, including with `--no-restart`. A download, checksum, build, or migration failure leaves the
installed executables untouched. A lock serializes updates to the installation. Each replacement uses a rename on
the installation filesystem; `rh.previous` and `rivethub-migrate.previous` retain the previous executables for manual
rollback. If the second rename fails, the first is restored. Schema changes remain forward-compatible: binary rollback
does not undo migrations. Restart uses the installed system or user systemd service through the existing service CLI
implementation. A restart failure exits 1 and retains both backups; it does not undo the database migration.

Mesh mode reads the existing typed registry at `<RIVETHUB_SHARED_DIR>/mesh.json` through the gateway's registry reader.
It resolves the local node name with the same config, environment, and operating-system hostname precedence as the runtime. It validates every SSH destination and concurrently preflights SSH
before updating locally, then visits every reachable peer in registry order. Each peer executes its own `rh update`;
`--mesh` is never forwarded. Stable peers receive the exact release tag installed locally. The explicit SSH user overrides the registry user; absent both, SSH selects its normal user.
Batch mode and strict host-key checking use the operator's existing SSH trust. Paths and secrets are not interpolated
into a remote shell. Unreachable peers and failed peer updates are reported, other reachable peers continue, and the
aggregate exit is 1 if any peer failed. Local failure stops the rollout. Success exits 0; cancellation exits 1.

`rh db migrate|status [--config FILE] [--url URL | --path FILE]` uses the migration library for the selected Postgres
or SQLite backend. Without an explicit database, it resolves the normal config, including minimal standalone configs.
Status is read-only and reports `current`, `required`, and `pending`; migration reports the resulting status and is
idempotent. SQLite operations run on blocking workers. Cancellation drains an in-flight migration before reporting cancellation. Migration is an explicit operator action, never runtime startup
behavior introduced by these commands. Store errors direct operators to `rh db migrate`.

No update/db transcripts or their referenced inventory sections were supplied in the repository's CLI fixture directory.
The new tests cover the behavior above using isolated databases, a local HTTP release server, and fake process executables.
The update/db skip list is empty and checked; the fixture inventory check fails when new recordings arrive. Existing
README contract tests remain unchanged. Historical update/db wording and exit-code parity cannot be established without
those recordings; no transcript or TypeScript artifact was invented to fill that gap.
