#![cfg(test)]

#[path = "v1/agents.rs"]
mod agents;
use crate::common;
#[path = "v1/lifecycle.rs"]
mod lifecycle;
#[path = "v1/replay.rs"]
mod replay;
#[path = "v1/requests.rs"]
mod requests;
#[path = "v1/support.rs"]
mod support;

#[path = "v1/runner.rs"]
mod runner;

#[path = "v1/chat.rs"]
mod chat;
