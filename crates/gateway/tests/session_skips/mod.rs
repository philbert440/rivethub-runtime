#![expect(
    unreachable_pub,
    reason = "recording manifests are shared by replay tests"
)]
mod events;
mod http;
mod term;
pub use events::EVENTS;
pub use http::HTTP;
pub use term::TERM;
