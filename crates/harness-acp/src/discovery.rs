use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use agent_client_protocol::schema::v1 as acp;
use kernel::harness::Command;
use kernel::harness::SessionEvent;
use wire::harness::HarnessModelsSource;
use wire::harness::HarnessOffering;

use crate::connection::AgentConnection;
use crate::error::AcpError;
use crate::launch::AcpCommand;
use crate::options::SessionOptions;

#[derive(Debug, Clone)]
pub(crate) struct Advertisement {
    capabilities: acp::AgentCapabilities,
    options: Arc<SessionOptions>,
    commands: Vec<Command>,
}

impl Advertisement {
    pub(crate) fn from_agent(agent: &AgentConnection) -> Self {
        Self {
            capabilities: agent.capabilities.clone(),
            options: Arc::clone(&agent.options),
            commands: Vec::new(),
        }
    }

    pub(crate) fn commands(&self) -> &[Command] {
        &self.commands
    }

    pub(crate) fn apply(
        &self,
        offering: &mut HarnessOffering,
        input: Option<&crate::sheets::SheetInput>,
    ) -> Result<(), AcpError> {
        offering.chat = true;
        offering.capabilities.control.resume = self.capabilities.load_session;
        offering.capabilities.control.interrupt = true;
        offering.capabilities.control.approvals = true;
        offering.capabilities.observation.live_stream = true;
        offering.capabilities.observation.drive = Some(true);
        offering.capabilities.observation.list_sessions =
            self.capabilities.session_capabilities.list.is_some();
        if let Some(mut models) = self.options.models()? {
            let configured = input
                .and_then(|input| input.models.as_ref())
                .filter(|models| !models.is_empty());
            let mode = input.and_then(|input| input.mode);
            let source = if let Some(configured) =
                configured.filter(|_| mode != Some(config::ModelsMode::Discover))
            {
                let configured_models = offering
                    .capabilities
                    .models
                    .take()
                    .into_iter()
                    .flatten()
                    .filter(|model| configured.contains(&model.id));
                if mode == Some(config::ModelsMode::Replace) {
                    models = configured_models
                        .map(|configured| {
                            let mut enriched = configured;
                            if let Some(advertised) =
                                models.iter().find(|model| model.id == enriched.id)
                            {
                                enriched.clone_from(advertised);
                            }
                            enriched
                        })
                        .collect();
                    HarnessModelsSource::Config
                } else {
                    for configured in configured_models {
                        if !models.iter().any(|model| model.id == configured.id) {
                            models.push(configured);
                        }
                    }
                    HarnessModelsSource::Merged
                }
            } else {
                HarnessModelsSource::Discovered
            };
            offering.capabilities.models = Some(models);
            offering.capabilities.models_source = Some(source);
            offering.capabilities.turn_options = Some(true);
        }
        Ok(())
    }
}

pub const DEFAULT_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(30);

pub(crate) async fn discover(
    command: &AcpCommand,
    cwd: &Path,
    timeout: Duration,
) -> Result<Advertisement, AcpError> {
    let agent = tokio::time::timeout(timeout, crate::connection::start_new(command, cwd))
        .await
        .map_err(|_| AcpError::DiscoveryTimeout { timeout })??;
    let mut advertised = Advertisement::from_agent(&agent);
    agent.shutdown.cancel();
    let mut events = agent.events_rx;
    agent
        .reaped
        .await
        .map_err(|_| AcpError::Closed {
            method: "discovery",
        })?
        .map_err(|source| AcpError::Launch {
            program: command.program.display().to_string(),
            source,
        })?;
    agent.tasks.wait().await;
    while let Ok(event) = events.try_recv() {
        if let SessionEvent::AvailableCommands(commands) = event {
            advertised.commands = commands;
        }
    }
    Ok(advertised)
}

#[cfg(test)]
mod tests {
    use super::Advertisement;
    use crate::options::SessionOptions;
    use crate::sheets::{SheetInput, offering};
    use agent_client_protocol::schema::v1 as acp;
    use config::ModelsMode;
    use std::sync::Arc;
    use wire::harness::{HarnessId, HarnessModelsSource};

    #[test]
    fn configured_models_keep_advertised_labels_and_defaults() {
        let options = Arc::new(SessionOptions::default());
        options.replace(vec![
            acp::SessionConfigOption::select(
                "model",
                "Model",
                "opus",
                vec![
                    acp::SessionConfigSelectOption::new("opus", "Opus advertised"),
                    acp::SessionConfigSelectOption::new("fast", "Fast advertised"),
                ],
            )
            .category(acp::SessionConfigOptionCategory::Model),
        ]);
        let advertisement = Advertisement {
            capabilities: acp::AgentCapabilities::new(),
            options,
            commands: Vec::new(),
        };
        for (mode, source, ids) in [
            (
                ModelsMode::Replace,
                HarnessModelsSource::Config,
                vec!["opus", "custom/model"],
            ),
            (
                ModelsMode::Merge,
                HarnessModelsSource::Merged,
                vec!["opus", "fast", "custom/model"],
            ),
            (
                ModelsMode::Discover,
                HarnessModelsSource::Discovered,
                vec!["opus", "fast"],
            ),
        ] {
            let input = SheetInput {
                mode: Some(mode),
                models: Some(vec!["opus".to_owned(), "custom/model".to_owned()]),
                efforts: None,
            };
            let mut offering = offering(HarnessId::ClaudeCode, true, Some(&input)).expect("sheet");
            advertisement
                .apply(&mut offering, Some(&input))
                .expect("advertisement");
            assert_eq!(offering.capabilities.models_source, Some(source));
            let models = offering.capabilities.models.expect("models");
            assert_eq!(
                models
                    .iter()
                    .map(|model| model.id.as_str())
                    .collect::<Vec<_>>(),
                ids
            );
            let opus = models.first().expect("opus");
            assert_eq!(opus.label, "Opus advertised");
            assert_eq!(opus.is_default, Some(true));
        }
    }
}
