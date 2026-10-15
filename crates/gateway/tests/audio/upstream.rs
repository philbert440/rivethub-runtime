use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::{Json, Router};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_util::sync::CancellationToken;

use super::{Heard, Hit, Seen, Upstream};

impl Seen {
    fn new(stop: CancellationToken) -> Self {
        Self {
            hits: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            stop,
        }
    }

    async fn push(&self, hit: Hit) {
        self.hits.lock().await.push(hit);
    }

    pub(super) async fn take(&self) -> Vec<Hit> {
        std::mem::take(&mut *self.hits.lock().await)
    }
}

impl Upstream {
    pub(super) async fn start() -> Result<Self, String> {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .map_err(|error| error.to_string())?;
        let addr = listener.local_addr().map_err(|error| error.to_string())?;
        let token = CancellationToken::new();
        let seen = Seen::new(token.clone());
        let app = Router::new()
            .route("/stt", any(take))
            .route("/land", any(take))
            .route("/fail", any(take))
            .route("/hang", any(take))
            .route("/hop", any(take))
            .route("/ftp", any(take))
            .route("/tts", any(take))
            .route("/mpeg", any(take))
            .route("/tts-fail", any(take))
            .with_state(seen.clone());
        let stop = token.clone();
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move { stop.cancelled().await })
                .await
        });
        Ok(Self {
            addr,
            seen,
            token,
            server,
        })
    }

    pub(super) fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    pub(super) async fn stop(self) -> Result<(), String> {
        self.token.cancel();
        tokio::time::timeout(Duration::from_secs(5), self.server)
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())?;
        Ok(())
    }
}

async fn take(State(seen): State<Seen>, request: Request) -> Response {
    let path = request.uri().path().to_owned();
    if path == "/hang" {
        seen.stop.cancelled().await;
        return StatusCode::NO_CONTENT.into_response();
    }
    let Ok(body) = axum::body::to_bytes(request.into_body(), 2 * 1024 * 1024).await else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let body = body.to_vec();
    seen.push(Hit {
        path: path.clone(),
        body,
    })
    .await;
    answer_path(&path)
}

fn answer_path(path: &str) -> Response {
    match path {
        "/stt" => Json(Heard {
            text: "heard".to_owned(),
            ignored: true,
        })
        .into_response(),
        "/land" => Json(Heard {
            text: "followed".to_owned(),
            ignored: true,
        })
        .into_response(),
        "/fail" => (StatusCode::INTERNAL_SERVER_ERROR, "upstream said no").into_response(),
        "/tts-fail" => (StatusCode::INTERNAL_SERVER_ERROR, "synth failed").into_response(),
        "/hop" => redirect("/land"),
        "/ftp" => redirect("ftp://example.test/a"),
        "/tts" => bytes_response("audio/wav", b"RIFF"),
        "/mpeg" => bytes_response("audio/mpeg", b"ID3"),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

fn redirect(location: &str) -> Response {
    if let Ok(response) = Response::builder()
        .status(StatusCode::FOUND)
        .header(header::LOCATION, location)
        .body(Body::empty())
    {
        response
    } else {
        StatusCode::INTERNAL_SERVER_ERROR.into_response()
    }
}

fn bytes_response(content_type: &'static str, body: &'static [u8]) -> Response {
    if let Ok(response) = Response::builder()
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(body))
    {
        response
    } else {
        StatusCode::INTERNAL_SERVER_ERROR.into_response()
    }
}

pub(super) struct Huge {
    pub(super) url: String,
    token: CancellationToken,
    server: tokio::task::JoinHandle<()>,
}

impl Huge {
    pub(super) async fn start() -> Result<Self, String> {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .map_err(|error| error.to_string())?;
        let addr = listener.local_addr().map_err(|error| error.to_string())?;
        let token = CancellationToken::new();
        let stop = token.clone();
        let server = tokio::spawn(async move {
            loop {
                let accepted = tokio::select! {
                    () = stop.cancelled() => break,
                    result = listener.accept() => result,
                };
                let Ok((stream, _)) = accepted else {
                    break;
                };
                let child = stop.clone();
                tokio::spawn(async move { hold_huge(stream, child).await });
            }
        });
        Ok(Self {
            url: format!("http://{addr}/tts"),
            token,
            server,
        })
    }

    pub(super) async fn stop(self) -> Result<(), String> {
        self.token.cancel();
        tokio::time::timeout(Duration::from_secs(5), self.server)
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())?;
        Ok(())
    }
}

async fn hold_huge(mut stream: TcpStream, stop: CancellationToken) {
    let mut buf = [0u8; 512];
    let mut seen = Vec::new();
    loop {
        if seen.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
        if seen.len() > 8192 {
            return;
        }
        let count = match stream.read(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(count) => count,
        };
        let Some(part) = buf.get(..count) else {
            return;
        };
        seen.extend_from_slice(part);
    }
    let head = b"HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nContent-Length: 31457281\r\n\r\n";
    if stream.write_all(head).await.is_err() {
        return;
    }
    stop.cancelled().await;
}
