use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use parking_lot::Mutex;
use serde::Deserialize;
use wire::plugin::RouteName;
use wire::plugin::action::{ViewAction, ViewActionRefusal, ViewActionResult};
use wire::plugin::process::{
    PluginEventDelivery, PluginJobResult, PluginToolCall, PluginToolResult, PluginViewsChanged,
};
use wire::plugin::view::{View, ViewBlock, ViewInline, ViewProperty};

const BROKEN_VIEW: &str = r#"{"title":"Broken","blocks":[{"type":"text"}]}"#;
const LOOPING_ANSWER: &str = r#"{"actions":[{"type":"call","action":"loop"}]}"#;

struct Fixture {
    api: reqwest::Client,
    token: String,
    events: Mutex<Vec<PluginEventDelivery>>,
    data: PathBuf,
}

#[derive(Deserialize)]
struct ApiProbe {
    method: String,
    path: String,
}

#[derive(Deserialize)]
struct FileProbe {
    path: PathBuf,
}

fn main() {
    let environment = config::Environment::from_process();
    let (Some(socket), Some(api), Some(token), Some(data)) = (
        environment.text("RIVETHUB_PLUGIN_SOCKET"),
        environment.text("RIVETHUB_API_SOCKET"),
        environment.text("RIVETHUB_PLUGIN_TOKEN"),
        environment.text("RIVETHUB_PLUGIN_DATA"),
    ) else {
        say("the plug-in contract variables are missing");
        return;
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => return say(&format!("no runtime: {error}")),
    };
    let client = match reqwest::Client::builder().unix_socket(api).build() {
        Ok(client) => client,
        Err(error) => return say(&format!("no API client: {error}")),
    };
    let fixture = Arc::new(Fixture {
        api: client,
        token: token.to_owned(),
        events: Mutex::new(Vec::new()),
        data: PathBuf::from(data),
    });
    if let Err(error) = runtime.block_on(serve(fixture, socket.to_owned())) {
        say(&format!("stopped: {error}"));
    }
}

fn say(line: &str) {
    let mut out = std::io::stdout().lock();
    if let Err(error) = writeln!(out, "fixture plug-in: {line}") {
        drop(error);
    }
}

async fn serve(fixture: Arc<Fixture>, socket: String) -> std::io::Result<()> {
    let own = probe(&fixture, "GET", "/api/plugin/self").await;
    say(&format!("started; /api/plugin/self answered {own}"));
    let listener = tokio::net::UnixListener::bind(socket)?;
    let router = Router::new()
        .route("/health", get(|| async { StatusCode::OK }))
        .route("/views/{view}", post(view))
        .route("/actions/{action}", post(action))
        .route("/tools/{tool}", post(tool))
        .route("/events", post(event))
        .route("/jobs/{job}", post(job))
        .route(
            "/http/{*rest}",
            get(|| async { "hello from the fixture plug-in" }),
        )
        .with_state(fixture);
    axum::serve(listener, router).await
}

async fn probe(fixture: &Fixture, method: &str, path: &str) -> String {
    let url = format!("http://rivethub{path}");
    let request = if method == "POST" {
        let views = match RouteName::try_from("status".to_owned()) {
            Ok(view) => vec![view],
            Err(error) => return error.to_string(),
        };
        fixture.api.post(url).json(&PluginViewsChanged { views })
    } else {
        fixture.api.get(url)
    };
    match request.bearer_auth(&fixture.token).send().await {
        Ok(response) => {
            let status = response.status().as_u16();
            match response.text().await {
                Ok(body) => format!("{status} {body}"),
                Err(error) => format!("{status} ({error})"),
            }
        }
        Err(error) => format!("unreachable: {error}"),
    }
}

fn raw_json(body: &'static str) -> Response {
    ([(header::CONTENT_TYPE, "application/json")], body).into_response()
}

fn shown(text: String) -> ViewInline {
    ViewInline::Text {
        text,
        tone: None,
        icon: None,
        monospace: true,
    }
}

async fn view(State(fixture): State<Arc<Fixture>>, Path(view): Path<String>) -> Response {
    if view == "broken" {
        return raw_json(BROKEN_VIEW);
    }
    let count = fixture.events.lock().len();
    Json(View {
        title: "Fixture".into(),
        subtitle: Some(format!("process {}", std::process::id())),
        toolbar: Vec::new(),
        blocks: vec![
            ViewBlock::Properties {
                items: vec![
                    ViewProperty {
                        label: "Events".into(),
                        value: shown(count.to_string()),
                    },
                    ViewProperty {
                        label: "Process".into(),
                        value: shown(std::process::id().to_string()),
                    },
                ],
            },
            ViewBlock::Markdown {
                text: format!("Data folder: `{}`", fixture.data.display()),
            },
        ],
    })
    .into_response()
}

async fn action(Path(action): Path<String>) -> Response {
    match action.as_str() {
        "refuse" => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(ViewActionRefusal {
                message: "refused".into(),
                fields: [("name".to_owned(), "taken".to_owned())].into(),
            }),
        )
            .into_response(),
        "loop" => raw_json(LOOPING_ANSWER),
        other => Json(ViewActionResult {
            actions: vec![ViewAction::Toast {
                text: format!("did {other}"),
                tone: None,
            }],
        })
        .into_response(),
    }
}

async fn tool(
    State(fixture): State<Arc<Fixture>>,
    Path(tool): Path<String>,
    Json(call): Json<PluginToolCall>,
) -> Json<PluginToolResult> {
    let answered = match tool.as_str() {
        "echo" => serde_json::to_string(&call.arguments).map_err(|error| error.to_string()),
        "events" => {
            let events = fixture.events.lock().clone();
            serde_json::to_string(&events).map_err(|error| error.to_string())
        }
        "pid" => Ok(std::process::id().to_string()),
        "read" => match call.arguments.deserialize::<FileProbe>() {
            Ok(asked) => std::fs::read_to_string(&asked.path).map_err(|error| error.to_string()),
            Err(error) => Err(error.to_string()),
        },
        "api" => match call.arguments.deserialize::<ApiProbe>() {
            Ok(asked) => Ok(probe(&fixture, &asked.method, &asked.path).await),
            Err(error) => Err(error.to_string()),
        },
        other => Err(format!("no tool {other}")),
    };
    Json(match answered {
        Ok(text) => PluginToolResult {
            text,
            is_error: false,
        },
        Err(text) => PluginToolResult {
            text,
            is_error: true,
        },
    })
}

async fn event(
    State(fixture): State<Arc<Fixture>>,
    Json(delivery): Json<PluginEventDelivery>,
) -> StatusCode {
    say(&format!("received event {}", delivery.seq));
    fixture.events.lock().push(delivery);
    StatusCode::NO_CONTENT
}

async fn job(Path(job): Path<String>) -> Json<PluginJobResult> {
    say(&format!("ran job {job}"));
    Json(PluginJobResult {
        outcome: wire::plugin::host::PluginJobOutcome::Succeeded,
        message: Some(format!("ran {job}")),
    })
}
