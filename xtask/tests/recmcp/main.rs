use std::future::Future;
use std::path::Path;

use rmcp::ServerHandler;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ErrorCode, ErrorData,
    Implementation, JsonObject, ListToolsResult, PaginatedRequestParams, ProtocolVersion,
    ServerCapabilities, ServerConfig, Tool, ToolAnnotations,
};
use rmcp::service::{RequestContext, RoleServer, serve_server};
use xtask::recmcp::records::{
    CALLS_FILE, CallOutcome, CallsDocument, META_FILE, MetaDocument, TOOLS_FILE, ToolsDocument,
};
use xtask::recmcp::script::{self, Script, ScriptCall, ScriptError};
use xtask::recmcp::{collect, mask, write_documents};

const ECHO_SCHEMA: &str =
    r#"{"type":"object","properties":{"message":{"type":"string"}},"required":["message"]}"#;
const FAIL_SCHEMA: &str = r#"{"type":"object","properties":{}}"#;

struct CatalogServer;

impl ServerHandler for CatalogServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("acme-kit", "1.2.3"))
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, ErrorData>> {
        std::future::ready(catalog())
    }

    fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<CallToolResponse, ErrorData>> {
        std::future::ready(dispatch_call(&request))
    }
}

fn catalog() -> Result<ListToolsResult, ErrorData> {
    let mut echo = Tool::new("acme_echo", "echo the message back", schema(ECHO_SCHEMA)?);
    echo.annotations = Some(ToolAnnotations::from_raw(
        None,
        Some(true),
        Some(false),
        Some(true),
        Some(false),
    ));
    let fail = Tool::new(
        "acme_fail",
        "always fails and mentions /home/planted-user/notes",
        schema(FAIL_SCHEMA)?,
    );
    Ok(ListToolsResult::with_all_items(vec![fail, echo]))
}

fn dispatch_call(request: &CallToolRequestParams) -> Result<CallToolResponse, ErrorData> {
    match request.name.as_ref() {
        "acme_echo" => {
            let message = request
                .arguments
                .as_ref()
                .and_then(|arguments| arguments.get("message"))
                .and_then(|value| value.as_str())
                .ok_or_else(|| ErrorData::invalid_params("message is required", None))?;
            Ok(
                CallToolResult::success(vec![ContentBlock::text(format!("acme: {message}"))])
                    .into(),
            )
        }
        "acme_fail" => {
            Ok(CallToolResult::error(vec![ContentBlock::text("acme failed on purpose")]).into())
        }
        other => Err(ErrorData::new(
            ErrorCode::METHOD_NOT_FOUND,
            format!("tool is unknown: {other}"),
            None,
        )),
    }
}

fn schema(text: &str) -> Result<JsonObject, ErrorData> {
    serde_json::from_str(text)
        .map_err(|error| ErrorData::internal_error(format!("test schema: {error}"), None))
}

fn arguments(text: &str) -> Result<JsonObject, serde_json::Error> {
    serde_json::from_str(text)
}

fn recording_script() -> Result<Script, Box<dyn std::error::Error>> {
    Ok(Script {
        calls: vec![
            ScriptCall {
                name: "acme_echo".to_owned(),
                arguments: Some(arguments(r#"{"message":"héllo ✓"}"#)?),
                note: Some("the round trip".to_owned()),
            },
            ScriptCall {
                name: "acme_fail".to_owned(),
                arguments: Some(arguments(r#"{"message":"boom"}"#)?),
                note: None,
            },
            ScriptCall {
                name: "acme_missing".to_owned(),
                arguments: Some(JsonObject::new()),
                note: Some("an unknown tool".to_owned()),
            },
        ],
    })
}

#[tokio::test]
async fn records_the_three_files_from_an_in_process_server()
-> Result<(), Box<dyn std::error::Error>> {
    let out = tempfile::tempdir()?;
    let script = recording_script()?;
    let (client_io, server_io) = tokio::io::duplex(8192);
    let server = tokio::spawn(serve_server(CatalogServer, server_io));
    let recording = collect(client_io, &script).await?;
    server.await??;
    let masked = write_documents(
        out.path(),
        &recording,
        "acme-server",
        "2026-10-13T08:30:00Z",
    )?;

    assert_tools(
        &std::fs::read_to_string(out.path().join(TOOLS_FILE))?,
        &masked,
    )?;
    assert_calls(&std::fs::read_to_string(out.path().join(CALLS_FILE))?)?;
    assert_meta(&std::fs::read_to_string(out.path().join(META_FILE))?)?;
    Ok(())
}

fn assert_tools(text: &str, masked: &[mask::Masked]) -> Result<(), Box<dyn std::error::Error>> {
    assert!(
        !text.contains("/home/planted-user"),
        "the planted home path must be masked: {text}"
    );
    assert!(text.contains("[masked home-path]"));
    let tools: ToolsDocument = serde_json::from_str(text)?;
    let names: Vec<&str> = tools.tools.iter().map(|tool| tool.name.as_ref()).collect();
    assert_eq!(names, ["acme_echo", "acme_fail"], "tools must be sorted");
    let echo = tools.tools.first().ok_or("the echo tool is missing")?;
    assert_eq!(
        echo.description.as_ref().map(AsRef::as_ref),
        Some("echo the message back")
    );
    assert_eq!(
        echo.input_schema.get("type").and_then(|kind| kind.as_str()),
        Some("object")
    );
    let annotations = echo
        .annotations
        .as_ref()
        .ok_or("the echo annotations are missing")?;
    assert_eq!(annotations.read_only_hint, Some(true));
    assert_eq!(annotations.idempotent_hint, Some(true));
    assert_eq!(masked.len(), 1, "only the planted home path is masked");
    let hit = masked
        .first()
        .ok_or("the masked report must not be empty")?;
    assert_eq!(hit.rule, mask::HOME_PATH);
    assert_eq!(hit.file, TOOLS_FILE);
    Ok(())
}

fn assert_calls(text: &str) -> Result<(), Box<dyn std::error::Error>> {
    let calls: CallsDocument = serde_json::from_str(text)?;
    assert_eq!(calls.calls.len(), 3);
    let echo_call = calls.calls.first().ok_or("the echo call is missing")?;
    assert_eq!(echo_call.request.name, "acme_echo");
    assert_eq!(echo_call.note.as_deref(), Some("the round trip"));
    let CallOutcome::Result { result } = &echo_call.outcome else {
        return Err("acme_echo must record a result".into());
    };
    assert_eq!(result.is_error, Some(false));
    let echo_text = result
        .content
        .first()
        .and_then(|block| block.as_text())
        .map(|text_content| text_content.text.clone());
    assert_eq!(echo_text.as_deref(), Some("acme: héllo ✓"));
    let fail_call = calls.calls.get(1).ok_or("the fail call is missing")?;
    let CallOutcome::Result { result } = &fail_call.outcome else {
        return Err("acme_fail must record a result".into());
    };
    assert_eq!(result.is_error, Some(true));
    let missing_call = calls.calls.get(2).ok_or("the unknown call is missing")?;
    let CallOutcome::ProtocolError { code, message } = &missing_call.outcome else {
        return Err("acme_missing must record the protocol error".into());
    };
    assert_eq!(*code, ErrorCode::METHOD_NOT_FOUND.0);
    assert!(message.contains("acme_missing"), "error names the tool");
    Ok(())
}

fn assert_meta(text: &str) -> Result<(), Box<dyn std::error::Error>> {
    let meta: MetaDocument = serde_json::from_str(text)?;
    assert_eq!(meta.recorder_version, env!("CARGO_PKG_VERSION"));
    assert_eq!(meta.program, "acme-server");
    assert_eq!(meta.recorded_at, "2026-10-13T08:30:00Z");
    assert_eq!(
        meta.server.protocol_version,
        ProtocolVersion::LATEST_WITH_INITIALIZE
    );
    let server_info = meta
        .server
        .server_info
        .as_ref()
        .ok_or("the server info is missing")?;
    assert_eq!(server_info.name, "acme-kit");
    assert_eq!(server_info.version, "1.2.3");
    Ok(())
}

#[test]
fn masks_privacy_rule_matches_in_a_document() {
    let document = concat!(
        r#"{ "path": "/home/someone/notes", "keep": "/home/rivethub/data","#, // gate-allow: the payload is the finding
        r#" "host": "box.lan", "private": "10.0.0.7","#, // gate-allow: the payload is the finding
        r#" "link": "fe80::1", "public": "203.0.113.7" }"#, // gate-allow: the payload is the finding
    );
    let (masked_text, hits) = mask::mask_document(CALLS_FILE, document);
    assert_eq!(
        masked_text,
        concat!(
            r#"{ "path": "[masked home-path]/notes", "keep": "/home/rivethub/data","#,
            r#" "host": "[masked host]", "private": "[masked ip]","#,
            r#" "link": "[masked ip]", "public": "[masked ip]" }"#
        )
    );
    let rules: Vec<&str> = hits.iter().map(|hit| hit.rule).collect();
    assert_eq!(
        rules,
        [
            mask::HOME_PATH,
            mask::LAN_HOST,
            mask::IP,
            mask::IP,
            mask::IP
        ]
    );
    assert!(hits.iter().all(|hit| hit.file == CALLS_FILE));
}

#[test]
fn script_parse_errors_are_typed() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let path = |name: &str| dir.path().join(name);

    std::fs::write(path("broken.json"), "{")?;
    let error = script::load(&path("broken.json")).unwrap_err();
    assert!(matches!(error, ScriptError::Parse { .. }), "{error}");

    std::fs::write(path("wrong-shape.json"), r#"{"calls": "all"}"#)?;
    let error = script::load(&path("wrong-shape.json")).unwrap_err();
    assert!(matches!(error, ScriptError::Parse { .. }), "{error}");

    std::fs::write(path("unknown-field.json"), r#"{"calls": [], "extra": 1}"#)?;
    let error = script::load(&path("unknown-field.json")).unwrap_err();
    assert!(matches!(error, ScriptError::Parse { .. }), "{error}");

    std::fs::write(path("empty.json"), r#"{"calls": []}"#)?;
    let error = script::load(&path("empty.json")).unwrap_err();
    assert!(matches!(error, ScriptError::NoCalls { .. }), "{error}");

    std::fs::write(path("no-name.json"), r#"{"calls": [{"name": ""}]}"#)?;
    let error = script::load(&path("no-name.json")).unwrap_err();
    let ScriptError::EmptyName { path: named, index } = error else {
        return Err("the empty name must be a typed EmptyName error".into());
    };
    assert_eq!(index, 0);
    assert_eq!(named, path("no-name.json"));

    let error = script::load(&path("missing.json")).unwrap_err();
    assert!(matches!(error, ScriptError::Read { .. }), "{error}");
    Ok(())
}

#[test]
fn the_default_script_parses_and_names_only_safe_calls() -> Result<(), Box<dyn std::error::Error>> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("data/mcp-script.json");
    let script = script::load(&path)?;
    let names: Vec<&str> = script.calls.iter().map(|call| call.name.as_str()).collect();
    assert_eq!(
        names,
        ["echo", "echo", "echo", "no-such-tool", "skill_list"]
    );
    assert!(
        script
            .calls
            .iter()
            .all(|call| call.note.as_deref().is_some_and(|note| !note.is_empty()))
    );
    Ok(())
}
