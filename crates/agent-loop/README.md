# Agent turn and safety hooks

`AgentLoop` performs one turn through `kernel::ModelProvider` and `kernel::Tool` (B8), emitting model chunks on a bounded
channel. It assembles interleaved tool deltas by index, rejects incomplete or inconsistent streams, preserves reasoning,
accumulates usage with checked arithmetic, and feeds serialized MCP results back as tool messages. A step is one provider
request followed by its requested tools. The step limit includes the final tool batch; a zero limit makes no provider call.

The composition root supplies a provider, a fixed tool catalog, the hook pipeline, and explicit per-turn authorization.
Only authorized catalog definitions reach the model. Unavailable, unauthorized, or safety-blocked tools produce MCP error
results without execution. Tool execution errors are also returned to the model. Provider, stream, audit, and pipeline
failures fail the turn. Request tool definitions are rebuilt from the authorized catalog; caller-supplied definitions
cannot authorize a tool. There are no credentials or secret comparisons in this crate.

Turn state belongs to the calling task. Independent turns can share an `Arc<AgentLoop>`; each turn owns its tool `JoinSet`.
Independent calls execute concurrently and results return in provider index order. Cancellation and the overall timeout
drop provider futures and receivers, abort and join tool tasks, then audit an aborted `turn:after`. The separate
`cleanup_timeout` bounds that final audit attempt. Dropping the run future also aborts its tool tasks. The ports do not
carry a cancellation token, so adapters must stop their work when the port future or stream receiver is dropped.
`ModelChunk::Done` marks each provider response, while `TurnResult` marks completion of the whole turn.

`HookPipeline` accepts all typed `wire::hooks::HookContext` variants (B15). Its fixed stages are shell danger, workspace
fence, then audit. Existing block/skip decisions are preserved. Safety errors become typed pipeline error records and a
blocked context. Audit errors propagate; no execution proceeds without an acknowledged audit write. The dedicated audit
actor owns its file, writes one serialized `HookPipelineResult` per line, and acknowledges only after flush and sync.
Its caller-owned `JoinSet` reports writer failures. Cancel its token and join it at shutdown; queued records drain first.
Timestamps are supplied at the edge in the turn context; durations use monotonic time.

Shell checks use a compiled data set of regular expressions and explicitly configured tool names. Wrong command types
are rejected. A missing command is permitted by the B15 `command missing` fixture. These patterns detect dangerous
commands; they are not a shell parser or an execution sandbox. Tools still own their operating-system permissions.
The workspace fence accepts configured tool names and existing allowed directories, normalizes components, rejects
parent traversal, and compares component-wise with `Path::starts_with`. Existing ancestors are canonicalized on a blocking
task, so symlink escapes and dangling links fail closed even for new leaf files. Relative paths require an explicit
working directory. Empty roots deny every tracked path. No home directory is inferred. A pre-execution check cannot
eliminate filesystem races across an opaque Tool port; the actual file tool must enforce containment at its IO boundary.

The replay suite reads the shared fixtures through `testkit::recording`. Recorded absolute paths are mounted beneath a
repository-local temporary filesystem, including the fixture symlink. Expected outcomes are compared directly as typed
values. `tests/fixtures/skips.json` lists out-of-slice fixture names and reasons; it is empty because all 19 supplied cases
are in scope. A test checks the list against fixture names and the replay count.

Not included: session ownership, skill execution, delegation, reflection, compaction, runtime plugin loading, arbitrary
shell/HTTP hook launchers, built-in shell/file tools, provider adapters, or workflow journals. Those belong to other
services/adapters, or are explicitly excluded by the runtime design. Their existing wire hook events can still pass
through this pipeline and be audited.
