use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use config::provider::{ProviderBody, ProviderSlot};
use config::{Config, Environment};
use harness_acp::{HeadlessLaunch, Retry, Supervisor};
use kernel::harness::HarnessAdapter;
use kernel::model::{ModelProvider, ProviderHealth};
use kernel::shutdown::Activities;
use thiserror::Error;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

#[derive(Debug, Error)]
pub enum ModelError {
    #[error("provider {0} is missing")]
    Missing(String),
    #[error("provider {0} has no supported type")]
    Type(String),
    #[error("provider {0} requires a model on the provider or agent")]
    Model(String),
    #[error("headless provider startup failed: {0}")]
    Startup(#[from] tokio::task::JoinError),
    #[error("provider {0} has no composed harness adapter")]
    Harness(String),
    #[error(transparent)]
    Provider(#[from] providers::BuildError),
    #[error(transparent)]
    Cli(#[from] providers::cli::CliError),
}

#[expect(
    missing_debug_implementations,
    reason = "kernel harness ports do not implement Debug"
)]
#[derive(Default)]
pub struct ModelFactory {
    harnesses: BTreeMap<String, Arc<dyn HarnessAdapter>>,
    health: BTreeMap<String, ProviderHealth>,
    tasks: TaskTracker,
    cancel: CancellationToken,
    embed_endpoint: Option<String>,
    embed_model: Option<String>,
    activities: Activities,
}

#[derive(Debug, Clone)]
pub(crate) struct ModelTasks {
    tasks: TaskTracker,
    cancel: CancellationToken,
}

impl ModelTasks {
    pub(crate) async fn close(&self) {
        self.cancel.cancel();
        self.tasks.close();
        self.tasks.wait().await;
    }
}

#[expect(
    missing_debug_implementations,
    reason = "kernel model ports do not implement Debug"
)]
pub struct ModelBinding {
    pub provider: Arc<dyn ModelProvider>,
    pub model: String,
    pub embeddings: bool,
}

impl ModelFactory {
    fn with_embedding(env: &Environment) -> Self {
        Self {
            embed_endpoint: env.embed_url(),
            embed_model: env.embed_model(),
            ..Self::default()
        }
    }

    fn embedder(&self, config: &Config) -> Result<Option<providers::embed::Embedder>, ModelError> {
        let Some(memory) = &config.memory else {
            return Ok(None);
        };
        let text = |value: Option<&String>| {
            value
                .map(|text| text.trim())
                .filter(|text| !text.is_empty())
                .map(str::to_owned)
        };
        let settings = if config.sqlite_selected() {
            let section = memory.sqlite.as_ref();
            let endpoint = text(section.and_then(|section| section.embed_endpoint.as_ref()))
                .or_else(|| self.embed_endpoint.clone());
            let model = text(section.and_then(|section| section.embed_model.as_ref()))
                .or_else(|| self.embed_model.clone());
            endpoint.map(|endpoint| {
                Ok::<_, providers::BuildError>(providers::embed::EmbedConfig {
                    endpoint,
                    model: model.ok_or(providers::BuildError::MissingEmbedModel)?,
                    api_key: section.and_then(|section| section.embed_api_key.clone()),
                    api_prefix: None,
                    expected_dims: None,
                    timeout_ms: None,
                    query_instruction: None,
                })
            })
        } else {
            let section = memory.postgres.as_ref();
            let endpoint = text(section.and_then(|section| section.embed_endpoint.as_ref()))
                .or_else(|| self.embed_endpoint.clone());
            let model = text(section.and_then(|section| section.embed_model.as_ref()))
                .or_else(|| self.embed_model.clone());
            endpoint.map(|endpoint| {
                Ok::<_, providers::BuildError>(providers::embed::EmbedConfig {
                    endpoint,
                    model: model.ok_or(providers::BuildError::MissingEmbedModel)?,
                    api_key: section.and_then(|section| section.embed_api_key.clone()),
                    api_prefix: None,
                    expected_dims: section.and_then(|section| section.embed_expected_dims),
                    timeout_ms: section.and_then(|section| section.embed_timeout_ms),
                    query_instruction: section
                        .and_then(|section| section.embed_query_instruction.clone()),
                })
            })
        };
        settings
            .transpose()?
            .map(providers::embed::Embedder::build)
            .transpose()
            .map_err(Into::into)
    }

    pub(crate) fn tasks(&self) -> ModelTasks {
        ModelTasks {
            tasks: self.tasks.clone(),
            cancel: self.cancel.clone(),
        }
    }

    #[must_use]
    pub fn health(&self) -> Vec<ProviderHealth> {
        self.health.values().cloned().collect()
    }

    pub async fn from_config(
        config: &Config,
        env: &Environment,
        shutdown: &CancellationToken,
    ) -> Result<Self, ModelError> {
        Self::with_embedding(env)
            .with_agent_providers(config, env, shutdown)
            .await
    }

    pub async fn with_agent_providers(
        self,
        config: &Config,
        env: &Environment,
        shutdown: &CancellationToken,
    ) -> Result<Self, ModelError> {
        let ids = config
            .agents
            .iter()
            .flat_map(|agents| agents.values())
            .filter_map(|agent| agent.provider.as_deref())
            .collect();
        self.install(config, env, ids, shutdown).await
    }

    pub async fn for_provider(
        config: &Config,
        env: &Environment,
        id: &str,
        shutdown: &CancellationToken,
    ) -> Result<Self, ModelError> {
        Self::with_embedding(env)
            .install(config, env, BTreeSet::from([id]), shutdown)
            .await
    }

    #[must_use]
    pub fn with_harness(mut self, id: String, harness: Arc<dyn HarnessAdapter>) -> Self {
        self.harnesses.insert(id, harness);
        self
    }

    #[must_use]
    pub fn with_activities(mut self, activities: Activities) -> Self {
        self.activities = activities;
        self
    }

    async fn install(
        mut self,
        config: &Config,
        env: &Environment,
        ids: BTreeSet<&str>,
        shutdown: &CancellationToken,
    ) -> Result<Self, ModelError> {
        self.cancel = shutdown.child_token();
        let supervisor = Supervisor {
            shutdown: self.cancel.clone(),
            tasks: self.tasks.clone(),
            retry: Retry::STANDARD,
        };
        let search: Vec<_> = match env.get("PATH") {
            Some(paths) => std::env::split_paths(paths).collect(),
            None => Vec::new(),
        };
        let mut starting = JoinSet::new();
        for id in ids {
            if self.harnesses.contains_key(id) {
                continue;
            }
            let slot = slot(config, id)?;
            if let ProviderBody::Cli(body) = &slot.body {
                let kind = slot.kind.ok_or_else(|| ModelError::Type(id.into()))?;
                let harness = providers::cli::harness_id(kind)?;
                let launch =
                    HeadlessLaunch::prepare(config, harness, body, search.clone(), id.into()).await;
                starting.spawn(launch.start(supervisor.clone()));
            }
        }
        while let Some(started) = starting.join_next().await {
            let harness = started?;
            let health = harness.health();
            self.harnesses.insert(health.id.clone(), Arc::new(harness));
            self.health.insert(health.id.clone(), health);
        }
        Ok(self)
    }

    pub fn build(
        &self,
        config: &Config,
        id: &str,
        model: Option<&str>,
        shutdown: &CancellationToken,
    ) -> Result<ModelBinding, ModelError> {
        let slot = slot(config, id)?;
        let kind = slot.kind.ok_or_else(|| ModelError::Type(id.into()))?;
        match &slot.body {
            ProviderBody::Openai(body) => {
                let provider = providers::OpenAiCompatible::build(kind, body)?
                    .with_shutdown(shutdown.child_token())
                    .with_activities(self.activities.clone());
                let model = match model {
                    Some(model) => model.to_owned(),
                    None => provider.model().to_owned(),
                };
                Ok(ModelBinding {
                    provider: Arc::new(provider),
                    model,
                    embeddings: true,
                })
            }
            ProviderBody::Cli(body) => {
                let model = model
                    .or(body.model.as_deref())
                    .filter(|model| !model.is_empty())
                    .ok_or_else(|| ModelError::Model(id.into()))?
                    .to_owned();
                let harness = self
                    .harnesses
                    .get(id)
                    .ok_or_else(|| ModelError::Harness(id.into()))?;
                let mut options = body.as_ref().clone();
                options.cwd = options.cwd.or_else(|| {
                    config
                        .runtime
                        .as_ref()
                        .and_then(|runtime| runtime.workspace.clone())
                });
                let embedder = self.embedder(config)?;
                let embeddings = embedder.is_some();
                let provider = providers::cli::CliBacked::build(
                    kind,
                    options,
                    harness.clone(),
                    embedder,
                    shutdown,
                )?
                .with_task_tracker(self.tasks.clone());
                Ok(ModelBinding {
                    provider: Arc::new(provider),
                    model,
                    embeddings,
                })
            }
            ProviderBody::Unknown(_) | ProviderBody::Removed(_) => Err(ModelError::Type(id.into())),
        }
    }
}

fn slot<'a>(config: &'a Config, id: &str) -> Result<&'a ProviderSlot, ModelError> {
    config
        .providers
        .as_ref()
        .and_then(|slots| slots.get(id))
        .ok_or_else(|| ModelError::Missing(id.into()))
}
