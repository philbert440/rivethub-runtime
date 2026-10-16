# Operator CLI: B9, B10, B16

`Config::load` with `Profile::Node`, and `rh config validate`, still require `runtime`, `agents`, and `providers`. `rh local` never invents an agent, a provider, or an endpoint. When no config resolves it writes only `memory.backend: sqlite`, the database path under the state directory, and the gateway bind, then loads that file with `Profile::Standalone`, which does not require those sections. The task engine (`rh agent`) needs Postgres and is not part of `rh local`.

`rh` dispatches through the `cli` composition crate. Bare invocation and help print the derived clap command list.
The implemented commands include `version`, `config path`, `config show`, `config validate [path]`, `doctor [--json]`,
`status`, `stop`, `gateway`, `agent`, `worker`, `mcp`, and `local [--port <n>] [--no-lan]`. One typed `cli::Exit` owns exit codes, including the existing service startup codes.

`rh gateway` serves the gateway. `rh agent` runs the task engine. `rh worker --role <role>` runs one worker role. `rh mcp` runs the MCP sidecar.
`rh start` stays hidden from help and is the systemd unit's `ExecStart`. It supervises one child of this binary per
configured service root (`gateway`, `agent` when Postgres memory would start, each sqlite worker role, and `mcp` when that transport has a host or socket),
restarts an unexpected exit with backoff, and forwards SIGTERM. Deployments previously invoking bare `rh` to launch the gateway must use `rh gateway`. The
one-release compatibility binary dispatches through the same entry point.

Platforms: `rh` targets Linux and macOS (Apple Silicon). CI builds `rh` and runs the
`cli` tests on `macos-latest`. `rh service` and the restart step of `rh update` manage systemd units only. On macOS
running `rh start` under launchd (a LaunchAgent or LaunchDaemon plist) is the operator's job, and `rh update` refuses
any platform other than Linux x86_64 because that is the only published release asset.

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
