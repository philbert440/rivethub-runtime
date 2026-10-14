# Gateway sessions

The B1/B2/B12 session boundary is provided by `SessionRegistry`, `TerminalService`
and `Tenancy`. The composition root supplies a shared registry, the kernel
`TerminalMux` port, a permitted command roster and filesystem root, and optionally
`HarnessAdapter` and `TranscriptReader`. Attach these with the corresponding
`Gateway::with_*` methods. Identity defaults to refusal; configure explicit local
ownership, bearer credentials, or verified TLS certificate fingerprints.

Each terminal and harness session has its own bounded command channel. Registry
ownership and per-tenant projections use separate short-lived locks. Snapshot
creation and subscription share the projection lock, so an update cannot disappear
between them. Slow subscribers close and reconnect for a new snapshot. Session
deletion keeps an ownership tombstone so another tenant cannot acquire its id.

A chat-to-terminal switch waits for both the prompt result and `TurnEnded`, releases
and reaps ACP, then opens the terminal with the native resume id. Messages submitted
while the terminal owns the session queue until a turn-end hook or explicit stop;
the terminal exits before ACP resumes. There is one pending terminal switch and a
bounded message queue. No chat text is injected into a terminal. Shell commands
have no chat controller or session projection.

The session routes include `/event`, `/events`, `/sessions`, `/state`, `/session`,
`/ws`, harness-session creation, turn, stop, listing and transcript reads. Terminal
routes include `/term`, configuration, listing and the `/api/terminal` aliases.
The event socket emits a typed v1 snapshot followed by events/removals. The terminal
socket emits hello, replay bytes, live bytes, ownership changes and exit before
close. Binary input is passed to the port without Unicode decoding.

The kernel ports now expose terminal inspection, atomic replay attachment and exit
waiting, and harness release. The terminal adapter implements these for PTYs and
attaches its PTY to herdr when selected. ACP release waits for process reaping.
Shutdown cancels and joins component tasks; the serving layer closes the gateway
before waiting for upgraded connections.

## Validation and limits

`tests/session_replay.rs` replays typed values from the three requested recordings.
An inventory test requires every scenario to be named exactly once as replayed or
excluded. Exclusions and reasons are in `tests/session_skips`. The recording
fixture uses scripted kernel ports; dynamic process identifiers and timestamps are
substituted only in recorded expectations. Actual responses are compared directly.

`tests/sessions.rs` checks concurrent spawn ownership, both handoff directions,
queued prompts, tenant isolation including batch atomicity and aliases, default
refusal, snapshot/live ordering, byte preservation, malformed controls, exit
ordering, deletion and component-wise directory containment including symlinks.
All asynchronous assertions wait on events with timeouts.

Excluded recording behavior includes JavaScript coercion/prototype handling,
lossy terminal decoding, duplicate-key/lone-surrogate producer events, legacy
socket destruction, and terminal injection. Preset provisioning, transcript
discovery and recorded-cwd lookup, roster reload and multiplexer discovery belong
to other adapters/slices. Gateway capacity refuses new terminals instead of
evicting live sessions. Several subprocess/environment and dependent snapshot
recordings remain excluded; the port-level lifecycle invariants cover those
boundaries, but this is not a claim of full subprocess replay coverage.

The registry is in memory. This change exposes library composition APIs rather
than adding deployment credential/roster loading to the binary. Transcript reads
require an injected reader and a registered owner. All harnesses currently use
exclusive handoff; simultaneous multi-client harness attachment is not exposed by
the kernel port. Real installed harness/herdr smoke tests remain the workspace's
opt-in tests; no binaries are installed by this change.
