# RivetHub runtime

The RivetHub runtime in Rust: gateway, sessions, memory, workers, MCP sidecar and the `rh` CLI. It is a drop-in replacement
for the TypeScript RivetOS runtime at sixteen observable boundaries and a fresh design everywhere else.

- `docs/CONTRACT.md` — the sixteen boundaries. This is the whole definition of "drop-in".
- `docs/ARCHITECTURE.md` — crates, ports, process shape, build order.
- `docs/BUILDER-RULES.md` — the one page every change is held to.
- `crates/` — the workspace (`wire`, `config`, `kernel`, `testkit`, adapters, services, `cli`, `rivethub`).
- `integrations/` — per-harness hook kits that capture sessions into memory (boundary B3).
- `distro/` — installer, systemd units, pins.

## Working here

```sh
scripts/install-hooks.sh          # pre-commit: gate + fmt; commit-msg: house identity, no attribution trailers
python3 scripts/gate.py .         # what CI runs first: crate map, dependency direction, forbidden patterns, privacy, secrets
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace
```

Every crate depends only downward (foundation ← kernel ← adapters/services ← composition); the gate fails a build the moment a
`Cargo.toml` breaks that. Production code has no comments, no `allow`, no `unwrap`/`expect`, no `unsafe`; `clippy.toml`
disallows std locks, untyped JSON, runtime construction and environment reads outside their one home. `deny.toml` keeps TLS
on rustls and errors on `thiserror`.

Licensed under Apache-2.0.
