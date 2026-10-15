use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::http::HeaderName;
use axum::http::HeaderValue;
use axum::http::Method;
use axum::http::Request;
use axum::http::Uri;
use gateway::Gateway;
use gateway::Settings;
use kernel::memory::BrowseFilter;
use kernel::memory::Limit;
use kernel::memory::MemoryStore;
use kernel::memory::MemoryStoreError;
use kernel::memory::SearchQuery;
use kernel::memory::TagAdd;
use kernel::memory::TagCountsQuery;
use kernel::memory::TagDecision;
use kernel::memory::TagId;
use kernel::memory::TagListQuery;
use kernel::memory::TagLookup;
use kernel::memory::TagTaxonomyDecision;
use kernel::memory::TagTaxonomyQuery;
use kernel::memory::TagTaxonomyUpsert;
use testkit::FakeMemoryStore;
use testkit::Request as Recorded;
use testkit::Response;
use tokio::sync::Semaphore;
use tokio::sync::watch;
use tower::ServiceExt;

pub(crate) mod roster;

const HOSTNAME: &str = "gold-host";
const NODE: &str = "golden-node";

pub(crate) fn settings() -> Settings {
    let scratch = std::path::PathBuf::from("/tmp/rr-gateway-unused-agents");
    Settings {
        hostname: HOSTNAME.to_string(),
        node: NODE.to_string(),
        allowed_origins: vec!["https://hub.example.test".to_string()],
        allowed_hosts: vec!["den.example.test".to_string()],
        files_root: None,
        static_dir: None,
        root_redirect: None,
        state_dir: scratch.join("state"),
        directory_root: scratch.join("agents"),
        mesh_file: Some(PathBuf::from("/no-such-gateway-mesh/mesh.json")),
        shared_dir: scratch.join("shared"),
        legacy_mesh_dir: PathBuf::from("/no-such-gateway-legacy"),
        allowed_harnesses: None,
        devices_enabled: false,
        probe_timeout: Duration::from_millis(200),
        mesh_cache_ttl: Duration::ZERO,
        mesh_ca: None,
    }
}

pub(crate) fn gateway_with_files(root: std::path::PathBuf) -> Arc<Gateway> {
    let mut settings = settings();
    settings.files_root = Some(root);
    let store = Arc::new(FakeMemoryStore::default());
    let handle: Arc<dyn MemoryStore> = store;
    Arc::new(Gateway::new(handle, settings))
}

pub(crate) fn gateway() -> (Arc<Gateway>, Arc<FakeMemoryStore>) {
    let store = Arc::new(FakeMemoryStore::default());
    let handle: Arc<dyn MemoryStore> = Arc::clone(&store) as Arc<dyn MemoryStore>;
    let gateway = Arc::new(Gateway::new(handle, settings()));
    (gateway, store)
}

pub(crate) fn spawn_healthz() -> Result<u16, FailedHealthz> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))
        .map_err(|error| FailedHealthz(format!("healthz bind: {error}")))?;
    let port = listener
        .local_addr()
        .map_err(|error| FailedHealthz(format!("healthz addr: {error}")))?
        .port();
    std::thread::spawn(move || answer_healthz(&listener));
    Ok(port)
}

fn answer_healthz(listener: &std::net::TcpListener) {
    for _ in 0..16 {
        let Ok((mut stream, _)) = listener.accept() else {
            break;
        };
        let mut buf = [0_u8; 2048];
        if std::io::Read::read(&mut stream, &mut buf).is_err() {
            continue;
        }
        let body = br#"{"ok":true,"sessions":0,"name":"gold-host","node":"golden-node"}"#;
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        if std::io::Write::write_all(&mut stream, head.as_bytes()).is_err() {
            continue;
        }
        if std::io::Write::write_all(&mut stream, body).is_err() {
            break;
        }
    }
}

#[derive(Debug)]
pub(crate) struct FailedHealthz(String);

impl std::fmt::Display for FailedHealthz {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

pub(crate) fn gateway_with(settings: Settings) -> Arc<Gateway> {
    let store = Arc::new(FakeMemoryStore::default());
    let handle: Arc<dyn MemoryStore> = Arc::clone(&store) as Arc<dyn MemoryStore>;
    Arc::new(Gateway::new(handle, settings))
}

pub(crate) fn failing_gateway() -> Arc<Gateway> {
    let handle: Arc<dyn MemoryStore> = Arc::new(FailingStore);
    Arc::new(Gateway::new(handle, settings()))
}

pub(crate) struct FailingStore;

fn unavailable<T>() -> Result<T, MemoryStoreError> {
    Err(MemoryStoreError::Unavailable("down".into()))
}

#[async_trait::async_trait]
impl MemoryStore for FailingStore {
    async fn wiki_topics(
        &self,
        _filter: kernel::wiki::WikiFilter,
    ) -> Result<wire::gateway::wiki::WikiTopicList, MemoryStoreError> {
        unavailable()
    }

    async fn wiki_page(
        &self,
        _slug: &wire::gateway::wiki::WikiSlug,
    ) -> Result<kernel::wiki::WikiLookup, MemoryStoreError> {
        unavailable()
    }

    async fn wiki_gaps(
        &self,
        _limit: u32,
    ) -> Result<wire::gateway::wiki::WikiGapsResponse, MemoryStoreError> {
        unavailable()
    }

    async fn append(
        &self,
        _batch: Vec<wire::memory::MemoryEntry>,
    ) -> Result<u64, MemoryStoreError> {
        unavailable()
    }

    async fn search(
        &self,
        _query: kernel::memory::SearchQuery,
    ) -> Result<kernel::memory::SearchOutcome, MemoryStoreError> {
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

    async fn tag_job_context(
        &self,
        _summary_id: uuid::Uuid,
    ) -> Result<Option<kernel::memory::TagJobContext>, MemoryStoreError> {
        unavailable()
    }

    async fn store_tag_suggestions(
        &self,
        _write: kernel::memory::TagSuggestionWrite,
    ) -> Result<kernel::memory::TagSuggestionStored, MemoryStoreError> {
        unavailable()
    }
}

pub(crate) struct GatedStore {
    inner: FakeMemoryStore,
    pub(crate) entered: watch::Sender<u64>,
    pub(crate) release: Semaphore,
}

impl Default for GatedStore {
    fn default() -> Self {
        Self {
            inner: FakeMemoryStore::default(),
            entered: watch::channel(0).0,
            release: Semaphore::new(0),
        }
    }
}

#[async_trait::async_trait]
impl MemoryStore for GatedStore {
    async fn wiki_topics(
        &self,
        filter: kernel::wiki::WikiFilter,
    ) -> Result<wire::gateway::wiki::WikiTopicList, MemoryStoreError> {
        self.inner.wiki_topics(filter).await
    }

    async fn wiki_page(
        &self,
        slug: &wire::gateway::wiki::WikiSlug,
    ) -> Result<kernel::wiki::WikiLookup, MemoryStoreError> {
        self.inner.wiki_page(slug).await
    }

    async fn wiki_gaps(
        &self,
        limit: u32,
    ) -> Result<wire::gateway::wiki::WikiGapsResponse, MemoryStoreError> {
        self.inner.wiki_gaps(limit).await
    }

    async fn append(&self, batch: Vec<wire::memory::MemoryEntry>) -> Result<u64, MemoryStoreError> {
        self.inner.append(batch).await
    }

    async fn search(
        &self,
        query: SearchQuery,
    ) -> Result<kernel::memory::SearchOutcome, MemoryStoreError> {
        self.entered.send_modify(|count| *count += 1);
        drop(self.release.acquire().await);
        self.inner.search(query).await
    }

    async fn browse(
        &self,
        filter: BrowseFilter,
    ) -> Result<Vec<wire::memory::MemoryEntry>, MemoryStoreError> {
        self.inner.browse(filter).await
    }

    async fn stats(&self) -> Result<wire::gateway::memory::MemoryStatsResponse, MemoryStoreError> {
        self.inner.stats().await
    }

    async fn get_full(
        &self,
        id: &str,
    ) -> Result<Option<wire::memory::MemoryEntry>, MemoryStoreError> {
        self.inner.get_full(id).await
    }

    async fn context(
        &self,
        session: &str,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<String, MemoryStoreError> {
        self.inner.context(session, now).await
    }

    async fn list_tags(
        &self,
        query: TagListQuery,
    ) -> Result<Vec<wire::gateway::memory::TagWire>, MemoryStoreError> {
        self.inner.list_tags(query).await
    }

    async fn pending_tags(
        &self,
        limit: Limit,
    ) -> Result<Vec<wire::gateway::memory::PendingTagWire>, MemoryStoreError> {
        self.inner.pending_tags(limit).await
    }

    async fn tag_counts(
        &self,
        query: TagCountsQuery,
    ) -> Result<Vec<wire::gateway::memory::TagCount>, MemoryStoreError> {
        self.inner.tag_counts(query).await
    }

    async fn list_tag_taxonomy(
        &self,
        query: TagTaxonomyQuery,
    ) -> Result<Vec<wire::gateway::memory::TagTaxonomyEntry>, MemoryStoreError> {
        self.inner.list_tag_taxonomy(query).await
    }

    async fn decide_tags(
        &self,
        ids: Vec<TagId>,
        decision: TagDecision,
        decided_by: Option<String>,
    ) -> Result<u64, MemoryStoreError> {
        self.inner.decide_tags(ids, decision, decided_by).await
    }

    async fn decide_tag_removals(
        &self,
        ids: Vec<TagId>,
        decision: TagDecision,
        decided_by: Option<String>,
    ) -> Result<u64, MemoryStoreError> {
        self.inner
            .decide_tag_removals(ids, decision, decided_by)
            .await
    }

    async fn add_tag(
        &self,
        add: TagAdd,
    ) -> Result<wire::gateway::memory::TagWire, MemoryStoreError> {
        self.inner.add_tag(add).await
    }

    async fn lookup_tags(
        &self,
        lookup: TagLookup,
    ) -> Result<
        std::collections::BTreeMap<String, Vec<wire::gateway::memory::TagWire>>,
        MemoryStoreError,
    > {
        self.inner.lookup_tags(lookup).await
    }

    async fn upsert_tag_taxonomy(
        &self,
        upsert: TagTaxonomyUpsert,
    ) -> Result<wire::gateway::memory::TagTaxonomyEntry, MemoryStoreError> {
        self.inner.upsert_tag_taxonomy(upsert).await
    }

    async fn decide_tag_taxonomy(
        &self,
        decision: TagTaxonomyDecision,
    ) -> Result<u64, MemoryStoreError> {
        self.inner.decide_tag_taxonomy(decision).await
    }

    async fn merge_tag_taxonomy(
        &self,
        merge: wire::gateway::memory::TagTaxonomyMerge,
    ) -> Result<wire::gateway::memory::TagTaxonomyMergeResult, MemoryStoreError> {
        self.inner.merge_tag_taxonomy(merge).await
    }

    async fn tag_job_context(
        &self,
        summary_id: uuid::Uuid,
    ) -> Result<Option<kernel::memory::TagJobContext>, MemoryStoreError> {
        self.inner.tag_job_context(summary_id).await
    }

    async fn store_tag_suggestions(
        &self,
        write: kernel::memory::TagSuggestionWrite,
    ) -> Result<kernel::memory::TagSuggestionStored, MemoryStoreError> {
        self.inner.store_tag_suggestions(write).await
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

pub(crate) async fn answer(app: axum::Router, request: Recorded) -> Result<Response, String> {
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
