# Model providers

`cli::CliBacked` implements the kernel `ModelProvider` over an injected
`Arc<dyn HarnessAdapter>`. Each call to `stream` or `chat` creates one fresh,
unregistered headless session. The requested session id is a random UUID;
subsequent operations use the canonical id returned by the adapter.

The composition root supplies the harness, an existing `embed::Embedder`, and a
shutdown token. The embedder is required because `ModelProvider` includes
embeddings; ACP has no embedding operation. Model discovery uses the selected
harness's offerings and includes the configured model. The provider has no
`harness-acp` dependency and does not launch processes itself.

## Configuration

The existing typed `config::provider::CliProvider` entry is reused:

```yaml
providers:
  coding:
    type: codex-cli
    model: example-model
    cwd: /home/user/project
    effort: high
    timeout_ms: 120000
    system_prompt: Keep answers concise.
```

The supported types select claude-code, codex, grok-build, hermes, opencode and
kimi-code respectively through `claude-cli`, `codex-cli`, `grok-cli`,
`hermes-cli`, `opencode-cli` and `kimi-code`. Omitted options remain absent;
`timeout_ms`, when present, must be positive. Request model and thinking select
the turn options; otherwise configured reasoning effort or effort is used.
The working directory reaches the harness port unchanged, where the harness
adapter enforces its workspace policy. Binary launch settings belong to the
injected harness's configuration at composition time.

The text-only kernel prompt port receives a serde JSON envelope preserving the
complete message history, roles, tool definitions, optional system prompt and
request token budget. It is prompt context: it does not install remote tools or
provide a protocol-level token limit. Tools execute within the harness. Every
permission request is denied; this headless provider has no interactive approval
channel. Legacy process-driver options in the shared config shape are not
translated into subprocess flags by this adapter.

Text, reasoning and usage map directly onto model chunks. A complete harness
tool call produces a start and one serialized argument delta. Tool results and
session metadata have no corresponding `ModelChunk` and are not emitted.
Usage remains absent unless the harness sends the kernel usage event. The ACP
adapter's production usage extraction is a separate integration task.

Prompt completion and event consumption run concurrently to prevent permission
or bounded-channel deadlocks. Success requires both prompt success and a turn-end
event. The session is released before `Done`. Errors, deadlines, receiver drop,
provider drop and shutdown cancel then release the session. `shutdown().await`
waits for tracked sessions. Creation is allowed to finish before cancellation
cleanup so dropping an in-flight creation cannot orphan its returned session.
The turn timeout starts after creation; the injected adapter owns startup and
release deadlines.

## Tests

The test-only harness implements every port operation. Tests replay the shared
`provider` and `providers/vllm` chunk recordings, comparing JSON arguments as
typed values. These are shared stream observations, not CLI process recordings:
the fixture README explicitly excludes CLI subprocesses. The named skip table
covers only the HTTP availability probe and is checked against the fixture
inventory. Additional tests cover permissions, lifecycle errors, cancellation,
concurrency, Unicode, usage, config typing, discovery and embeddings.

OpenAI-compatible discovery returns the configured model when `probe_models` is
false and makes no HTTP request. Embedding replies require typed numeric vectors
and explicit, unique indices covering every input; malformed replies fail instead
of selecting slot zero. Token errors retain their original typed sources across
concurrent callers.
