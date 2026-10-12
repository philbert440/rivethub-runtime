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
Anti-pattern: `serde_json::Value` / `Map<String, Value>` carried in structs and passed around. Pattern: a serde struct or
enum per payload. Unknown fields the contract says to preserve go in `wire::Extra` (`#[serde(flatten, default)] pub extra:
Extra`), the one sanctioned holder. Check: gate `UNTYPED`, clippy `disallowed-types` (`wire` and `testkit` are exempt by
their own `clippy.toml`).

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

## ENV
Anti-pattern: reading the environment anywhere but configuration. Pattern: the binary builds `config::Environment` from
`std::env::vars_os()` once and passes `Config` down. Check: clippy `disallowed-methods` (`std::env::var`, `var_os`).

## EXIT
Anti-pattern: `std::process::exit` outside `main`. Pattern: return an error; the binary maps it to the exit code.
Check: clippy `exit`, `disallowed-methods`.

## DEPS
Anti-pattern: a crate outside the policy (OpenSSL, `anyhow`, `once_cell`, `lazy_static`, a second async runtime, tmux).
Pattern: rustls; `thiserror`; `std::sync::LazyLock`/`OnceLock`; tokio; herdr. Check: `deny.toml` bans.
