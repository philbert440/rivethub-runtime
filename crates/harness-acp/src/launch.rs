use std::collections::BTreeMap;
use std::path::PathBuf;

use config::Config;
use config::HarnessSlot;
use strum::IntoEnumIterator as _;
use wire::harness::HarnessId;

use crate::error::AcpError;
use crate::error::ChatUnsupportedReason;
use crate::error::UnverifiedReason;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub working_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcpLaunch {
    Native(AcpCommand),
    Adapter(AcpCommand),
    Unverified {
        command: AcpCommand,
        reason: UnverifiedReason,
    },
}

impl AcpLaunch {
    #[must_use]
    pub fn command(&self) -> &AcpCommand {
        match self {
            Self::Native(command) | Self::Adapter(command) | Self::Unverified { command, .. } => {
                command
            }
        }
    }

    #[must_use]
    pub fn is_verified(&self) -> bool {
        !matches!(self, Self::Unverified { .. })
    }
}

#[derive(Debug, Clone)]
pub struct LaunchTable {
    entries: BTreeMap<HarnessId, LaunchEntry>,
}

#[derive(Debug, Clone)]
enum LaunchEntry {
    Launch(AcpLaunch),
    Unsupported(ChatUnsupportedReason),
}

impl LaunchTable {
    pub fn from_entries(entries: impl IntoIterator<Item = (HarnessId, AcpLaunch)>) -> Self {
        Self {
            entries: entries
                .into_iter()
                .map(|(harness, launch)| (harness, LaunchEntry::Launch(launch)))
                .collect(),
        }
    }

    #[must_use]
    pub fn from_config(config: &Config) -> Self {
        let mut entries = BTreeMap::new();
        for harness in HarnessId::iter() {
            entries.insert(harness, entry_for(config, harness));
        }
        Self { entries }
    }

    pub fn launch(&self, harness: HarnessId) -> Result<AcpLaunch, AcpError> {
        match self.entries.get(&harness) {
            Some(LaunchEntry::Launch(launch)) => Ok(launch.clone()),
            Some(LaunchEntry::Unsupported(reason)) => Err(AcpError::ChatUnsupported {
                harness,
                reason: *reason,
            }),
            None => Err(AcpError::Unconfigured { harness }),
        }
    }

    pub fn verified_command(&self, harness: HarnessId) -> Result<AcpCommand, AcpError> {
        match self.launch(harness)? {
            AcpLaunch::Native(command) | AcpLaunch::Adapter(command) => Ok(command),
            AcpLaunch::Unverified { command, reason } => Err(AcpError::UnverifiedLaunch {
                harness,
                program: command.program.display().to_string(),
                reason,
            }),
        }
    }

    #[must_use]
    pub fn facts(&self) -> Vec<LaunchFact> {
        self.entries
            .iter()
            .map(|(harness, entry)| LaunchFact::from_entry(*harness, entry))
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchFact {
    pub harness: HarnessId,
    pub program: Option<PathBuf>,
    pub verified_chat: bool,
}

impl LaunchFact {
    fn from_entry(harness: HarnessId, entry: &LaunchEntry) -> Self {
        match entry {
            LaunchEntry::Launch(launch) => Self {
                harness,
                program: Some(launch.command().program.clone()),
                verified_chat: sheet_chat(harness),
            },
            LaunchEntry::Unsupported(_) => Self {
                harness,
                program: None,
                verified_chat: false,
            },
        }
    }
}

fn sheet_chat(harness: HarnessId) -> bool {
    matches!(
        harness,
        HarnessId::ClaudeCode | HarnessId::Codex | HarnessId::Opencode
    )
}

fn entry_for(config: &Config, harness: HarnessId) -> LaunchEntry {
    match harness {
        HarnessId::Pi | HarnessId::Cursor => {
            LaunchEntry::Unsupported(ChatUnsupportedReason::TerminalOnly)
        }
        HarnessId::Cowork => LaunchEntry::Unsupported(ChatUnsupportedReason::CaptureOnly),
        HarnessId::GrokBuild => LaunchEntry::Launch(AcpLaunch::Native(acp_command(
            config,
            harness,
            "grok",
            vec!["agent".to_owned(), "stdio".to_owned()],
        ))),
        HarnessId::ClaudeCode => unverified(config, harness, "claude-code-acp", Vec::new()),
        HarnessId::Codex => unverified(config, harness, "codex-acp", Vec::new()),
        HarnessId::Opencode => unverified(config, harness, "opencode", vec!["acp".to_owned()]),
        HarnessId::Hermes => unverified(config, harness, "hermes", Vec::new()),
        HarnessId::KimiCode => unverified(config, harness, "kimi", Vec::new()),
        HarnessId::QwenCode => unverified(config, harness, "qwen", Vec::new()),
    }
}

fn acp_command(
    config: &Config,
    harness: HarnessId,
    program: &str,
    args: Vec<String>,
) -> AcpCommand {
    AcpCommand {
        program: configured(config, harness, program),
        args,
        working_dir: configured_dir(config, harness),
    }
}

fn unverified(
    config: &Config,
    harness: HarnessId,
    program: &str,
    args: Vec<String>,
) -> LaunchEntry {
    LaunchEntry::Launch(AcpLaunch::Unverified {
        command: acp_command(config, harness, program, args),
        reason: UnverifiedReason::NotInstalled,
    })
}

fn configured(config: &Config, harness: HarnessId, default: &str) -> PathBuf {
    config
        .tasks
        .as_ref()
        .and_then(|tasks| tasks.harnesses.as_ref())
        .and_then(|harnesses| harnesses.get(&HarnessSlot::Known(harness)))
        .and_then(|entry| entry.binary.as_ref())
        .map_or_else(
            || PathBuf::from(default),
            |binary| PathBuf::from(binary.trim()),
        )
}

fn configured_dir(config: &Config, harness: HarnessId) -> Option<PathBuf> {
    config
        .tasks
        .as_ref()
        .and_then(|tasks| tasks.harnesses.as_ref())
        .and_then(|harnesses| harnesses.get(&HarnessSlot::Known(harness)))
        .and_then(|entry| entry.cwd.clone())
}
