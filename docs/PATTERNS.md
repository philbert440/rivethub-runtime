# Patterns

Every entry is an anti-pattern found in review, the pattern to use instead, and the check that enforces it. A gate or clippy
finding names its entry. To add one: write the entry, add the check (clippy `disallowed-*`, `deny.toml`, or a `gate.py` rule
with this entry's code), and add a firing and a passing case to `scripts/gate_test.py`. The gate fails if a rule has no entry.

## COMMENT
Anti-pattern: comments explaining code. Pattern: names and types carry the meaning; a non-obvious constraint becomes a type
or a test. Check: gate `COMMENT`.

## ALLOW
Anti-pattern: `#[allow(...)]` / `#[expect(...)]` to silence a lint. Pattern: fix the code; if a lint is wrong for the whole
workspace, change `Cargo.toml` `[workspace.lints]` in a reviewed change. Check: gate `ALLOW`.

## UNWRAP
Anti-pattern: `.unwrap()` / `.expect()` in production code. Pattern: return the crate's `thiserror` error with `?`; a value
that cannot be absent is made non-optional by its type. Check: gate `UNWRAP`, clippy `unwrap_used`, `expect_used`.

## UNSAFE
Anti-pattern: `unsafe`. Pattern: a safe crate API. Check: gate `UNSAFE`, `unsafe_code = "forbid"`.

## JSMODEL
Anti-pattern: emulating JavaScript values or built-ins (`JsNumber`, `JsString`, UTF-16 slicing, `Date.parse` heuristics,
key-order preservation). Pattern: plain Rust types; the contract defines what is observable, and JavaScript runtime quirks are
not part of it. Check: gate `JSMODEL`.

## UNTYPED
Anti-pattern: `serde_json::Value` / `Map<String, Value>` in structs, helpers or signatures — including private fields with
accessors and deserialize helpers that return a raw map. Pattern: a serde struct or enum per payload. Two named types in
`crates/wire/src/json.rs` are the only holders of free-form JSON: `wire::Extra` for unknown fields the contract preserves
(`#[serde(flatten, default)] pub extra: Extra`), and `wire::JsonObject` for values the contract defines as arbitrary JSON
(MCP tool-call arguments, tool input schemas, provider `extra_body`). Nothing else names `serde_json::Value` or `Map`.
Check: gate `UNTYPED` (everywhere except `wire/src/json.rs`, tests and `testkit`), clippy `disallowed-types`.

## TAGFIELD
Anti-pattern: a constant discriminator stored as a field, `pub r#type: SnapshotTag` with a one-variant enum. Pattern:
`#[serde(tag = "type", rename = "snapshot")]` on the struct, or `#[serde(tag = "type")]` on the enum of variants. A `type` field whose enum has several values and whose variants share
the same fields is fine. Check: gate `TAGFIELD` (a `r#type` field typed as a one-variant enum or a `String`).

## STRINGLY
Anti-pattern: a `String` field whose values are a closed set that already has an enum (`harness: Option<String>` while
`HarnessId` exists). Pattern: use the enum. Check: gate `STRINGLY` (a field named after a workspace enum that names a closed category —
`…Id`, `…Kind`, `…Status`, `…Role`, `…Phase`, `…Mode`, `…State`, `…Source`, `…Activity`; e.g. `harness_id` or `harness` for
`HarnessId` — must not be `String`).

## JSONMAP
Anti-pattern: `serde_json::Map<String, T>` as a typed map. Pattern: `BTreeMap<K, T>` (ordered output) or `HashMap<K, T>`, with
a typed key (`SessionId`) where one exists. Check: gate `UNTYPED` (any `serde_json::Map`).

## DISPATCH
Anti-pattern: `match s.as_str() { "a" => … }` on a protocol tag or kind. Pattern: a serde enum
(`#[serde(tag = "type", rename_all = "snake_case")]`) and `match` on the variant. Check: gate `DISPATCH`.

## NAMING
Anti-pattern: `den` / `room` identifiers inherited from the TypeScript. Pattern: descriptive names (`gateway`, `session`,
`view`). Wire strings that must stay are serde renames, never Rust identifiers. Check: gate `NAMING`.

## RUNTIME
Anti-pattern: a library building a tokio runtime or using `#[tokio::main]`. Pattern: libraries expose `async fn`; only the
`rivethub` binary builds the runtime. Check: gate `RUNTIME`, clippy `disallowed-methods`.

## STDLOCK
Anti-pattern: `std::sync::Mutex` / `RwLock`. Pattern: `parking_lot` for short critical sections never held across `.await`;
`tokio::sync` when state is held across an await. Check: gate `STDLOCK`, clippy `disallowed-types`.

## PRINT
Anti-pattern: `println!` / `eprintln!` in libraries. Pattern: `tracing` events; only `cli` and `rivethub` write to stdout.
Check: gate `PRINT`, clippy `print_stdout`, `print_stderr`.

## DYNVALUE
Anti-pattern: a home-made dynamic value tree (`enum Node { Null, Bool, Int, Str, Seq, Map }`, an order-preserving
`Mapping`, a hand-written `Deserializer` for it) with validation as lookups over that tree. Pattern: deserialize straight
into typed structs and enums; validate the typed value. When a pass genuinely needs the raw tree (the `${VAR}` substitution
over string leaves before typing), use the format crate's own value type (`serde_yaml::Value`) inside that one function and
convert with `serde_yaml::from_value`; never define a value model. Check: gate `DYNVALUE`.

## ENV
Anti-pattern: reading the environment anywhere but configuration. Pattern: the binary builds `config::Environment` from
`std::env::vars_os()` once and passes `Config` down. Check: clippy `disallowed-methods` (`std::env::var`, `var_os`).

## EXIT
Anti-pattern: `std::process::exit` outside `main`. Pattern: return an error; the binary maps it to the exit code.
Check: clippy `exit`, `disallowed-methods`.

## DEPS
Anti-pattern: a crate outside the policy (OpenSSL, `anyhow`, `once_cell`, `lazy_static`, a second async runtime, tmux).
Pattern: rustls; `thiserror`; `std::sync::LazyLock`/`OnceLock`; tokio; herdr. Check: `deny.toml` bans.
