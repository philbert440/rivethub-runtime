# Gateway composition recordings

`startup.json` is the existing startup-refusal recording, moved without changing
its contents from the gateway crate so library and binary replays share one copy.
It contains four refusal scenarios, not a successful startup or shutdown trace.
The legacy names in its error lines are B1/B16 compatibility text.

`rivethub/tests/startup.rs` exercises the recorded refusals through
`rh gateway --check`. Only dynamic expected paths are substituted; actual output
is compared directly. `composition-skips.json` names the single validation-order
deviation. An inventory test requires every recording scenario to appear once as
replayed or explicitly skipped with a reason.

`http-plain.json` remains the HTTP transport recording. Existing gateway and
testkit replays cover HTTP, sessions, terminal, memory, tags, v1, wiki and mesh.
The composition tests additionally start the real binary with an ephemeral test
CA and database, exercise health, memory, TLS client identity, a revoked leaf,
the mesh agent listener and `/ws`, and await shutdown through both Unix signals.
These live tests pin the current startup and shutdown lines independently of the
historical refusal recording. They await readiness and socket events with
timeouts, without startup polling or wall-clock sleeps.

The composed registry tests deserialize `wire/tests/golden/gateway/users-tenancy.json`
using the shared B11 `UsersRegistry` type and refuse the invalid registry fixture.
TLS coverage maps verified SPIFFE user/device pairs and legacy device leaf names,
including refusal of unknown names, conflicting user claims, duplicate mappings,
and malformed explicit files. No fingerprint registry format is accepted.
