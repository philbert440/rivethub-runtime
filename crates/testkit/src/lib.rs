pub mod capture;
pub mod capture_pg;
pub mod cases;
pub mod fakes;
pub mod http;
pub mod json;
pub mod memory;
pub mod paths;
pub mod pgdump;
pub mod provider;
pub mod sqlite;
pub mod stub;
pub mod ws;

use std::path::Path;
use std::path::PathBuf;

pub use capture::HookFile;
pub use capture::HookOutcome;
pub use capture::HookPost;
pub use capture_pg::PgOutcome;
pub use capture_pg::PgStep;
pub use cases::Thrown;
pub use fakes::FakeHarness;
pub use fakes::FakeJobQueue;
pub use fakes::FakeMux;
pub use fakes::FakeProvider;
pub use fakes::FakeReader;
pub use fakes::FakeTool;
pub use fakes::Script;
pub use http::Handshake;
pub use http::Request;
pub use http::Response;
pub use json::Json;
pub use json::Masks;
pub use json::json_eq;
pub use json::json_masked_eq;
pub use json::masked_eq;
pub use json::substitute;
pub use memory::FakeMemoryStore;
pub use memory::MemoryRequest;
pub use paths::golden_path;
pub use paths::recording;
pub use pgdump::PgNamer;
pub use pgdump::capture_tx;
pub use pgdump::dump_rivetos;
pub use pgdump::queue_present;
pub use provider::Canned;
pub use provider::CapturedRequest;
pub use provider::ProviderTurn;
pub use sqlite::FixtureDb;
pub use sqlite::open_fixture;
pub use sqlite::query_json;
pub use sqlite::rows_eq;
pub use sqlite::sqlite_version;
pub use stub::Stub;
pub use stub::stub_server;
pub use ws::Frame;
pub use ws::WsCall;
pub use ws::WsReply;
pub use ws::WsRequest;
pub use ws::WsSent;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mismatch {
    pub id: String,
    pub detail: String,
}

impl Mismatch {
    pub fn new(id: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            detail: detail.into(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("recording {}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("recording {}: {message}", path.display())]
    Format { path: PathBuf, message: String },
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

impl Error {
    pub(crate) fn io(path: &Path, source: std::io::Error) -> Self {
        Self::Io {
            path: path.to_path_buf(),
            source,
        }
    }

    pub(crate) fn format(path: &Path, message: impl Into<String>) -> Self {
        Self::Format {
            path: path.to_path_buf(),
            message: message.into(),
        }
    }
}

pub(crate) fn read_text(path: &Path) -> Result<String, Error> {
    std::fs::read_to_string(path).map_err(|source| Error::io(path, source))
}

pub(crate) fn parse_json<T>(path: &Path, text: &str) -> Result<T, Error>
where
    T: serde::de::DeserializeOwned,
{
    serde_json::from_str(text).map_err(|error| Error::format(path, error.to_string()))
}
