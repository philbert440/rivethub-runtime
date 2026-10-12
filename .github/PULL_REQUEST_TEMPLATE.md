## Contract rows served

<!-- B-numbers from docs/CONTRACT.md. A change that serves no row is a bug fix against a Rust-introduced defect or a rules change; say which. -->

## Evidence

- Replays: <!-- which recordings ran, result -->
- Gate: <!-- paste the final GATE line -->
- Size: <!-- crate lines vs target in docs/ARCHITECTURE.md -->
- Coverage: <!-- line % ; coverage-floor.txt raised if it went up -->

## Checklist

- [ ] Behaviour at the boundary only; nothing of the TypeScript's structure ported
- [ ] No comments, no `allow`, no `unwrap`/`expect`, no `unsafe` in production code
- [ ] Depends only downward (gate DEP clean)
- [ ] One Conventional Commit, house identity, no AI attribution trailers
