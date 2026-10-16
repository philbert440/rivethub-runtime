use std::collections::BTreeMap;
use std::sync::Arc;

use config::provider::{ProviderBody, ProviderSlot, ProviderType};
use config::{Config, Environment};
use kernel::harness::HarnessAdapter;
use kernel::model::ModelProvider;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Error)]
pub enum ModelError {
    #[error("provider {0} is missing")]
    Missing(String),
    #[error("provider {0} has no supported type")]
    Type(String),
    #[error("provider {0} requires a model on the provider or agent")]
    Model(String),
    #[error("provider {provider} ({kind}) has no headless launcher: {source}")]
    Launcher {
        provider: String,
        kind: ProviderType,
        source: Box<harness_acp::AcpError>,
    },
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
    tasks: tokio_util::task::TaskTracker,
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
    pub(crate) fn tasks(&self) -> tokio_util::task::TaskTracker {
        self.tasks.clone()
    }

    pub async fn from_config(config: &Config, env: &Environment) -> Result<Self, ModelError> {
        let mut factory = Self::default();
        if let Some(agents) = &config.agents {
            for agent in agents.values() {
                if let Some(id) = &agent.provider {
                    factory.install(config, env, id).await?;
                }
            }
        }
        Ok(factory)
    }

    pub async fn for_provider(
        config: &Config,
        env: &Environment,
        id: &str,
    ) -> Result<Self, ModelError> {
        let mut factory = Self::default();
        factory.install(config, env, id).await?;
        Ok(factory)
    }

    #[must_use]
    pub fn with_harness(mut self, id: String, harness: Arc<dyn HarnessAdapter>) -> Self {
        self.harnesses.insert(id, harness);
        self
    }

    async fn install(
        &mut self,
        config: &Config,
        env: &Environment,
        id: &str,
    ) -> Result<(), ModelError> {
        if self.harnesses.contains_key(id) {
            return Ok(());
        }
        let slot = slot(config, id)?;
        if let ProviderBody::Cli(body) = &slot.body {
            let kind = slot.kind.ok_or_else(|| ModelError::Type(id.into()))?;
            let harness = providers::cli::harness_id(kind)?;
            let search = match env.get("PATH") {
                Some(paths) => std::env::split_paths(paths).collect(),
                None => Vec::new(),
            };
            let adapter =
                harness_acp::AcpHarnessAdapter::for_provider(config, harness, body, search)
                    .await
                    .map_err(|source| ModelError::Launcher {
                        provider: id.into(),
                        kind,
                        source: Box::new(source),
                    })?;
            self.harnesses.insert(id.into(), Arc::new(adapter));
        }
        Ok(())
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
                let provider = providers::OpenAiCompatible::build(kind, body)?;
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
                let embedder = embedder(config)?;
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

fn embedder(config: &Config) -> Result<Option<providers::embed::Embedder>, ModelError> {
    let Some(memory) = &config.memory else {
        return Ok(None);
    };
    let settings = if config.sqlite_selected() {
        memory.sqlite.as_ref().and_then(|section| {
            section.embed_endpoint.as_ref().map(|endpoint| {
                Ok::<_, providers::BuildError>(providers::embed::EmbedConfig {
                    endpoint: endpoint.clone(),
                    model: section
                        .embed_model
                        .clone()
                        .ok_or(providers::BuildError::MissingEmbedModel)?,
                    api_key: section.embed_api_key.clone(),
                    api_prefix: None,
                    expected_dims: None,
                    timeout_ms: None,
                    query_instruction: None,
                })
            })
        })
    } else {
        memory.postgres.as_ref().and_then(|section| {
            section.embed_endpoint.as_ref().map(|endpoint| {
                Ok::<_, providers::BuildError>(providers::embed::EmbedConfig {
                    endpoint: endpoint.clone(),
                    model: section
                        .embed_model
                        .clone()
                        .ok_or(providers::BuildError::MissingEmbedModel)?,
                    api_key: section.embed_api_key.clone(),
                    api_prefix: None,
                    expected_dims: section.embed_expected_dims,
                    timeout_ms: section.embed_timeout_ms,
                    query_instruction: section.embed_query_instruction.clone(),
                })
            })
        })
    };
    settings
        .transpose()?
        .map(providers::embed::Embedder::build)
        .transpose()
        .map_err(Into::into)
}
