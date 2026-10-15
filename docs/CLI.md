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
