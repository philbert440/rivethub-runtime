use values::{JsonObject, PluginId, SessionId, ToolName};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginToolList {
    pub version: u64,
    pub tools: Vec<PluginToolEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginToolEntry {
    pub plugin: PluginId,
    pub tool: PluginTool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginTool {
    pub name: ToolName,
    pub title: Option<String>,
    pub description: String,
    pub input_schema: JsonObject,
    pub hints: Vec<PluginToolHint>,
}

#[cfg_attr(
    dylint_lib = "rr_lints",
    expect(
        duptype,
        reason = "KERNELWIRE: the kernel owns what a plug-in tool declares about itself; wire::plugin::manifest::PluginToolHint is the manifest's copy, translated by the plug-in host"
    )
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginToolHint {
    ReadOnly,
    Destructive,
    Idempotent,
    OpenWorld,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginToolCall {
    pub arguments: JsonObject,
    pub session_id: Option<SessionId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginToolResult {
    pub text: String,
    pub is_error: bool,
}
