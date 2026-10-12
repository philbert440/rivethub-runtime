# AGENT.md — RivetHub runtime

## State (2026-10-12)

Repository created from the design in `docs/`. On `main`: the guardrails, and `config`, `wire`, `kernel`, `testkit` (Wave 0).
The repository is Rust only: the per-harness hook kits and the Node installer stayed in the RivetOS monorepo (LANGUAGES
allow-list). The gate is `cargo xtask check`; the design rules that need names, types or whole-program knowledge are Dylint
lints in `lints/` (run by `cargo dylint`, pinned nightly), everything that is a property of Rust code lives in
`[workspace.lints]` and `clippy.toml`, dependency direction is `deny.toml` bans with wrappers, and everything else is an
`xtask` check.

## How work happens

1. One crate = one lane = one builder brief (briefs live outside this repo with the recordings). The builder reads
   `docs/CONTRACT.md` row(s), the inventory section and the recording for the crate — never the TypeScript source.
2. Done means `cargo xtask check` green (it runs fmt, clippy `-D warnings`, `cargo dylint --all`, `cargo deny check` and the
   xtask checks), tests, replays, `coverage-floor.txt` not lowered, one Conventional Commit under a house identity, no
   attribution trailers.
3. Reviewer: Claude Opus, Rust idiom and quality only; boundary acceptance is mechanical.

## Gotchas

- The layer map lives in `deny.toml` `[bans]` deny entries with wrappers. A new crate is a design change first
  (docs/ARCHITECTURE.md), then a ban entry whose wrappers are exactly the crates allowed to depend on it.
- The lint policy is `xtask/lints-policy.toml`; `[workspace.lints]` must equal it (xtask `LINTPOLICY`). Only a reviewed
  policy change may touch either.
- Git hooks are the xtask binary, not scripts: `cargo xtask install-hooks` copies the built binary to
  `target/git-hooks/{pre-commit,commit-msg}` and sets `core.hooksPath`; it dispatches on the name it was invoked as.
- `clippy.toml` `disallowed-methods` blocks `std::env::var` everywhere; the `config` crate reads the environment through
  `std::env::vars_os()` once.
- Privacy and secret scans use hashed denylists in `xtask/data/`; add a hash, never the plaintext.
- New Dylint lint: write it in `lints/src/`, register it in `register_lints`, add the UI test (`lints/ui/<name>/` with one
  firing and one passing case), add it to `xtask/src/patterns.rs` and give it a `docs/PATTERNS.md` entry.
