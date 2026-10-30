use std::sync::Arc;

use crate::Gateway;
use crate::StartupMemory;

impl Gateway {
    #[must_use]
    pub fn with_plugins(mut self, host: Arc<crate::plugins::PluginHost>) -> Self {
        if let Some(terminal) = &self.terminal {
            terminal.attach_plugins(Arc::clone(&host));
        }
        self.plugins = Some(host);
        self
    }

    #[must_use]
    pub fn plugins(&self) -> Option<&Arc<crate::plugins::PluginHost>> {
        self.plugins.as_ref()
    }

    #[must_use]
    pub fn with_topics(
        mut self,
        store: Arc<dyn kernel::topics::TopicStore>,
        filing: Arc<dyn kernel::filing::FilingStore>,
    ) -> Self {
        self.topics = Some(store);
        self.filing = Some(filing);
        self
    }

    pub(crate) fn filing(
        &self,
    ) -> Result<&Arc<dyn kernel::filing::FilingStore>, crate::error::ApiError> {
        self.filing
            .as_ref()
            .ok_or_else(|| crate::error::ApiError::Unavailable("filing store unavailable".into()))
    }

    pub(crate) fn topics(
        &self,
    ) -> Result<&Arc<dyn kernel::topics::TopicStore>, crate::error::ApiError> {
        self.topics
            .as_ref()
            .ok_or_else(|| crate::error::ApiError::Unavailable("topic store unavailable".into()))
    }

    #[must_use]
    pub fn with_tidy(
        mut self,
        tidy: Arc<dyn kernel::tidy::TidyStore>,
        upkeep: Arc<dyn kernel::tidy::UpkeepStore>,
    ) -> Self {
        self.tidy = Some(tidy);
        self.upkeep = Some(upkeep);
        self
    }

    pub(crate) fn tidy(&self) -> Result<&Arc<dyn kernel::tidy::TidyStore>, crate::error::ApiError> {
        self.tidy
            .as_ref()
            .ok_or_else(|| crate::error::ApiError::Unavailable("tidy store unavailable".into()))
    }

    pub(crate) fn upkeep(
        &self,
    ) -> Result<&Arc<dyn kernel::tidy::UpkeepStore>, crate::error::ApiError> {
        self.upkeep
            .as_ref()
            .ok_or_else(|| crate::error::ApiError::Unavailable("upkeep store unavailable".into()))
    }

    #[must_use]
    pub fn with_summary_watch(mut self, watch: Arc<dyn kernel::memory::SummaryWatch>) -> Self {
        self.summaries = Some(watch);
        self
    }

    #[must_use]
    pub fn with_decisions(mut self, store: Arc<dyn kernel::topics::DecisionStore>) -> Self {
        self.decisions = Some(store);
        self
    }

    pub(crate) fn decisions(
        &self,
    ) -> Result<&Arc<dyn kernel::topics::DecisionStore>, crate::error::ApiError> {
        self.decisions
            .as_ref()
            .ok_or_else(|| crate::error::ApiError::Unavailable("decision store unavailable".into()))
    }

    #[must_use]
    pub fn with_startup_memory(
        mut self,
        pages: Arc<dyn kernel::topics::TopicPageStore>,
        kept: Arc<dyn kernel::topics::StartupStore>,
    ) -> Self {
        self.startup = Some(StartupMemory { pages, kept });
        self
    }

    pub(crate) fn pages(
        &self,
    ) -> Result<&dyn kernel::topics::TopicPageStore, crate::error::ApiError> {
        self.startup
            .as_ref()
            .map(|startup| startup.pages.as_ref())
            .ok_or_else(|| crate::error::ApiError::Unavailable("wiki pages unavailable".into()))
    }

    pub(crate) fn startup_memory(
        &self,
    ) -> Option<(
        &dyn kernel::topics::TopicPageStore,
        &dyn kernel::topics::StartupStore,
    )> {
        self.startup
            .as_ref()
            .map(|startup| (startup.pages.as_ref(), startup.kept.as_ref()))
    }

    pub(crate) fn memory_export(&self) -> Option<&Arc<dyn kernel::memory::MemoryExport>> {
        self.memory_export.as_ref()
    }
}
