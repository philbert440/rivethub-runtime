#![cfg(test)]

#[path = "v1/agents.rs"]
mod agents;
#[path = "common/mod.rs"]
#[expect(
    dead_code,
    reason = "the harness module is included whole by every test binary and this one uses a subset"
)]
mod common;
#[path = "v1/lifecycle.rs"]
mod lifecycle;
#[path = "v1/replay.rs"]
mod replay;
#[path = "v1/requests.rs"]
mod requests;
#[path = "v1/support.rs"]
pub mod support;

#[path = "v1/runner.rs"]
pub mod runner;

#[path = "v1/chat.rs"]
mod chat;
