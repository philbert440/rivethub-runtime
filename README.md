# RivetHub runtime

The RivetHub runtime in Rust: gateway, sessions, memory, workers, MCP sidecar and the `rh` CLI. It is a drop-in replacement
for the TypeScript RivetOS runtime at sixteen observable boundaries and a fresh design everywhere else.

- `docs/CONTRACT.md` — the sixteen boundaries. This is the whole definition of "drop-in".
- `docs/ARCHITECTURE.md` — crates, ports, process shape, build order.
- `docs/BUILDER-RULES.md` — the one page every change is held to.
- `crates/` — the workspace (`wire`, `config`, `kernel`, `testkit`, adapters, services, `cli`, `rivethub`).
- `lints/` — the Dylint library (`cargo dylint`) carrying the design rules that need names, types or whole-program
  knowledge; outside the workspace, pinned to the nightly its `dylint_linting` version requires.
- `xtask/` — the repository tooling (`cargo xtask`): `check` (the repository gate), `install-hooks`, the languages
  allow-list, the lint policy.

## Working here

```sh
cargo xtask install-hooks        # installs the xtask binary into target/git-hooks (pre-commit, commit-msg)
cargo xtask check                # what CI runs first: fmt, clippy, dylint, cargo-deny, languages, privacy, secrets
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

The repository is Rust only. The git hooks are the `xtask` binary itself: `cargo xtask install-hooks` copies it to
`target/git-hooks/pre-commit` and `target/git-hooks/commit-msg` and points `core.hooksPath` there; the binary dispatches on
the name it is invoked as (pre-commit: staged privacy + fmt; commit-msg: house identity, no attribution trailers).
The runtime ships one generic capture entry point, `rh capture hook`, built on the `capture` crate's writer, spool and
redaction; the per-harness hook kits (install scripts and hook configuration) live in the client repositories and call
it. Other clients of the runtime live in their own repositories too and reach the runtime over its boundaries.

Every crate depends only downward (foundation ← kernel ← adapters/services ← composition); `deny.toml` fails the build the
moment a `Cargo.toml` breaks that (each internal crate is a ban whose wrappers are exactly the crates allowed to use it).
Production code has no comments, no `allow`, no `unwrap`/`expect`, no `unsafe`; `clippy.toml` disallows std locks, untyped
JSON, runtime construction and environment reads outside their one home, and `lints/` carries the rest of
`docs/PATTERNS.md` as Dylint lints. `deny.toml` keeps TLS on rustls and bans `anyhow`, OpenSSL and a second async runtime.

Licensed under Apache-2.0.
