#![expect(
    clippy::redundant_pub_crate,
    reason = "private modules: rustc unreachable_pub rejects plain pub, so crate visibility is the widest reachable form"
)]

mod auth;
mod client;
mod directory;
mod engine;
mod error;
mod jobs;
mod memory;
mod origin;
mod paths;
mod payload;
mod postgres;
mod price;
mod record;
mod redirect;
mod run;
mod serve;
mod sql_job;
mod sql_task;
mod store;
mod submit;

pub use client::GatewayClient;
pub use directory::Directory;
pub use directory::LocalAgent;
pub use directory::Prepared;
pub use directory::ResolvedTarget;
pub use directory::TargetInput;
pub use engine::Engine;
pub use engine::Settings;
pub use error::ConflictAction;
pub use error::Error;
pub use jobs::ClaimSelector;
pub use jobs::ClaimedJob;
pub use jobs::JobWake;
pub use jobs::NewJob;
pub use jobs::StoredJob;
pub use jobs::TaskJobs;
pub use memory::Memory;
pub use origin::TaskOrigin;
pub use payload::TaskJobPayload;
pub use postgres::Postgres;
pub use price::TokenRate;
pub use record::Finish;
pub use record::TaskRecord;
pub use run::RunOutcome;
pub use store::TaskStore;
pub use submit::HeartbeatSpec;
pub use submit::NewTask;
