# Golden deviations — crates/config

The replay in `tests/validate/` drives every recorded case in
`tests/golden/fixtures/validate_config*.jsonl` through `config::validate_document`
as one test per recorded case. Proof frame:

- A case is WELL-TYPED when `serde_path_to_error::deserialize` of the document
  into the typed `Config` succeeds. Well-typed cases must match the recorded
  findings exactly, compared as a sorted multiset of (severity, path, message).
  The CONTRACT §3 decisions (11, 12/20, 13, 15) are applied to the recorded
  side first — they are boundary behaviour, not deviations.
- A TYPE-CONFUSED case (typed deserialization fails) produces exactly one error
  finding at serde's path, message `"<path>" has the wrong type: <serde
  message>`. The replay asserts only that the recorded outcome also had at
  least one error.
- `format_validation_result`, `resolve_env_vars`, `shared_dir`, `shared_path`,
  `install_root` and `install_path` replays are exact and carry no deviations.

The deviations below are the cases where the recording's expected findings are
not reproduced verbatim. They are pinned as data in
`tests/golden/deviations.jsonl` (one row per case: id, class, and the expected
findings that replace the recorded ones); `deviations_are_documented` asserts
the per-file counts against this document. 5,825 of 6,008 validate cases match
the recording exactly (after the decision replacements); the remaining cases
fall into the four classes below.

## Accepted-but-rejected (115 cases)

TS validated provider/plugin-side config keys only for presence and shape of
the section; it never type-checked individual values (plugin `ctx.pluginConfig`
is read loosely). The Rust typed model gives every key an honest type, so a
wrong-typed value under such a key now fails deserialization and the whole
document is type-confused. CONTRACT §2 puts these inputs outside the boundary
(inputs clients never send). Five examples per file:

- `validate_config.jsonl` — 2 accepted-but-rejected; ids: validate_config.jsonl:125, validate_config.jsonl:384
- `validate_config_agents_providers.jsonl` — 17 accepted-but-rejected; ids: validate_config_agents_providers.jsonl:61, validate_config_agents_providers.jsonl:86, validate_config_agents_providers.jsonl:166, validate_config_agents_providers.jsonl:235, validate_config_agents_providers.jsonl:249
- `validate_config_den.jsonl` — 2 accepted-but-rejected; ids: validate_config_den.jsonl:164, validate_config_den.jsonl:753
- `validate_config_memory.jsonl` — 28 accepted-but-rejected; ids: validate_config_memory.jsonl:58, validate_config_memory.jsonl:94, validate_config_memory.jsonl:201, validate_config_memory.jsonl:245, validate_config_memory.jsonl:291
- `validate_config_mesh_channels_misc.jsonl` — 24 accepted-but-rejected; ids: validate_config_mesh_channels_misc.jsonl:9, validate_config_mesh_channels_misc.jsonl:18, validate_config_mesh_channels_misc.jsonl:57, validate_config_mesh_channels_misc.jsonl:60, validate_config_mesh_channels_misc.jsonl:135
- `validate_config_runtime.jsonl` — 36 accepted-but-rejected; ids: validate_config_runtime.jsonl:2, validate_config_runtime.jsonl:24, validate_config_runtime.jsonl:44, validate_config_runtime.jsonl:51, validate_config_runtime.jsonl:85
- `validate_config_tasks_workflows.jsonl` — 6 accepted-but-rejected; ids: validate_config_tasks_workflows.jsonl:37, validate_config_tasks_workflows.jsonl:179, validate_config_tasks_workflows.jsonl:335, validate_config_tasks_workflows.jsonl:395, validate_config_tasks_workflows.jsonl:412

`validate_config_untyped.jsonl` has none. Examples of the class: a string
`runtime.plugin_dirs` (TS never type-checked it), a list `providers.claude-cli.binary`,
a string under `memory.sqlite.workers`. Every affected case carries at least
one typed Rust error; none loses a recorded warning without a replacement.

## Order-only "Available" lists (58 cases)

The `Provider … referenced by agent … is not defined` / `Default agent … is not
defined in [agents]` / `Heartbeat agent …` messages embed the agent or provider
names in document order. The typed model holds `agents` and `providers` as
`BTreeMap`, so the list is sorted; CONTRACT §2 excludes key order from the
boundary (the only order-sensitive boundary artefacts are capture event ids).
Paths, severities and the name multisets match; only the in-message order
differs. Five examples per file:

- `validate_config.jsonl` — 13 order-only Available lists; ids: validate_config.jsonl:31, validate_config.jsonl:65, validate_config.jsonl:88, validate_config.jsonl:336, validate_config.jsonl:339
- `validate_config_agents_providers.jsonl` — 25 order-only Available lists; ids: validate_config_agents_providers.jsonl:71, validate_config_agents_providers.jsonl:97, validate_config_agents_providers.jsonl:114, validate_config_agents_providers.jsonl:126, validate_config_agents_providers.jsonl:175
- `validate_config_runtime.jsonl` — 20 order-only Available lists; ids: validate_config_runtime.jsonl:54, validate_config_runtime.jsonl:68, validate_config_runtime.jsonl:143, validate_config_runtime.jsonl:209, validate_config_runtime.jsonl:211

## Sequence-shaped sections and entries (3 cases)

serde's derived structs accept the sequence form, so a section written as a
YAML list (`agents.local: []`, `providers.reasoning_effort: []`) deserializes
into the all-default entry instead of failing with the recorded
`… must be an object` error. The table pins the actual expected findings for
each such case. Examples:

- `validate_config_agents_providers.jsonl` — 3 sequence-shaped sections or entries; ids: validate_config_agents_providers.jsonl:47, validate_config_agents_providers.jsonl:81, validate_config_agents_providers.jsonl:657

## Explicit nulls read as missing (7 cases)

Absent and explicit `null` are the same "missing" on every typed field
(NULLABLE: no `Option<Option<T>>`). serde_yaml goes one step further
for the plain container shapes: an explicit null deserializes as the empty
sequence (`runtime.heartbeats`, `memory.capture.redaction.patterns`,
`den.allowed_origins`) or the all-default section (`den.devices`,
`memory.capture`), so the document stays well-typed and the recorded
must-be-array / must-be-an-object / must-be-a-list error is not reproduced.
The table pins the actual expected findings for each such case: recorded error
findings whose input path holds an explicit `null`, while the Rust side is
silent. Examples:

- `validate_config_den.jsonl` — 3 explicit nulls read as missing; ids: validate_config_den.jsonl:131, validate_config_den.jsonl:466, validate_config_den.jsonl:740
- `validate_config_memory.jsonl` — 3 explicit nulls read as missing; ids: validate_config_memory.jsonl:234, validate_config_memory.jsonl:337, validate_config_memory.jsonl:468
- `validate_config_runtime.jsonl` — 1 explicit nulls read as missing; ids: validate_config_runtime.jsonl:599

## Decision 15 — capture redaction is a real setting

- Decision: `memory.capture.redaction.{enabled,builtins,patterns}` is honoured
  by the hook-side capture writer (default off), so the TypeScript warning that
  the block is "validated but not yet injected into harness hook processes" no
  longer applies and is never emitted.
- Old: the warning above under `memory.capture.redaction.enabled` whenever
  `enabled: true`. New: the warning is absent; the setting is passed through
  and reachable as `Config::capture_redaction()`.
- The replay drops every recorded instance of the warning before comparing
  (a uniform contract adjustment, applied to every case).

## Decision 13 — workflows left the runtime

- Decision: the separate `rivethub-workflows` package drives workflows over the
  public API, so a top-level `workflows:` section is accepted and ignored with
  one warning for ANY value present; the recorded per-key validation
  (`"workflows" must be an object`, `"workflows.enabled" must be a boolean`,
  `Unknown workflows key "…"`, and so on) is not reproduced.
- The replay replaces every recorded finding under `workflows` with the single
  warning `"workflows" is ignored — workflows moved out of the runtime into the
  rivethub-workflows package.` whenever the key is present.

## Decision 12/20 — the plugins list is accepted and ignored

- Decision: the runtime loads no plugins, so a top-level `plugins:` list is
  accepted and ignored with one warning for ANY value present; the recorded
  shape validation (`"plugins" must be an array of npm package names`,
  `Each plugins entry must be a non-empty package name string`,
  `Duplicate plugin entry "…"`) is not reproduced. `plugins/tools/*` entries
  get a deprecation notice at load time (one per entry, `Loaded::notices`).
- The replay replaces every recorded finding under `plugins` with the single
  warning `"plugins" is ignored — the Rust runtime loads no plugins.` whenever
  the key is present.

## Decision 11 — provider types the runtime does not build

- Decision: a provider whose type is `anthropic`, `google`, `gemini` or `xai`
  — given by `type` or implied by the provider id — produces exactly one error
  naming the harness to reach those models through instead (`claude-code` /
  `gemini` / `grok-build`): `Provider "<id>" type "<type>" is not built — reach
  <type> models through the <harness> harness instead.` The id `openai-compat`
  keeps its recorded split message. Recorded per-entry findings (model
  required, token-command family, unknown-key warnings) are not reproduced.
- The replay drops every recorded finding under `providers.<id>` and adds the
  decision error. Recordings never carry a `type:` key, so the id decides.

## Decision 9 — no embedded Postgres (PGlite)

- Decision: `memory.postgres.embedded` selects no transport any more; the
  memory backend resolves to `sqlite` and the one-time Postgres import runs at
  first start. `memory.backend` (`postgres` | `sqlite`) is new and never
  appears in the recordings, so no recorded case diverges; the decision is
  asserted in `tests/decisions.rs`. An unknown `memory.backend` value is a
  type-confused document (serde unknown variant).

## Decisions that add behaviour the recordings cannot reach

Each carries a table test in `tests/decisions.rs` naming the decision:

- Decision 16: `RIVETHUB_*` variables are preferred over their `RIVETOS_*`
  equivalents, with a deprecation notice naming both; the config probe prefers
  `~/.rivethub/config.yaml` before `~/.rivetos/config.yaml` (legacy flag).
- Decision 9: the new `memory.backend` key accepts `postgres` and `sqlite`.
- Decision 10: `RIVETHUB_MCP_ENABLE_{SHELL,FILE,SEARCH}` and their `RIVETOS_`
  spellings are accepted, ignored, and reported as one deprecation warning
  each; `plugins/tools/*` entries in the `plugins:` list get a load-time
  deprecation notice each.
- Decision 2: `RIVETHUB_TERM_MUX=tmux` (or the `RIVETOS_` spelling) warns and
  resolves as unset; `herdr` stays the only multiplexer value. The
  `…_INJECT_*` and `…_TMUX_USER_CONF` variables are accepted, ignored, and
  warned.

`tests/decisions.rs` also pins the inventory's documented defaults that the
typed config exposes (`den.port` 5174, `memory.postgres.embedded.port` 5433,
`auto_migrate` true, `max_connections` 96) and verbatim messages the
recordings do not reach (blank `den.host`, `den.port: 0`, blank
`den.allowed_origins` entries, `runtime.turn_timeout: 0`, shell-unsafe
`mesh.advertise_host`, empty/ReDoS/invalid redaction patterns, claude-cli
`permission_prompts` / `permission_timeout_ms`).

## Diagnostic-text allowance (CONTRACT §3)

`Invalid regex: <diagnostic>` findings (capture redaction patterns) keep the
TypeScript prefix verbatim; the diagnostic suffix comes from `regress` (the
plan's ECMAScript engine). CONTRACT §3 allows engine diagnostic text to differ
after the TypeScript prefix.

## Root-shape and wrong-type messages

`Config must be a YAML object (got …)` and the per-field `… must be a string`
/ `… must be an array` shape messages are replaced, per the proof frame, by
the single typed error `"<path>" has the wrong type: <serde message>` for
type-confused documents. Required-section and required-field missing messages
(`Missing required section "runtime"`, `Missing required field
"runtime.workspace"`, `"memory.sqlite.path" is required …`) remain verbatim
because absent keys deserialize to `Option::None` on the typed fields.
