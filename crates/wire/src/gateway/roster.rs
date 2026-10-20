use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;

use crate::harness::HarnessId;

pub const BUILTIN_DEFAULT_ID: &str = "claude";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltinCommand {
    pub id: &'static str,
    pub label: &'static str,
    pub argv: &'static [&'static str],
    pub session: bool,
}

const BUILTIN: &[BuiltinCommand] = &[
    BuiltinCommand {
        id: "claude",
        label: "Claude Code",
        argv: &["claude"],
        session: true,
    },
    BuiltinCommand {
        id: "grok",
        label: "Grok Build",
        argv: &["grok", "--permission-mode", "bypassPermissions"],
        session: true,
    },
    BuiltinCommand {
        id: "hermes",
        label: "Hermes",
        argv: &["hermes", "--yolo", "--accept-hooks"],
        session: true,
    },
    BuiltinCommand {
        id: "kimi",
        label: "Kimi Code",
        argv: &["kimi", "--yolo"],
        session: true,
    },
    BuiltinCommand {
        id: "codex",
        label: "Codex",
        argv: &["codex"],
        session: true,
    },
    BuiltinCommand {
        id: "opencode",
        label: "OpenCode",
        argv: &["opencode"],
        session: true,
    },
    BuiltinCommand {
        id: "pi",
        label: "Pi",
        argv: &["pi"],
        session: true,
    },
    BuiltinCommand {
        id: "qwen",
        label: "Qwen Code",
        argv: &["qwen", "--approval-mode", "yolo"],
        session: true,
    },
    BuiltinCommand {
        id: "cursor",
        label: "Cursor",
        argv: &["agent", "--force", "--trust"],
        session: true,
    },
    BuiltinCommand {
        id: "shell",
        label: "Shell",
        argv: &["bash", "-l"],
        session: false,
    },
];

#[must_use]
pub fn builtin_commands() -> &'static [BuiltinCommand] {
    BUILTIN
}

#[must_use]
pub fn harness_for_roster_key(key: &str) -> Option<HarnessId> {
    match key {
        "claude" => Some(HarnessId::ClaudeCode),
        "grok" => Some(HarnessId::GrokBuild),
        "hermes" => Some(HarnessId::Hermes),
        "kimi" => Some(HarnessId::KimiCode),
        "codex" => Some(HarnessId::Codex),
        "opencode" => Some(HarnessId::Opencode),
        "pi" => Some(HarnessId::Pi),
        "qwen" => Some(HarnessId::QwenCode),
        "cursor" => Some(HarnessId::Cursor),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionFlags {
    pub pin: Option<&'static str>,
    pub resume: &'static str,
}

#[must_use]
pub fn session_flags(harness: HarnessId) -> Option<SessionFlags> {
    let (pin, resume) = match harness {
        HarnessId::ClaudeCode | HarnessId::GrokBuild | HarnessId::QwenCode => {
            (Some("--session-id"), "--resume")
        }
        HarnessId::Pi => (Some("--session-id"), "--session"),
        HarnessId::Hermes | HarnessId::Cursor => (None, "--resume"),
        HarnessId::KimiCode | HarnessId::Opencode => (None, "--session"),
        HarnessId::Codex => (None, "resume"),
        HarnessId::Cowork => return None,
    };
    Some(SessionFlags { pin, resume })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RosterFile {
    pub default: String,
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    pub commands: BTreeMap<String, RosterEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RosterEntry {
    pub label: String,
    #[serde(rename = "cmd")]
    pub argv: Vec<String>,
    #[serde(default, rename = "room")]
    pub session: Option<bool>,
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}
