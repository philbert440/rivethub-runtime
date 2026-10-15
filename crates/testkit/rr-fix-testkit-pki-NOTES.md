# Final fix round: testkit, xtask, PKI

Base: local main and HEAD were both `b0dd458` before edits. No other checkout was read. The design HTML, FINAL-FIX-ROUND.md, repository PATTERNS and fixture READMEs were consulted.

The requested external notes location conflicts with the final hard rule permitting writes only inside this repository. These notes are therefore stored here.

## Checked against main

- xtask non-UTF-8 source skipping: already fixed. `code::comments` emits UTF8 findings; `rustsrc::read_sources` propagates decoding errors. `non_utf8_source_is_a_finding` already covers it.
- xtask patterns tuple model: already fixed. `patterns::Entry` has named `name` and `body` fields.
- testkit test using `env::temp_dir`: already fixed; tests use owned tempfile directories.
- No check-10/FALLBACK entries existed for PKI or xtask. Ten entries covering twenty testkit occurrences remained. Those entries were removed, without adding any baseline or weakening a check.
- No testkit replay directly loaded `diff/*.jsonl` on current main. Its own synthetic cases replay still emulated config internals and JavaScript undefined; that replay is now an explicit checked exclusion inventory.
- Summarizer fixture naming was already corrected on main.

## Changes

- PKI draws all new certificate serials from 16 OS-random bytes, refuses zero, checks persisted imported and issued serials, and bounds collisions to sixteen attempts. Allocation and issuance remain in the existing immediate SQLite transaction. Root and intermediate certificates reserve serials as well. Imported OpenSSL serial metadata remains readable for compatibility, but is never incremented or used to allocate new serials.
- Provider fakes require explicit chat, stream, embedding and model scripts. FNV embeddings remain an explicitly selected mode and reject zero width. Exhausted scripts are typed errors. Transcript fixtures validate session IDs at construction and reject missing sessions. Provider and reader responsibilities are separate modules.
- The fake echo tool parses a serde input type; schemars derives its schema from that type. Construction propagates schema serialization errors.
- Memory append assigns OS-random UUIDs and insertion timestamps where the write contract permits omission. Search never invents empty identifiers or epoch timestamps. Tagging by session resolves an explicitly seeded entity mapping and refuses missing mappings. B6 empty tag display/reason defaults carry statement-level CONTRACT citations. The B13 awaiting-input TTL remains the meaningful 24-hour default described by the contract, with the old manual fallback removed.
- HTTP status, base64, SQLite UTF-8, PostgreSQL numeric/boolean/timestamp and recording regex errors propagate. HTTP and WebSocket HTTP exchanges share the response comparator. JSON content types are decoded before comparison, including identical invalid bodies.
- JSON comparisons preserve null members and parsed numeric types; they do not coerce large integers through f64. Capture masks are applied to expected data only. Lone-surrogate repair and surrogate-cut equivalence were removed. No recording bytes were changed.
- The HTTP stub uses CancellationToken and a drop guard; closing it reports serving and task errors.
- The testkit replay test is split into files below 600 lines. Its synthetic request recipe is built with serde rather than interpreting a JavaScript expression. The generic case loader uses serde record types.
- `dump_rivetos` is renamed to `dump_rivethub`.

## Recording exclusions

`tests/fixtures/skipped.json` names the four config-internal synthetic cases and explains each exclusion. The replay inventory checks every entry. `capture-skipped.json` names `hook/1`, whose lone surrogate is invalid Unicode, and the test requires decoding that row to fail. The synthetic `main/deviation-server-header` case is also named in `http-skipped.json`, with a checked reason explaining its deliberate exclusion. All other testkit transport scenarios are replayed. PKI's old skip list named other crates' work and was removed together with its redundant inventory test. The policy test asserts fifteen scenarios and replays every one; PKI has no exclusions. An intermediate empty-list file triggered DUPFIXTURE against another crate's empty list, so the redundant file was removed rather than weakening the gate or disguising identical data with formatting.

Compatibility names remain only in the fixture name-translation helper, recorded fixture data, the B4 `ros_*` output keys, and imported OpenSSL layout metadata. No new legacy product identifier is introduced.

## Necessary call-site changes outside the three crates

- agent-loop tests/cases/{turns,limits,lifecycle}.rs: handle fallible FakeTool construction. The lifecycle helper stores the definition constructed by its test setup so its infallible trait method cannot panic.
- gateway tests/audio.rs: handle fallible Response::json. gateway tests/replay.rs: use plain-text diagnostic failure responses in the infallible callback and propagate status parsing errors.
- gateway tests/session_replay.rs and tests/v1/replay.rs: handle fallible status parsing.
- gateway tests/capture.rs and terminal tests/settings.rs: handle fallible mask compilation.
- tasks tests/replay.rs: handle fallible status parsing and HTTP stub shutdown.
- tasks tests/{budget,engine,workdir}.rs, capture tests/spool_and_transport.rs and cli tests/status.rs: handle fallible HTTP stub shutdown.
- memory-postgres tests/capture_pg.rs: use the renamed dump helper.
- Cargo.lock was updated by Cargo for testkit's schemars dependency, never edited by hand.

## Left for Phil / outside this slice

Crate size targets, Direct PTY semantics and exact zod issue JSON are unchanged and require Phil's decisions. The renewed instruction to finish after the scope question authorizes the minimal caller and wire fixes below. No service, database configuration, git configuration or remote was changed. Nothing was pushed.

## Workspace caller fixes needed to complete the run

The user renewed the instruction to make both commands green after the remaining owning-crate edits were identified and the scope question was raised. These additional changes address failures exposed by removing permissive fake and comparator behavior:

- mcp/tests/memory.rs seeds the session-to-entity mapping explicitly for its authorized tag-write test.
- memory-postgres/src/embed.rs changes only three test setups: each supplies the exact number of embedding responses its cache assertions require.
- wire/src/gateway/devices.rs serializes unavailable mesh session counts as null and models an optional activity report, whose typed state distinguishes no activity from an unavailable report. gateway/src/routes/surface/mesh/mod.rs constructs the local known-empty report; these are not route mount edits. wire/tests/mesh.rs verifies exact serialization/deserialization and rejects wrongly typed reports. gateway/tests/surface.rs asserts the new explicit no-activity state. The field deserializer delegates to the typed enum and propagates serde errors; it never scans or rewrites JSON. wire/src/stream.rs serializes absent OpenAI message content as null. Existing owning-crate recording tests validate complete responses/requests under strict comparison. The recorded denUrl key remains a compatibility wire key.
- capture/tests/hook_replay.rs checks a local exclusion inventory for hook/3, hook/4 and hook/5, whose recorded tool arguments rounded a u64 and changed a floating-point timeout to an integer. A native regression executes all three inputs and compares the complete posted tool arguments against parsed transcript inputs, preserving precision and numeric types.
- memory-postgres/tests/capture_pg.rs checks a local exclusion inventory for transcript ingest-twice and jsonb-key-reordering (recorded u64 rounding), transcript ai-title-upgrade and hook long-prompt (surrogate-half cuts), plus the three historical schema scenarios previously skipped implicitly. A native character-boundary regression requires intact non-BMP characters at both title and content caps. A SQLx-isolated regression verifies exact u64 and floating-point tool arguments round-trip through PostgreSQL. The oversized existing test helper is split into hook and transcript modules; the helper behavior is otherwise unchanged. Replay failures are accumulated so every remaining scenario runs before reporting errors.
- The numeric exclusions live in each owning crate, not in testkit's slice inventory. Original recording bytes are unchanged, and no comparison exception or numeric coercion is introduced.

## Validation

The lane PostgreSQL endpoint remains reachable. No service or database was started, stopped or reconfigured. Initial validation passed the gate and 74 scoped tests, while full-workspace diagnostics exposed the caller failures addressed above. Final validation is in progress.
