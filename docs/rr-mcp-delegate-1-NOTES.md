# MCP delegation

`kernel::tasks::Delegation` is the port. `agents` returns presets and runtime agents with the nodes that host them. `delegate` takes a target, prompt, optional model and node, and a timeout, and waits for the task outcome under a `CancellationToken`. `tasks::Engine` implements the port over directory resolution, submit, load, and wait. `mcp` registers `delegate_task` and `list_agents` through `Server::builder().delegation(Arc<dyn Delegation>, Surface)`. `delegate_task` is registered only on `Surface::Stdio`. The rivethub composition root is not wired; the orchestrator injects the port.

Recorded `tools/list` entries for the two tools are compared as parsed values after `translate_recorded_product_names`. The recording has no delegation call payloads. The checked skip list is the five product-removed tools (`echo`, `skill_list`, `skill_manage`, `internet_search`, `web_fetch`). A cancelled call kills the row. A timeout returns the queued row and does not kill it.

Postgres on `127.0.0.1:5441` refused connections at the start of this run and accepted them before `cargo xtask check`. No database service was started, stopped, or reconfigured.

## Commands

- `cargo fmt --all -- --check`: exit 0.
- `cargo clippy -p tasks -p mcp -p kernel --all-targets --all-features --locked -- -D warnings`: exit 0.
- `cargo xtask check --locked`: `GATE OK: 0 finding(s), cargo xtask check`. This run includes workspace `cargo clippy --locked --all-targets --all-features -- -D warnings`, `cargo deny --all-features --locked check` (advisories, bans, licenses, and sources ok; pre-existing unused-wrapper warnings), cargo-machete, and `cargo sqlx prepare --workspace --check`.
- `cargo nextest run -p mcp -p tasks -p kernel --all-features --locked`: `Summary [   1.555s] 164 tests run: 164 passed, 0 skipped`.
- `cargo nextest run --workspace --all-features --locked`: final result `Summary [  11.474s] 1184 tests run: 1184 passed, 16 skipped`. Two earlier full runs and one `--no-fail-fast` run failed only `gateway::audio mic_round_trip_writes_pcm_and_rejects_bad_frames` with `deadline has elapsed` (2s socket wait). That test passes alone in 0.17s and is outside this slice. A `--test-threads=8` run also passed all 1184 tests. The 16 skips are pre-existing workspace skips.
- `cargo deny --all-features check`: advisories ok, bans ok, licenses ok, sources ok.
- `cargo build --workspace --locked`: exit 0.
