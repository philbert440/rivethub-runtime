#![expect(
    unreachable_pub,
    reason = "the harness is included as a private module in every test binary"
)]
#![expect(
    dead_code,
    reason = "every test binary includes this module and uses a different subset"
)]

use std::net::SocketAddr;
use std::sync::Arc;

use axum::http::HeaderName;
use axum::http::HeaderValue;
use axum::http::Method;
use axum::http::Request;
use axum::http::Uri;
use gateway::Gateway;
use gateway::Settings;
use kernel::memory::MemoryStore;
use kernel::memory::MemoryStoreError;
use testkit::FakeMemoryStore;
use testkit::Request as Recorded;
use testkit::Response;
use tower::ServiceExt;

const HOSTNAME: &str = "gold-host";
const NODE: &str = "golden-node";

pub fn settings() -> Settings {
    Settings {
        hostname: HOSTNAME.to_string(),
        node: NODE.to_string(),
        allowed_origins: vec!["https://hub.example.test".to_string()],
        allowed_hosts: vec!["den.example.test".to_string()],
    }
}

pub fn gateway() -> (Arc<Gateway>, Arc<FakeMemoryStore>) {
    let store = Arc::new(FakeMemoryStore::default());
    let handle: Arc<dyn MemoryStore> = Arc::clone(&store) as Arc<dyn MemoryStore>;
    let gateway = Arc::new(Gateway::new(handle, settings()));
    (gateway, store)
}

pub fn failing_gateway() -> Arc<Gateway> {
    let handle: Arc<dyn MemoryStore> = Arc::new(FailingStore);
    Arc::new(Gateway::new(handle, settings()))
}

pub struct FailingStore;

fn unavailable<T>() -> Result<T, MemoryStoreError> {
    Err(MemoryStoreError::Unavailable("down".into()))
}

#[async_trait::async_trait]
impl MemoryStore for FailingStore {
    async fn append(
        &self,
        _batch: Vec<wire::memory::MemoryEntry>,
    ) -> Result<u64, MemoryStoreError> {
        unavailable()
    }

    async fn search(
        &self,
        _query: kernel::memory::SearchQuery,
    ) -> Result<Vec<wire::memory::MemorySearchResult>, MemoryStoreError> {
        unavailable()
    }

    async fn browse(
        &self,
        _filter: kernel::memory::BrowseFilter,
    ) -> Result<Vec<wire::memory::MemoryEntry>, MemoryStoreError> {
        unavailable()
    }

    async fn stats(&self) -> Result<wire::gateway::memory::MemoryStatsResponse, MemoryStoreError> {
        unavailable()
    }

    async fn get_full(
        &self,
        _id: &str,
    ) -> Result<Option<wire::memory::MemoryEntry>, MemoryStoreError> {
        unavailable()
    }

    async fn context(
        &self,
        _session: &str,
        _now: chrono::DateTime<chrono::Utc>,
    ) -> Result<String, MemoryStoreError> {
        unavailable()
    }

    async fn list_tags(
        &self,
        _query: kernel::memory::TagListQuery,
    ) -> Result<Vec<wire::gateway::memory::TagWire>, MemoryStoreError> {
        unavailable()
    }

    async fn pending_tags(
        &self,
        _limit: kernel::memory::Limit,
    ) -> Result<Vec<wire::gateway::memory::PendingTagWire>, MemoryStoreError> {
        unavailable()
    }

    async fn tag_counts(
        &self,
        _query: kernel::memory::TagCountsQuery,
    ) -> Result<Vec<wire::gateway::memory::TagCount>, MemoryStoreError> {
        unavailable()
    }

    async fn list_tag_taxonomy(
        &self,
        _query: kernel::memory::TagTaxonomyQuery,
    ) -> Result<Vec<wire::gateway::memory::TagTaxonomyEntry>, MemoryStoreError> {
        unavailable()
    }

    async fn decide_tags(
        &self,
        _ids: Vec<kernel::memory::TagId>,
        _decision: kernel::memory::TagDecision,
        _decided_by: Option<String>,
    ) -> Result<u64, MemoryStoreError> {
        unavailable()
    }

    async fn decide_tag_removals(
        &self,
        _ids: Vec<kernel::memory::TagId>,
        _decision: kernel::memory::TagDecision,
        _decided_by: Option<String>,
    ) -> Result<u64, MemoryStoreError> {
        unavailable()
    }

    async fn add_tag(
        &self,
        _add: kernel::memory::TagAdd,
    ) -> Result<wire::gateway::memory::TagWire, MemoryStoreError> {
        unavailable()
    }

    async fn lookup_tags(
        &self,
        _lookup: kernel::memory::TagLookup,
    ) -> Result<
        std::collections::BTreeMap<String, Vec<wire::gateway::memory::TagWire>>,
        MemoryStoreError,
    > {
        unavailable()
    }

    async fn upsert_tag_taxonomy(
        &self,
        _upsert: kernel::memory::TagTaxonomyUpsert,
    ) -> Result<wire::gateway::memory::TagTaxonomyEntry, MemoryStoreError> {
        unavailable()
    }

    async fn decide_tag_taxonomy(
        &self,
        _decision: kernel::memory::TagTaxonomyDecision,
    ) -> Result<u64, MemoryStoreError> {
        unavailable()
    }

    async fn merge_tag_taxonomy(
        &self,
        _merge: wire::gateway::memory::TagTaxonomyMerge,
    ) -> Result<wire::gateway::memory::TagTaxonomyMergeResult, MemoryStoreError> {
        unavailable()
    }
}

fn remote_addr(request: &Recorded) -> Result<SocketAddr, String> {
    let via = request.via.as_deref().unwrap_or_default();
    if via.contains("remote") || via.contains("127.0.0.2") {
        "127.0.0.2:41000"
            .parse()
            .map_err(|error| format!("remote test address: {error}"))
    } else {
        "127.0.0.1:41000"
            .parse()
            .map_err(|error| format!("loopback test address: {error}"))
    }
}

pub async fn answer(app: axum::Router, request: Recorded) -> Result<Response, String> {
    let addr = remote_addr(&request)?;
    let uri: Uri = request
        .target
        .parse()
        .map_err(|error| format!("recorded target {}: {error}", request.target))?;
    let method = Method::from_bytes(request.method.as_bytes())
        .map_err(|error| format!("recorded method: {error}"))?;
    let mut builder = Request::builder().method(method).uri(uri);
    for (name, value) in &request.headers {
        let parsed_name = HeaderName::from_bytes(name.to_ascii_lowercase().as_bytes())
            .map_err(|error| format!("header name {name}: {error}"))?;
        let parsed_value = HeaderValue::from_bytes(value.as_bytes())
            .map_err(|error| format!("header value {value}: {error}"))?;
        builder = builder.header(parsed_name, parsed_value);
    }
    let mut request = builder
        .body(axum::body::Body::from(request.body))
        .map_err(|error| format!("recorded request: {error}"))?;
    request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(addr));
    let response = app
        .oneshot(request)
        .await
        .map_err(|error| format!("in-process service: {error}"))?;
    let status = response.status().as_u16();
    let headers = response
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_string(),
                String::from_utf8_lossy(value.as_bytes()).to_string(),
            )
        })
        .collect();
    let body = axum::body::to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .map_err(|error| format!("response body: {error}"))?
        .to_vec();
    Ok(Response {
        status,
        headers,
        body,
    })
}
