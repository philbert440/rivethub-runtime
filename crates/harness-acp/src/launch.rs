use std::collections::BTreeMap;
use std::path::PathBuf;

use config::Config;
use config::HarnessSlot;
use strum::IntoEnumIterator as _;
use wire::harness::HarnessId;

use crate::error::AcpError;
use crate::error::ChatUnsupportedReason;

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
    Unverified(AcpCommand),
}

impl AcpLaunch {
    #[must_use]
    pub fn command(&self) -> &AcpCommand {
        match self {
            Self::Native(command) | Self::Adapter(command) | Self::Unverified(command) => command,
        }
    }

    #[must_use]
    pub fn is_verified(&self) -> bool {
        !matches!(self, Self::Unverified(_))
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
        let launch = self.launch(harness)?;
        if launch.is_verified() {
            return Ok(launch.command().clone());
        }
        Err(AcpError::UnverifiedLaunch {
            harness,
            program: launch.command().program.display().to_string(),
        })
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
                verified_chat: launch.is_verified(),
            },
            LaunchEntry::Unsupported(_) => Self {
                harness,
                program: None,
                verified_chat: false,
            },
        }
    }
}

fn entry_for(config: &Config, harness: HarnessId) -> LaunchEntry {
    match harness {
        HarnessId::Pi | HarnessId::Cursor => {
            LaunchEntry::Unsupported(ChatUnsupportedReason::TerminalOnly)
        }
        HarnessId::Cowork => LaunchEntry::Unsupported(ChatUnsupportedReason::CaptureOnly),
        HarnessId::ClaudeCode => LaunchEntry::Launch(AcpLaunch::Adapter(AcpCommand {
            program: configured(config, harness, "claude-code-acp"),
            args: Vec::new(),
            working_dir: configured_dir(config, harness),
        })),
        HarnessId::Codex => LaunchEntry::Launch(AcpLaunch::Adapter(AcpCommand {
            program: configured(config, harness, "codex-acp"),
            args: Vec::new(),
            working_dir: configured_dir(config, harness),
        })),
        HarnessId::Opencode => LaunchEntry::Launch(AcpLaunch::Native(AcpCommand {
            program: configured(config, harness, "opencode"),
            args: vec!["acp".to_owned()],
            working_dir: configured_dir(config, harness),
        })),
        HarnessId::GrokBuild => LaunchEntry::Launch(AcpLaunch::Unverified(AcpCommand {
            program: configured(config, harness, "grok"),
            args: Vec::new(),
            working_dir: configured_dir(config, harness),
        })),
        HarnessId::Hermes => LaunchEntry::Launch(AcpLaunch::Unverified(AcpCommand {
            program: configured(config, harness, "hermes"),
            args: Vec::new(),
            working_dir: configured_dir(config, harness),
        })),
        HarnessId::KimiCode => LaunchEntry::Launch(AcpLaunch::Unverified(AcpCommand {
            program: configured(config, harness, "kimi"),
            args: Vec::new(),
            working_dir: configured_dir(config, harness),
        })),
        HarnessId::QwenCode => LaunchEntry::Launch(AcpLaunch::Unverified(AcpCommand {
            program: configured(config, harness, "qwen"),
            args: Vec::new(),
            working_dir: configured_dir(config, harness),
        })),
    }
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
