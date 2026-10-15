# ACP harness adapter

Claude Code launches through `claude-agent-acp`, from
`@agentclientprotocol/claude-agent-acp` 0.90.0. Its configured binary path is passed
unchanged. Grok uses `grok agent stdio`. Other launch entries retain their explicit
unverified status until their real invocation has been tested; setting a binary
path alone does not establish that it speaks ACP.

Model and effort selection use the agent's categorized session config options and
`session/set_config_option`. Model offerings preserve the agent's display names
and mark its current selection as default. Config notifications refresh that
selection. Session listing is advertised when initialize reports the capability.
Offerings use an existing session's advertisement when available; otherwise a
known launcher with an explicit working directory is probed through a temporary
ACP session, then cancelled and reaped. Missing working directories remain absent.

Each session has its own command channel and cancellation token. Background tasks
are tracked, and release waits for the child and its connection tasks to finish.
Permission decisions require an explicit offered option. Missing permission titles
and content that cannot be represented by the kernel's text event are typed errors.

The protocol integration tests use the Rust fake agent over real stdio. They cover
new/load/prompt/cancel, permissions, config selections, discovery, process failures,
and shutdown. All synchronization waits for events with deadlines. Live binary
probes are opt-in tests in `real_binaries`; no package is installed by those tests.

CONTRACT B12 pins ACP to schema conformance and live binary checks; its recordings
cover terminal behavior. This checkout contains no ACP recording corpus or ACP
fixture README in testkit.
The capture store README describes synthetic transcript examples, not ACP traffic;
those are not presented as protocol recordings or added to an ACP skip inventory.
