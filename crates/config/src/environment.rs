use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};

use crate::enums::LogLevel;

#[derive(Debug, Clone, Default)]
pub struct Environment {
    vars: BTreeMap<OsString, OsString>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLevelSetting {
    pub level: LogLevel,
    pub ignored: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TermMuxSetting {
    Auto,
    Herdr,
    Direct,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TermMuxChoice {
    pub setting: TermMuxSetting,
    pub ignored: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TermSettings {
    pub max_ptys: usize,
    pub scrollback_bytes: usize,
    pub detached_ttl_ms: u64,
    pub idle_ttl_ms: u64,
    pub exit_linger_ms: u64,
    pub session_gc_ms: u64,
    pub ignored: Vec<String>,
}

impl TermSettings {
    #[must_use]
    pub fn defaults() -> Self {
        Self {
            max_ptys: 4,
            scrollback_bytes: 262_144,
            detached_ttl_ms: 1_800_000,
            idle_ttl_ms: 1_800_000,
            exit_linger_ms: 60_000,
            session_gc_ms: 0,
            ignored: Vec::new(),
        }
    }
}

impl Environment {
    pub fn from_vars<V, I>(vars: I) -> Self
    where
        V: AsRef<OsStr>,
        I: IntoIterator<Item = (V, V)>,
    {
        Self {
            vars: vars
                .into_iter()
                .map(|(key, value)| (key.as_ref().to_owned(), value.as_ref().to_owned()))
                .collect(),
        }
    }

    pub fn from_process() -> Self {
        Self {
            vars: std::env::vars_os().collect(),
        }
    }

    pub fn get(&self, name: &str) -> Option<&OsStr> {
        self.vars.get(OsStr::new(name)).map(OsString::as_os_str)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&OsString, &OsString)> {
        self.vars.iter()
    }

    pub fn text(&self, name: &str) -> Option<&str> {
        self.get(name).and_then(OsStr::to_str)
    }

    pub fn home(&self) -> Option<&str> {
        self.text("HOME")
    }

    #[must_use]
    pub fn log_level(&self) -> LogLevelSetting {
        let stated = self
            .text("RIVETOS_LOG_LEVEL")
            .map(str::trim)
            .filter(|value| !value.is_empty());
        match stated {
            None => LogLevelSetting {
                level: LogLevel::Info,
                ignored: None,
            },
            Some(value) => match value.parse::<LogLevel>() {
                Ok(level) => LogLevelSetting {
                    level,
                    ignored: None,
                },
                Err(_) => LogLevelSetting {
                    level: LogLevel::Info,
                    ignored: Some(value.to_owned()),
                },
            },
        }
    }

    #[must_use]
    pub fn node_name(&self, hostname: &str) -> String {
        ["RIVETOS_DEN_NODE_NAME", "RIVETOS_DEN_NODE_ID"]
            .into_iter()
            .find_map(|name| {
                self.text(name)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
            })
            .map_or_else(|| local_node_name(hostname), str::to_owned)
    }

    pub(crate) fn postgres_url(&self) -> Option<String> {
        self.text("RIVETOS_PG_URL")
            .map(str::trim)
            .filter(|url| !url.is_empty())
            .map(str::to_owned)
    }

    pub fn term_mux(&self) -> Option<&str> {
        match self
            .text("RIVETHUB_TERM_MUX")
            .or_else(|| self.text("RIVETOS_TERM_MUX"))
        {
            Some("tmux") | None => None,
            Some(value) => Some(value),
        }
    }

    pub fn term_mux_choice(&self) -> TermMuxChoice {
        let stated = self
            .text("RIVETHUB_TERM_MUX")
            .or_else(|| self.text("RIVETOS_TERM_MUX"))
            .map(str::trim);
        match stated {
            None | Some("tmux") => TermMuxChoice {
                setting: TermMuxSetting::Auto,
                ignored: None,
            },
            Some("herdr") => TermMuxChoice {
                setting: TermMuxSetting::Herdr,
                ignored: None,
            },
            Some("none") => TermMuxChoice {
                setting: TermMuxSetting::Direct,
                ignored: None,
            },
            Some(value) => TermMuxChoice {
                setting: TermMuxSetting::Direct,
                ignored: Some(value.to_owned()),
            },
        }
    }

    pub fn term_settings(&self) -> TermSettings {
        let mut settings = TermSettings::defaults();
        let mut ignored = Vec::new();
        match self.term_value::<usize>("RIVETHUB_TERM_MAX", "RIVETOS_DEN_TERM_MAX") {
            Ok(Some(value)) => settings.max_ptys = value,
            Ok(None) => {}
            Err(entry) => ignored.push(entry),
        }
        match self.term_value::<usize>("RIVETHUB_TERM_SCROLLBACK", "RIVETOS_DEN_TERM_SCROLLBACK") {
            Ok(Some(value)) => settings.scrollback_bytes = value,
            Ok(None) => {}
            Err(entry) => ignored.push(entry),
        }
        match self.term_value::<u64>(
            "RIVETHUB_TERM_DETACHED_TTL_MS",
            "RIVETOS_DEN_TERM_DETACHED_TTL_MS",
        ) {
            Ok(Some(value)) => settings.detached_ttl_ms = value,
            Ok(None) => {}
            Err(entry) => ignored.push(entry),
        }
        match self.term_value::<u64>("RIVETHUB_TERM_IDLE_TTL_MS", "RIVETOS_DEN_TERM_IDLE_TTL_MS") {
            Ok(Some(value)) => settings.idle_ttl_ms = value,
            Ok(None) => {}
            Err(entry) => ignored.push(entry),
        }
        match self.term_value::<u64>(
            "RIVETHUB_TERM_EXIT_LINGER_MS",
            "RIVETOS_DEN_TERM_EXIT_LINGER_MS",
        ) {
            Ok(Some(value)) => settings.exit_linger_ms = value,
            Ok(None) => {}
            Err(entry) => ignored.push(entry),
        }
        match self.term_value::<u64>(
            "RIVETHUB_TERM_SESSION_GC_MS",
            "RIVETOS_DEN_TERM_SESSION_GC_MS",
        ) {
            Ok(Some(value)) => settings.session_gc_ms = value,
            Ok(None) => {}
            Err(entry) => ignored.push(entry),
        }
        settings.ignored = ignored;
        settings
    }

    fn term_value<T: std::str::FromStr>(
        &self,
        new: &str,
        legacy: &str,
    ) -> Result<Option<T>, String> {
        let Some(text) = self
            .text(new)
            .or_else(|| self.text(legacy))
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return Ok(None);
        };
        text.parse::<T>()
            .map(Some)
            .map_err(|_| format!("{new}={text}"))
    }

    pub fn legacy_notice(&self, new_name: &str, legacy_name: &str) -> Option<String> {
        if self.text(new_name).is_some() || self.text(legacy_name).is_none() {
            return None;
        }
        Some(format!(
            "{legacy_name} is deprecated — rename it to {new_name}; the legacy name is read only while {new_name} is unset."
        ))
    }

    pub fn legacy_notices(&self) -> Vec<String> {
        let pairs = [
            ("RIVETHUB_SHARED_DIR", "RIVETOS_SHARED_DIR"),
            ("RIVETHUB_INSTALL_ROOT", "RIVETOS_INSTALL_ROOT"),
            ("RIVETHUB_TERM_MUX", "RIVETOS_TERM_MUX"),
            ("RIVETHUB_TERM_MAX", "RIVETOS_DEN_TERM_MAX"),
            ("RIVETHUB_TERM_SCROLLBACK", "RIVETOS_DEN_TERM_SCROLLBACK"),
            (
                "RIVETHUB_TERM_DETACHED_TTL_MS",
                "RIVETOS_DEN_TERM_DETACHED_TTL_MS",
            ),
            ("RIVETHUB_TERM_IDLE_TTL_MS", "RIVETOS_DEN_TERM_IDLE_TTL_MS"),
            (
                "RIVETHUB_TERM_EXIT_LINGER_MS",
                "RIVETOS_DEN_TERM_EXIT_LINGER_MS",
            ),
            (
                "RIVETHUB_TERM_SESSION_GC_MS",
                "RIVETOS_DEN_TERM_SESSION_GC_MS",
            ),
        ];
        pairs
            .into_iter()
            .filter_map(|(new_name, legacy_name)| self.legacy_notice(new_name, legacy_name))
            .collect()
    }

    pub fn deprecation_warnings(&self) -> Vec<String> {
        let mut warnings = Vec::new();
        for tool in ["SHELL", "FILE", "SEARCH"] {
            for prefix in ["RIVETHUB", "RIVETOS"] {
                let name = format!("{prefix}_MCP_ENABLE_{tool}");
                if self.text(&name).is_some() {
                    warnings.push(format!(
                        "{name} is deprecated — built-in {} tools were removed (every harness ships its own) and the variable is ignored.",
                        tool.to_lowercase(),
                    ));
                }
            }
        }
        for suffix in [
            "INJECT_READY_MS",
            "INJECT_READY_MAX_MS",
            "INJECT_CONFIRM_MS",
            "INJECT_SUBMIT_DELAY_MS",
            "TMUX_USER_CONF",
        ] {
            for prefix in ["RIVETHUB", "RIVETOS"] {
                let name = format!("{prefix}_{suffix}");
                if self.text(&name).is_some() {
                    warnings.push(format!(
                        "{name} is deprecated — terminal injection settings were removed with tmux support and the variable is ignored."
                    ));
                }
            }
        }
        for name in ["RIVETHUB_TERM_MUX", "RIVETOS_TERM_MUX"] {
            if self.text(name) == Some("tmux") {
                warnings.push(format!(
                    "{name}=tmux is ignored — herdr is the only terminal multiplexer and the value is treated as unset."
                ));
            }
        }
        warnings
    }
}

#[must_use]
fn local_node_name(hostname: &str) -> String {
    let trimmed = hostname.trim();
    if trimmed.is_empty() {
        "local".to_owned()
    } else {
        trimmed.to_owned()
    }
}
