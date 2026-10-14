# Gateway v1 routes

Implemented under `crates/gateway/src/routes/v1/`, with boundary types in
`crates/wire/src/gateway/v1.rs`. The only shared gateway source change is the
fallback-router mount in `routes/mod.rs`. Existing origin, host, transport auth,
CORS and compatibility-header removal run before the new router.

## Surface and composition

- `GET /v1/models` obtains model names from `ModelProvider::list_models`.
- `POST /v1/chat/completions` runs exactly one headless `AgentLoop::run` turn.
  JSON and SSE share the same execution, audit, persistence and error handling.
  Text, reasoning and measured usage are preserved. SSE sends the assistant role,
  deltas, final choice, requested usage and `[DONE]`. Failed streams send a typed
  error and never send a success terminator.
- `GET/POST /api/conversations` list and create conversations.
- `GET/PATCH/DELETE /api/conversations/{id}` retrieve, rename and delete them.
- `x-rivet-conversation` selects an existing UUID; otherwise a conversation is
  created. Responses return that header. `x-rivet-title` supplies a title for a
  new conversation. These are the compatibility header names already used by
  the gateway CORS policy.

The spoofing test sends the recorded compatibility header `x-rivetos-user` to
verify the existing auth layer removes it. Skip-list names retain recorded
scenario identifiers verbatim, including legacy terms; they are fixture keys,
not product identifiers.

The composition entry is `Service::open(provider_id, provider, hooks, directory,
cancel)`, followed by `service.router(&gateway, secure)`. It receives the same
provider used to construct its internal agent loop. The caller supplies an
already-scoped conversation directory and gateway, owns the audit writer, and
calls `service.shutdown().await` before closing the audit writer. The default
gateway router exposes the mounted routes but returns 503 until the service is
attached. This slice does not add provider selection to the CLI composition root.

The direct gateway-to-agent-loop edge is explicitly required by this brief. Its
narrow dependency-policy exception is recorded in `deny.toml` and
`docs/ARCHITECTURE.md`; adapter dependencies remain forbidden. No lint policy was
changed.

## Ownership and integrity

Each conversation has a bounded command channel and its own worker. Registry and
snapshot locks are held briefly, never over an await. Independent conversations
run concurrently. An atomic turn lease refuses overlapping turns or mutations
of the same conversation with 409. All request workers and storage workers are
under a `TaskTracker` and cancellation tree. Dropping a response body cancels its
turn; shutdown cancels and joins the workers. Conversation creation finishes its
atomic commit under the tracker even if its HTTP caller disconnects.

Records are typed JSON files with UUID filenames, written by atomic replacement
and file/directory synchronization on blocking workers. Startup rejects corrupt
records and mismatched names. Parent traversal, symlink records and changed
directory targets fail closed; canonical path containment is component-wise.
Limits and truncation use characters for titles and bytes for request/response
sizes. Usage arithmetic is checked. Failed turns do not publish partial assistant
messages. The store assumes one service owns its directory.

## Evidence, assumptions and omissions

The supplied HTTP and WS recordings contain no v1 or conversations exchange.
Neither gateway fixture directory contains the README mentioned in the brief.
The final hard rules prohibit reading the fallback inventory, so sections 15.2
and 15.3 were not read. Exact parity for unrecorded routes therefore remains
unverified: route shapes and defaults above are explicit implementation choices,
not claims about unseen recordings.

Chosen limits are a 256 KiB request body, 4 MiB provider response, 120-second turn,
5-second audit cleanup and one provider step. Titles accept 1–200 characters; an
implicit title uses up to 80 characters from the final user message. Missing
stream means JSON. Missing usage stays absent. Model creation time is 0 because
the provider port supplies names, not model creation timestamps.

Tool execution, caller-supplied tools, multimodal content and sampling options
are outside this headless text slice and are refused through typed validation.
No provider adapter, tenant resolver or secret comparison was reimplemented.
The existing auth layer supplies no per-request tenant principal: storage scope
is the configured service instance, and dynamic user-to-store selection remains
the responsibility of the gateway auth foundation. PostgreSQL conversation CRUD
is not exposed by the existing memory port; these records use component-owned
files instead. Existing captured-memory records are not conversation records in
this component.

The notes stay inside this repository because the final hard rules prohibit the
requested external notes write. No prohibited inventory or source was read.

## Verification

The v1 integration suite has 13 tests covering JSON and SSE success, reasoning,
usage, exactly one audited provider turn, persisted messages, CRUD and restart,
Unicode lengths, typed invalid input, method/unknown-route handling, unavailable
service, origin and remote auth, instance isolation, traversal and symlink
refusal, provider failure, SSE failure without `[DONE]`, body limits, usage
overflow, disconnect, same-conversation conflicts, independent work and shutdown.
All event waits are bounded; tests do not sleep.

The three shared-boundary HTTP cases are compared as typed values without
normalizing both sides. `tests/fixtures/v1/skips.json` explicitly names the other
300 HTTP/WS scenarios and their reasons. A test checks completeness, uniqueness,
stale names and newly added in-slice recordings. User-registry fixture assets
are checked as valid/invalid typed documents. No recording was copied or edited.

Focused result: 13 passed. Full workspace nextest result:

```text
Summary [  15.544s] 579 tests run: 579 passed, 9 skipped
```

`SQLX_OFFLINE=true cargo xtask check --locked` passed:

```text
GATE OK: 0 finding(s), cargo xtask check
```

Both commands used the existing lane PostgreSQL environment. Verification logs
are retained under `target/rr-g-v1-check.log` and `target/rr-g-v1-nextest.log`.
