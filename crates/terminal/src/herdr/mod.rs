pub mod client;
pub mod codec;
pub mod namespace;
pub mod paths;
pub mod turns;
pub mod ui;

use serde::Deserialize;
use serde::Serialize;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;
use strum::EnumString;
use strum::IntoEnumIterator;
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
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    EnumString,
    strum::Display,
    strum::EnumIter,
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
    pub const fn reports_turns(self) -> bool {
        matches!(self, Self::Codex | Self::Opencode | Self::Claude)
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
    AgentKind::iter()
        .find(|kind| kind.to_string() == argv0)
        .map_or(PaneKind::Plain, PaneKind::Agent)
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

#[derive(Debug, thiserror::Error)]
pub enum ProbeFailure {
    #[error("herdr not found on PATH or in ~/.local/bin")]
    NotFound,
    #[error(
        "herdr at {binary} is not {PINNED_VERSION} (RivetHub pins {PINNED_VERSION} / protocol 20)"
    )]
    Version { binary: PathBuf, found: String },
    #[error("cannot execute herdr at {binary}: {source}")]
    Execute {
        binary: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("cannot inspect herdr at {binary}: {source}")]
    Inspect {
        binary: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("herdr at {binary} exited with {status}")]
    Status {
        binary: PathBuf,
        status: std::process::ExitStatus,
    },
    #[error("herdr version is not UTF-8: {0}")]
    Encoding(#[from] std::string::FromUtf8Error),
}

fn search_paths(paths: &[PathBuf], home: Option<&Path>) -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = paths.iter().map(|dir| dir.join("herdr")).collect();
    if let Some(home) = home {
        candidates.push(home.join(".local").join("bin").join("herdr"));
    }
    candidates
}

async fn find_binary(paths: &[PathBuf], home: Option<&Path>) -> Result<HerdrBinary, ProbeFailure> {
    for path in search_paths(paths, home) {
        match tokio::fs::metadata(&path).await {
            Ok(metadata) if metadata.is_file() => return Ok(HerdrBinary { path }),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(ProbeFailure::Inspect {
                    binary: path,
                    source,
                });
            }
        }
    }
    Err(ProbeFailure::NotFound)
}

pub async fn probe(paths: &[PathBuf], home: Option<&Path>) -> Result<HerdrBinary, ProbeFailure> {
    let binary = find_binary(paths, home).await?;
    let output = tokio::process::Command::new(&binary.path)
        .arg("--version")
        .output()
        .await
        .map_err(|source| ProbeFailure::Execute {
            binary: binary.path.clone(),
            source,
        })?;
    if !output.status.success() {
        return Err(ProbeFailure::Status {
            binary: binary.path,
            status: output.status,
        });
    }
    let found = String::from_utf8(output.stdout)?.trim().to_owned();
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
        tokio::fs::create_dir_all(&setup.config_home).await?;
        let mut server = None;
        if !tokio::fs::try_exists(&socket).await? {
            let said = paths::server_errors(&socket);
            if let Some(dir) = said.parent() {
                tokio::fs::create_dir_all(dir).await?;
            }
            let stderr = std::fs::File::create(&said)?;
            let child = server_command(&cli.binary, &setup.config_home, cli.server())
                .stderr(stderr)
                .spawn()
                .map_err(Error::Io)?;
            server = Some(paths::Starting { child, said });
        }
        paths::wait_for_socket(&socket, SERVER_OPEN_TIMEOUT, &name, server.as_mut()).await?;
        let client = HerdrClient::new(&socket);
        client
            .workspace_create(codec::WorkspaceCreateParams {
                cwd: cwd.to_path_buf(),
                env: entry_env.clone(),
                label: label.to_owned(),
            })
            .await?;
        let Some(argv0) = argv.first() else {
            return Err(Error::EmptyArgv);
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
        Ok(Self {
            cli,
            config_home: setup.config_home.clone(),
            name,
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
            client: HerdrClient::new(&socket),
        })
    }

    pub async fn stop(&self) -> Result<(), Error> {
        let mut child = server_command(&self.cli.binary, &self.config_home, self.cli.server_stop())
            .spawn()
            .map_err(Error::Io)?;
        let socket = self.client.socket().to_path_buf();
        let confirm = async {
            let status = child.wait().await?;
            if !status.success() {
                return Err(Error::StopStatus(status));
            }
            wait_for_server_exit(&socket).await?;
            Ok::<(), Error>(())
        };
        match tokio::time::timeout(SERVER_EXIT_WINDOW, confirm).await {
            Ok(confirmed) => confirmed?,
            Err(_) => {
                return Err(Error::HerdrStopTimeout {
                    socket: Box::from(self.client.socket()),
                    timeout_ms: SERVER_EXIT_WINDOW.as_millis(),
                    session: self.name.clone(),
                });
            }
        }
        if let Some(dir) = self.client.socket().parent()
            && let Err(error) = tokio::fs::remove_dir_all(dir).await
            && error.kind() != std::io::ErrorKind::NotFound
        {
            return Err(Error::Io(error));
        }
        Ok(())
    }
}

async fn wait_for_server_exit(socket: &Path) -> Result<(), Error> {
    loop {
        match UnixStream::connect(socket).await {
            Ok(_) => tokio::time::sleep(SERVER_EXIT_POLL).await,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
                ) =>
            {
                return Ok(());
            }
            Err(error) => return Err(Error::Io(error)),
        }
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
