use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use wire::events::SessionBroadcast;
use wire::plugin::action::{ViewActionRefusal, ViewActionRequest, ViewActionResult};
use wire::plugin::canvas::{CanvasEventRequest, CanvasResponse};
use wire::plugin::host::{
    PluginChangedNotice, PluginEnableRequest, PluginInstallRequest, PluginList, PluginLogs,
    PluginSettings, PluginSettingsUpdate, PluginViewsNotice,
};
use wire::plugin::manifest::PluginManifest;
use wire::plugin::panel::{PanelHostMessage, PanelMessage};
use wire::plugin::process::{
    PluginEventDelivery, PluginJobRequest, PluginJobResult, PluginNotification, PluginNotify,
    PluginSelf, PluginToolCall, PluginToolResult, PluginViewsChanged,
};
use wire::plugin::view::{View, ViewBlock, ViewRequest};
use wire::plugin::{PluginId, RouteName, ToolName};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/plugin");

fn fixture(name: &str) -> Result<Value, String> {
    let path = format!("{FIXTURES}/{name}.json");
    let text = std::fs::read_to_string(&path).map_err(|error| format!("{path}: {error}"))?;
    serde_json::from_str(&text).map_err(|error| format!("{path}: {error}"))
}

fn normalized(value: &Value) -> Value {
    match value {
        Value::Object(members) => Value::Object(
            members
                .iter()
                .filter(|(_, member)| !member.is_null())
                .map(|(key, member)| (key.clone(), normalized(member)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(normalized).collect()),
        Value::Number(number) => number.as_f64().map_or(Value::Null, Value::from),
        Value::Null | Value::Bool(_) | Value::String(_) => value.clone(),
    }
}

fn roundtrip<T: DeserializeOwned + Serialize>(fixture: &Value, key: &str) -> Result<(), String> {
    let cases = fixture
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{key}: not a list of cases"))?;
    for (index, case) in cases.iter().enumerate() {
        let typed: T = serde_json::from_value(case.clone())
            .map_err(|error| format!("{key}[{index}]: {error}"))?;
        let written =
            serde_json::to_value(&typed).map_err(|error| format!("{key}[{index}]: {error}"))?;
        if normalized(&written) != normalized(case) {
            return Err(format!(
                "{key}[{index}] changed in a roundtrip\n  in : {case}\n  out: {written}"
            ));
        }
    }
    Ok(())
}

#[test]
fn a_workflows_plugin_fits_the_contract() -> Result<(), String> {
    let workflows = fixture("workflows")?;
    roundtrip::<PluginManifest>(&workflows, "manifests")?;
    roundtrip::<View>(&workflows, "views")?;
    roundtrip::<ViewRequest>(&workflows, "viewRequests")?;
    roundtrip::<ViewActionRequest>(&workflows, "actionRequests")?;
    roundtrip::<ViewActionResult>(&workflows, "actionResults")?;
    roundtrip::<ViewActionRefusal>(&workflows, "actionRefusals")?;
    roundtrip::<CanvasEventRequest>(&workflows, "canvasEvents")?;
    roundtrip::<CanvasResponse>(&workflows, "canvasResponses")?;
    roundtrip::<PluginEventDelivery>(&workflows, "eventDeliveries")?;
    roundtrip::<PluginToolCall>(&workflows, "toolCalls")?;
    roundtrip::<PluginToolResult>(&workflows, "toolResults")?;
    roundtrip::<PluginJobRequest>(&workflows, "jobRequests")?;
    roundtrip::<PluginJobResult>(&workflows, "jobResults")?;
    Ok(())
}

#[test]
fn a_file_browser_plugin_fits_the_contract() -> Result<(), String> {
    let files = fixture("files")?;
    roundtrip::<PluginManifest>(&files, "manifests")?;
    roundtrip::<View>(&files, "views")?;
    roundtrip::<ViewActionRequest>(&files, "actionRequests")?;
    roundtrip::<ViewActionResult>(&files, "actionResults")?;
    Ok(())
}

#[test]
fn host_routes_and_notices_fit_the_contract() -> Result<(), String> {
    let host = fixture("host")?;
    roundtrip::<PluginList>(&host, "lists")?;
    roundtrip::<PluginInstallRequest>(&host, "installRequests")?;
    roundtrip::<PluginEnableRequest>(&host, "enableRequests")?;
    roundtrip::<PluginLogs>(&host, "logs")?;
    roundtrip::<PluginSettings>(&host, "settings")?;
    roundtrip::<PluginSettingsUpdate>(&host, "settingsUpdates")?;
    roundtrip::<PluginChangedNotice>(&host, "changedNotices")?;
    roundtrip::<PluginViewsNotice>(&host, "viewsNotices")?;
    roundtrip::<SessionBroadcast>(&host, "changedNotices")?;
    roundtrip::<SessionBroadcast>(&host, "viewsNotices")?;
    roundtrip::<PluginSelf>(&host, "selves")?;
    roundtrip::<PluginViewsChanged>(&host, "viewsChanged")?;
    roundtrip::<PluginNotify>(&host, "notifies")?;
    roundtrip::<PluginNotification>(&host, "notifications")?;
    roundtrip::<PanelHostMessage>(&host, "panelHostMessages")?;
    roundtrip::<PanelMessage>(&host, "panelMessages")?;
    Ok(())
}

fn accepts<T: DeserializeOwned>(text: &str) -> bool {
    serde_json::from_value::<T>(Value::String(text.to_owned())).is_ok()
}

#[test]
fn plugin_ids_tool_names_and_route_names_are_checked() {
    for id in [
        "workflows",
        "file-browser",
        "a1",
        "abcdefghijklmnopqrstuvwx",
    ] {
        assert!(accepts::<PluginId>(id), "{id}");
    }
    for id in [
        "a",
        "1abc",
        "Files",
        "ab-",
        "a--b",
        "a_b",
        "abcdefghijklmnopqrstuvwxy",
    ] {
        assert!(!accepts::<PluginId>(id), "{id}");
    }
    for name in ["start_run", "x", "run2"] {
        assert!(accepts::<ToolName>(name), "{name}");
    }
    for name in ["start__run", "_x", "x_", "start-run", "2run"] {
        assert!(!accepts::<ToolName>(name), "{name}");
    }
    for name in ["run-42", "start_run", "0"] {
        assert!(accepts::<RouteName>(name), "{name}");
    }
    let long = "a".repeat(65);
    for name in ["", "-run", "a/b", "Run", "a.b", long.as_str()] {
        assert!(!accepts::<RouteName>(name), "{name}");
    }
}

#[test]
fn unknown_fields_and_kinds_are_refused() {
    let typo = serde_json::json!({
        "manifest": 1, "id": "files", "name": "Files", "version": "0.1.0", "description": "Files.",
        "run": { "command": ["files"] }, "permision": ["host.files.read"]
    });
    assert!(serde_json::from_value::<PluginManifest>(typo).is_err());
    let colour = serde_json::json!({ "type": "text", "text": "x", "colour": "red" });
    assert!(serde_json::from_value::<ViewBlock>(colour).is_err());
    let unknown = serde_json::json!({ "type": "chart", "series": [] });
    assert!(serde_json::from_value::<ViewBlock>(unknown).is_err());
    let text = serde_json::json!({ "type": "text", "text": "x" });
    assert!(serde_json::from_value::<ViewBlock>(text).is_ok());
}
