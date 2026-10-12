# AGENT.md — RivetHub runtime

## State (2026-10-12)

Repository created from the design in `docs/`. Workspace is empty (`crates/` has no members yet); Wave 0 (`wire`, `config`,
`kernel`, `testkit`) is the first code. `integrations/` and `distro/` carry their history from the RivetOS monorepo and still
use the `rivetos` names; they are updated to `rh` / `RIVETHUB_*` in Wave 5 with the compatibility aliases from CONTRACT
decision 16.

## How work happens

1. One crate = one lane = one builder brief (briefs live outside this repo with the recordings). The builder reads
   `docs/CONTRACT.md` row(s), the inventory section and the recording for the crate — never the TypeScript source.
2. Done means `python3 scripts/gate.py .` green, `cargo clippy -D warnings`, tests, replays, `coverage-floor.txt` not lowered,
   one Conventional Commit under a house identity, no attribution trailers.
3. Reviewer: Claude Opus, Rust idiom and quality only; boundary acceptance is mechanical.

## Gotchas

- `LAYER` in `scripts/gate.py` is the crate map. A new crate is a design change first (docs/ARCHITECTURE.md), then LAYER.
- `clippy.toml` `disallowed-methods` blocks `std::env::var` everywhere; the `config` crate reads the environment through
  `std::env::vars_os()` once.
- Privacy and secret scans use hashed denylists in `scripts/`; add a hash, never the plaintext.
