use std::collections::BTreeMap;
use std::sync::Arc;

use wire::plugin::PluginId;
use wire::plugin::host::{PluginLogStream, PluginSettings, PluginSettingsUpdate};
use wire::plugin::process::PluginEvent;
use wire::plugin::view::ViewValue;

use super::{HostError, PluginHost, registry};

impl PluginHost {
    pub(crate) fn plugin_settings(&self, plugin: &PluginId) -> Result<PluginSettings, HostError> {
        self.read(plugin, |entry| {
            kernel::plugins::settings::shown(
                &entry.stored.manifest.settings,
                &entry.stored.settings,
            )
        })
    }

    pub(crate) async fn change_settings(
        &self,
        plugin: &PluginId,
        update: &PluginSettingsUpdate,
    ) -> Result<PluginSettings, HostError> {
        let _operation = self.operations.lock().await;
        let kept = self
            .read(plugin, |entry| {
                kernel::plugins::settings::apply(
                    &entry.stored.manifest.settings,
                    &entry.stored.settings,
                    &update.values,
                )
            })?
            .map_err(|problem| HostError::Invalid(problem.to_string()))?;
        let (values, queue) = self.update(plugin, |entry| {
            entry.stored.settings = kept;
            (
                effective(&entry.stored),
                entry
                    .live
                    .supervisor
                    .as_ref()
                    .map(|supervisor| Arc::clone(&supervisor.events)),
            )
        })?;
        self.persist().await?;
        self.log(plugin, PluginLogStream::Host, "settings changed".to_owned());
        if let Some(queue) = queue {
            queue.push(PluginEvent::SettingsChanged { values });
        }
        self.plugin_settings(plugin)
    }
}

pub(super) fn effective(stored: &registry::Stored) -> BTreeMap<String, ViewValue> {
    kernel::plugins::settings::effective(&stored.manifest.settings, &stored.settings)
}
