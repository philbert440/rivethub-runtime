# Operator CLI: B9, B10, B16

`rh` dispatches through the `cli` composition crate. Bare invocation and help print the derived clap command list.
The implemented commands are `version`, `config path`, `config show`, `config validate [path]`, `doctor [--json]`,
`status`, and `stop`. One typed `cli::Exit` owns exit codes, including the existing service startup codes.

`rh start` retains the existing gateway launcher. It is hidden from this slice's help; process supervision and the
remaining service commands are deferred. Deployments previously invoking bare `rh` to launch the gateway must use
`rh start`. The one-release compatibility binary dispatches through the same entry point.

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
