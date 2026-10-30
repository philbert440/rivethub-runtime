use schemars::generate::SchemaSettings;
use schemars::{JsonSchema, Schema};
use serde::Deserialize;
use wire::JsonObject;
use wire::plugin::host::PluginToolEntry;
use wire::plugin::manifest::{PluginTool, PluginToolHint};
use wire::plugin::{PluginId, ToolName};

use super::super::HostError;
use super::NAMESPACE;

#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::EnumString, strum::Display)]
#[strum(serialize_all = "lowercase")]
pub(super) enum Tool {
    Templates,
    Scaffold,
    Install,
    Status,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct TemplatesArgs {
    #[serde(skip)]
    #[schemars(skip)]
    _marker: (),
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct ScaffoldArgs {
    #[schemars(description = "The template's name, as plugins-templates lists it")]
    pub(super) template: String,
    #[schemars(
        description = "The plug-in's id and folder name: 2 to 24 of a-z, 0-9 and single hyphens, starting with a letter"
    )]
    pub(super) id: String,
    #[schemars(description = "The name people see")]
    pub(super) name: String,
    #[schemars(description = "One sentence on what it does")]
    pub(super) description: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct InstallArgs {
    #[schemars(description = "The plug-in's id, which is also its folder's name")]
    pub(super) id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct StatusArgs {
    #[schemars(description = "The plug-in's id; leave out to list every plug-in on the node")]
    pub(super) id: Option<String>,
    #[schemars(
        description = "How many log lines to read (default 40)",
        range(min = 1, max = 200)
    )]
    pub(super) lines: Option<usize>,
}

struct Definition {
    tool: Tool,
    title: &'static str,
    description: &'static str,
    hints: &'static [PluginToolHint],
    schema: fn() -> Result<JsonObject, serde_json::Error>,
}

const READ_ONLY: &[PluginToolHint] = &[PluginToolHint::ReadOnly, PluginToolHint::Idempotent];

const DEFINITIONS: [Definition; 4] = [
    Definition {
        tool: Tool::Templates,
        title: "List plug-in templates",
        description: "List the plug-in templates this node holds: working example plug-ins that plugins-scaffold copies to start a new one. The first step when someone asks for a RivetHub plug-in.",
        hints: READ_ONLY,
        schema: schema::<TemplatesArgs>,
    },
    Definition {
        tool: Tool::Scaffold,
        title: "Start a RivetHub plug-in",
        description: "Start building a RivetHub plug-in on this node: copies a template into a new folder in the node's plug-in workshop, its manifest carrying this id, name and description. Read the GUIDE.md it holds, rewrite the example into what was asked for, then call plugins-install. Answers the folder's absolute path.",
        hints: &[],
        schema: schema::<ScaffoldArgs>,
    },
    Definition {
        tool: Tool::Install,
        title: "Install or reload a plug-in",
        description: "Install a plug-in from its folder in the node's plug-in workshop, or reload it after you changed it. Checks the manifest first (an invalid one changes nothing and the answer names the field), waits for a running plug-in to restart, and asks each of its pages for its view the way the apps do, reporting any answer rh refuses. Never turns a plug-in on and never grants a permission: a new plug-in stays off until a person turns it on from a paired device, and the answer says when and where they must. Once it is on, a reload restarts it with your changes unless it now asks for more permissions.",
        hints: &[],
        schema: schema::<InstallArgs>,
    },
    Definition {
        tool: Tool::Status,
        title: "Read a plug-in's state and log",
        description: "Read a plug-in's state, its permissions (granted and waiting for a person), its last exit and its last log lines, oldest first: its stdout and stderr, and rh's own lines (host), which name every answer rh refused and the field it refused. Without an id, lists this node's plug-ins.",
        hints: READ_ONLY,
        schema: schema::<StatusArgs>,
    },
];

fn schema<T: JsonSchema>() -> Result<JsonObject, serde_json::Error> {
    let schema = SchemaSettings::draft2020_12()
        .with(|settings| settings.inline_subschemas = true)
        .with_transform(plain)
        .into_generator()
        .into_root_schema_for::<T>();
    let mut object = JsonObject::from_serialize(&schema)?;
    object.retain(|key| key != "$schema");
    if object.get("properties").is_none() {
        object.insert("properties", JsonObject::default())?;
    }
    Ok(object)
}

fn plain(schema: &mut Schema) {
    schema.remove("title");
    schema.remove("format");
    schemars::transform::transform_subschemas(&mut plain, schema);
}

pub(in crate::plugins) fn entries() -> Result<Vec<PluginToolEntry>, HostError> {
    let invalid = |error: wire::plugin::PluginNameError| HostError::Invalid(error.to_string());
    let plugin = PluginId::try_from(NAMESPACE.to_owned()).map_err(invalid)?;
    DEFINITIONS
        .iter()
        .map(|definition| {
            Ok(PluginToolEntry {
                plugin: plugin.clone(),
                tool: PluginTool {
                    name: ToolName::try_from(definition.tool.to_string()).map_err(invalid)?,
                    title: Some(definition.title.to_owned()),
                    description: definition.description.to_owned(),
                    input_schema: (definition.schema)()?,
                    hints: definition.hints.to_vec(),
                },
            })
        })
        .collect()
}
