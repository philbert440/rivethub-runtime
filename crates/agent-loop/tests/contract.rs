#![cfg(test)]

pub mod support;

#[path = "cases/hooks.rs"]
mod hooks;
#[path = "cases/lifecycle.rs"]
mod lifecycle;
#[path = "cases/limits.rs"]
mod limits;
#[path = "cases/replay.rs"]
mod replay;
#[path = "cases/stream.rs"]
mod stream;
#[path = "cases/turns.rs"]
mod turns;
