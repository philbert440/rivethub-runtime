pub mod client;
pub mod codec;
pub mod namespace;
pub mod paths;

use serde::Deserialize;
use serde::Serialize;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;
use strum::EnumString;
use tokio::net::UnixStream;

use crate::Error;
use crate::herdr::client::HerdrClient;

pub const PINNED_VERSION: &str = "0.8.2";
pub const DEFAULT_PANE: &str = "w1:p1";
pub const CONFIG_HOME_ENV: &str = "XDG_CONFIG_HOME";
pub const RUNTIME_DIR_ENV: &str = "XDG_RUNTIME_DIR";

pub const CTL_TIMEOUT: Duration = Duration::from_millis(250);
pub const SERVER_OPEN_TIMEOUT: Duration = Duration::from_secs(30);
const SERVER_EXIT_WINDOW: Duration = Duration::from_secs(2);
const SERVER_EXIT_POLL: Duration = Duration::from_millis(25);
pub const PANE_READY_TIMEOUT_MS: u64 = 10_000;
pub const PANE_READY_TIMEOUT: Duration = Duration::from_millis(PANE_READY_TIMEOUT_MS);
pub const WORKSPACE_CREATE_TIMEOUT: Duration = Duration::from_millis(5_000);
pub const SEND_TEXT_TIMEOUT: Duration = Duration::from_millis(2_000);
pub const PANE_BUSY_RETRY: Duration = Duration::from_millis(50);

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, EnumString, strum::Display,
)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum AgentKind {
    Pi,
    Claude,
    Codex,
    Gemini,
    Cursor,
    Devin,
    Agy,
    Cline,
    Omp,
    Mastracode,
    Opencode,
    Copilot,
    Kimi,
    Kiro,
    Droid,
    Amp,
    Grok,
    Hermes,
    Kilo,
    Qodercli,
    Qwen,
    Maki,
}

impl AgentKind {
    #[must_use]
    pub fn is_bare(argv0: &str) -> bool {
        Self::try_from(argv0).is_ok()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneKind {
    Agent(AgentKind),
    Plain,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, EnumString, strum::Display,
)]
pub enum HerdrEventKind {
    #[serde(
        rename = "pane.agent_status_changed",
        alias = "pane_agent_status_changed"
    )]
    #[strum(serialize = "pane.agent_status_changed")]
    PaneAgentStatusChanged,
    #[serde(rename = "pane.agent_detected", alias = "pane_agent_detected")]
    #[strum(serialize = "pane.agent_detected")]
    PaneAgentDetected,
}

#[must_use]
pub fn classify(argv0: &str) -> PaneKind {
    if argv0.contains('/') {
        return PaneKind::Plain;
    }
    match AgentKind::try_from(argv0) {
        Ok(kind) => PaneKind::Agent(kind),
        Err(_) => PaneKind::Plain,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HerdrCli {
    pub binary: PathBuf,
    pub session: String,
}

impl HerdrCli {
    #[must_use]
    pub fn server(&self) -> Vec<String> {
        self.subcommand(&["server"])
    }

    #[must_use]
    pub fn server_stop(&self) -> Vec<String> {
        self.subcommand(&["server", "stop"])
    }

    #[must_use]
    pub fn attach(&self) -> Vec<String> {
        self.subcommand(&[])
    }

    #[must_use]
    pub fn snapshot(&self) -> Vec<String> {
        self.subcommand(&["session", "snapshot"])
    }

    #[must_use]
    pub fn agent_list(&self) -> Vec<String> {
        self.subcommand(&["agent", "list"])
    }

    #[must_use]
    pub fn agent_read(&self, target: &str, lines: u32) -> Vec<String> {
        let mut argv = self.subcommand(&["agent", "read", target]);
        argv.push("--lines".to_owned());
        argv.push(lines.to_string());
        argv
    }

    #[must_use]
    pub fn pane_read(&self, pane: &str, lines: u32) -> Vec<String> {
        let mut argv = self.subcommand(&["pane", "read", pane]);
        argv.push("--lines".to_owned());
        argv.push(lines.to_string());
        argv
    }

    #[must_use]
    pub fn pane_list(&self) -> Vec<String> {
        self.subcommand(&["pane", "list"])
    }

    fn subcommand(&self, rest: &[&str]) -> Vec<String> {
        let mut argv = vec!["--session".to_owned(), self.session.clone()];
        argv.extend(rest.iter().map(ToString::to_string));
        argv
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HerdrBinary {
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeFailure {
    NotFound,
    Version { binary: PathBuf, found: String },
}

impl From<&ProbeFailure> for Error {
    fn from(value: &ProbeFailure) -> Self {
        match value {
            ProbeFailure::NotFound => Self::HerdrNotFound,
            ProbeFailure::Version { binary, found } => Self::HerdrVersion {
                binary: Box::from(binary.as_path()),
                pinned: PINNED_VERSION.to_owned(),
                found: found.clone(),
            },
        }
    }
}

pub fn search_paths(paths: &[PathBuf], home: Option<&Path>) -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = paths.iter().map(|dir| dir.join("herdr")).collect();
    if let Some(home) = home {
        candidates.push(home.join(".local").join("bin").join("herdr"));
    }
    candidates
}

pub fn find_binary(paths: &[PathBuf], home: Option<&Path>) -> Option<HerdrBinary> {
    search_paths(paths, home)
        .into_iter()
        .find(|candidate| candidate.is_file())
        .map(|path| HerdrBinary { path })
}

pub async fn probe(paths: &[PathBuf], home: Option<&Path>) -> Result<HerdrBinary, ProbeFailure> {
    let binary = find_binary(paths, home).ok_or(ProbeFailure::NotFound)?;
    let versioned = tokio::process::Command::new(&binary.path)
        .arg("--version")
        .output()
        .await;
    let Ok(output) = versioned else {
        return Err(ProbeFailure::Version {
            binary: binary.path.clone(),
            found: "unreadable".to_owned(),
        });
    };
    let found = String::from_utf8_lossy(&output.stdout);
    let found = found.lines().next().unwrap_or("").trim().to_owned();
    if found
        .split_whitespace()
        .any(|token| token == PINNED_VERSION)
    {
        Ok(binary)
    } else {
        Err(ProbeFailure::Version {
            binary: binary.path,
            found,
        })
    }
}

#[derive(Clone, Debug)]
pub struct HerdrSession {
    pub cli: HerdrCli,
    pub config_home: PathBuf,
    pub name: String,
    pub pane: String,
    pub client: HerdrClient,
}

impl HerdrSession {
    pub async fn ensure(
        setup: &crate::mux::HerdrSetup,
        key: &crate::session::SessionKey,
        argv: &[String],
        cwd: &std::path::Path,
        entry_env: &std::collections::BTreeMap<String, String>,
        label: &str,
    ) -> Result<Self, Error> {
        let name = paths::session_dir_name(key);
        let cli = HerdrCli {
            binary: setup.binary.clone(),
            session: name.clone(),
        };
        let socket = paths::session_socket(&setup.config_home, &name)?;
        std::fs::create_dir_all(&setup.config_home).map_err(|error| Error::HerdrInit {
            message: error.to_string(),
        })?;
        if !socket.exists() {
            server_command(&cli.binary, &setup.config_home, cli.server())
                .spawn()
                .map_err(|error| Error::HerdrInit {
                    message: error.to_string(),
                })?;
        }
        paths::wait_for_socket(&socket, SERVER_OPEN_TIMEOUT, &name).await?;
        let client = HerdrClient::new(&socket);
        client
            .workspace_create(codec::WorkspaceCreateParams {
                cwd: cwd.to_path_buf(),
                env: entry_env.clone(),
                label: label.to_owned(),
            })
            .await?;
        let Some(argv0) = argv.first() else {
            return Err(Error::Spawn("argv is empty".to_owned()));
        };
        match classify(argv0) {
            PaneKind::Agent(kind) => {
                client
                    .agent_start(codec::AgentStartParams {
                        name: name.clone(),
                        kind,
                        pane_id: DEFAULT_PANE.to_owned(),
                        args: argv.iter().skip(1).cloned().collect(),
                        timeout_ms: PANE_READY_TIMEOUT_MS,
                    })
                    .await?;
                client
                    .events_subscribe(codec::EventsSubscribeParams {
                        subscriptions: vec![
                            codec::SubscriptionParams {
                                event: HerdrEventKind::PaneAgentStatusChanged,
                                pane_id: DEFAULT_PANE.to_owned(),
                            },
                            codec::SubscriptionParams {
                                event: HerdrEventKind::PaneAgentDetected,
                                pane_id: DEFAULT_PANE.to_owned(),
                            },
                        ],
                    })
                    .await?;
            }
            PaneKind::Plain => {
                client
                    .pane_send_text(codec::PaneSendTextParams {
                        pane_id: DEFAULT_PANE.to_owned(),
                        text: format!("{}\n", codec::shell_join(argv)),
                    })
                    .await?;
            }
        }
        Ok(Self {
            cli,
            config_home: setup.config_home.clone(),
            name,
            pane: DEFAULT_PANE.to_owned(),
            client,
        })
    }
    pub fn attach_existing(
        setup: &crate::mux::HerdrSetup,
        key: &crate::session::SessionKey,
    ) -> Result<Self, Error> {
        let name = paths::session_dir_name(key);
        let socket = paths::session_socket(&setup.config_home, &name)?;
        Ok(Self {
            cli: HerdrCli {
                binary: setup.binary.clone(),
                session: name.clone(),
            },
            config_home: setup.config_home.clone(),
            name,
            pane: DEFAULT_PANE.to_owned(),
            client: HerdrClient::new(&socket),
        })
    }

    pub async fn stop(&self) -> Result<(), Error> {
        let mut child = server_command(&self.cli.binary, &self.config_home, self.cli.server_stop())
            .spawn()
            .map_err(|error| Error::HerdrStop {
                session: self.name.clone(),
                message: error.to_string(),
            })?;
        let socket = self.client.socket().to_path_buf();
        let confirm = async {
            child.wait().await.map_err(|error| Error::HerdrStop {
                session: self.name.clone(),
                message: error.to_string(),
            })?;
            wait_for_server_exit(&socket).await;
            Ok::<(), Error>(())
        };
        match tokio::time::timeout(SERVER_EXIT_WINDOW, confirm).await {
            Ok(confirmed) => confirmed?,
            Err(_) => {
                return Err(Error::HerdrStopTimeout {
                    socket: Box::from(self.client.socket()),
                    timeout_ms: paths::duration_ms(SERVER_EXIT_WINDOW),
                    session: self.name.clone(),
                });
            }
        }
        if let Some(dir) = self.client.socket().parent()
            && let Err(error) = std::fs::remove_dir_all(dir)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(
                "herdr session dir {} kept after a clean stop ({error})",
                dir.display()
            );
        }
        Ok(())
    }
}

async fn wait_for_server_exit(socket: &Path) {
    while UnixStream::connect(socket).await.is_ok() {
        tokio::time::sleep(SERVER_EXIT_POLL).await;
    }
}

fn server_command(binary: &Path, config_home: &Path, args: Vec<String>) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(binary);
    command
        .args(args)
        .env(CONFIG_HOME_ENV, config_home)
        .env(RUNTIME_DIR_ENV, config_home)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    command
}
