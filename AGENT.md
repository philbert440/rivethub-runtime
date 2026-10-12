# AGENT.md — RivetHub runtime

## State (2026-10-12)

Repository created from the design in `docs/`. On `main`: the gate and guardrails, and `config` (Wave 0). `wire`/`kernel` are
approved on `rr/v2-foundation` (Phil reviewed the types); `testkit` is in progress. The repository is Rust only (plus shell
for CI and hooks): the per-harness hook kits and the Node installer stayed in the RivetOS monorepo (gate rule STACK), and
`scripts/gate.py` moves to `cargo xtask`.

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
