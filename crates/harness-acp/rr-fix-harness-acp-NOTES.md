# Final harness-acp fix round

Compared with local main `b0dd4582bb20b79c5d1195280d80b20f4c11e4c1` before changes.
The working tree initially matched main for harness-acp.

Already fixed on main (harness commit `e1622a4`):

- ACP dependency upgraded to 3.3; model/effort use categorized config options and
  `session/set_config_option`, not `session/set_mode`.
- Missing working directories produce `AcpError::WorkingDir`; no `current_dir`
  fallback remains.
- Child wait errors propagate through the reaper; a signal exit remains
  `SessionEvent::Exited { code: None }`, distinct from a wait error.
- Grok's native invocation is `grok agent stdio`, marked verified on main.

Implemented:

- Claude Code uses `claude-agent-acp` from
  `@agentclientprotocol/claude-agent-acp` 0.90.0. Configured paths retain whitespace.
- Offerings consume agent capabilities and categorized model config options.
  Display names are the advertised names; the current selection is default.
  Grouped options work, selections and config notifications refresh the cache,
  and an unknown current model is a typed error. Explicit replace/merge model
  configuration remains supported. Session listing follows the agent's advertised
  listing capability.
- Discovery uses a session's advertisement, or a temporary ACP connection with
  an explicitly configured working directory. Probe errors propagate, and the
  probe child and connection tasks are cancelled and joined.
- Hermes timestamp parse errors retain their typed source instead of becoming
  `false`. This removes both harness-acp FALLBACK findings from production.
- Removed the single obsolete harness-acp FALLBACK baseline entry (count 2).
  The follow-up instruction to make the gate green and commit resolves the
  earlier conflict for this explicitly requested baseline reduction. No entry
  was added or increased and no gate behavior was weakened.
- Missing permission titles and non-text media produce typed errors. Missing tool
  output stays absent; explicit empty output stays present. Notification conversion
  errors are returned to ACP rather than silently accepted.
- Permission request ids use UUID v4 randomness. Concurrent withdrawal is an
  explicit unknown-request error rather than a successful answer.
- Session tasks use TaskTracker and CancellationToken; release waits for teardown.
  Stderr read failures are reported. Protocol tests use event deadlines and their
  shared helpers moved to keep each file below 600 lines.
- Initialize probes can attempt an unverified candidate, instead of refusing it
  before it can be tested.

Scope and unresolved constraints:

- The only file edited outside harness-acp is xtask/baseline.toml, removing the
  obsolete harness-acp entry. No external call-site changes.
- SessionId still exposes only its canonical string. Removing the duplicated
  prefix parse needs a shared wire accessor. Permission for that minimal scope
  expansion was requested and has not been received.
- No real harness binary was found on this lane's PATH. Hermes/Kimi/Qwen launches
  remain unverified; no invocation or successful live test is invented. No package
  was installed. Grok retains the verification already present on main.
- No ACP recording corpus or ACP fixture README exists under testkit fixtures in
  this checkout. The capture/stores README identifies synthetic transcript inputs,
  not ACP recordings. CONTRACT B12 pins ACP to schema/live conformance and records
  terminal behavior. No foreign slice's scenarios were used as an ACP skip list.
  Protocol coverage is Rust stdio conformance, not a claimed recording replay.
- Crate size targets, Direct PTY and zod issue JSON remain Phil decisions and were
  not changed.
- Notes are inside this crate because the final hard rule permits writes only
  inside this repository, conflicting with the earlier shared notes destination.
- The lane Postgres port was reachable. Services/database configuration were not
  changed. No push, branch switch, reset, stash, rebase or git configuration edit.

Validation:

- Workspace rustfmt check passed; staged diff whitespace check passed.
- Focused Clippy with all targets/all features and `-D warnings` passed.
- Final focused nextest run: 52 passed, 14 opt-in live tests skipped.
- First workspace run: 112 passed, one failed, 16 skipped; 1,066 tests were not run
  because nextest stopped at `cli::process::pid_files_must_be_regular_files`.
  Its `nonblocking PID read` deadline expired. This exact flaky test is named in
  the fix-round brief. The isolated retry passed in 0.161 seconds.
- Full workspace retry: 455 passed, five failed, 16 skipped; 719 tests were not run.
  All failures were gateway serving deadline tests outside the permitted edit scope:
  `stalled_request_head_is_cut_off`, `stalled_tls_handshake_is_cut_off`,
  `stalled_tls_handshake_does_not_block_other_clients`,
  `tls_serving_answers_healthz_over_rustls`,
  `trickled_request_body_hits_the_request_deadline`.
- Initial lint findings in the new code were fixed (borrowing, exhaustive enum
  arms, earlier lock release, explicit test-only helper module).
- The previous required xtask gate failed with exactly one finding: the obsolete
  harness-acp baseline entry records two fallbacks while the code has zero.
  Workspace formatting, Clippy, test inventory, dependency checks, SQLx prepare
  verification and all other repository checks passed.
- The previous workspace run without another compiler job from this task passed:
  1,179 passed, 16 skipped in 57.939 seconds. It used the lane DATABASE_URL.
  The preceding timeout failures did not reproduce in this run; overlap with
  compilation is a possible contributor, not a proven cause.
- After removing the obsolete baseline entry, the required gate passed with
  zero findings. The completion workspace run retains SQLX_OFFLINE=true for
  compilation; database tests still use the lane DATABASE_URL.
- Completion workspace run: 1,179 passed, 16 skipped in 55.751 seconds.
  Both required commands exited successfully after the baseline reduction.

Last required gate lines:

```text
Done!
GATE OK: 0 finding(s), cargo xtask check
```

Last required workspace run lines:

```text
Starting 1179 tests across 166 binaries (16 tests skipped)
────────────
Summary [  55.751s] 1179 tests run: 1179 passed, 16 skipped
```

Both completion checks passed. These changes are prepared for the requested
single Conventional Commit. No push will be made.
