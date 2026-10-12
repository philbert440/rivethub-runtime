use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};

#[derive(Debug, Clone, Default)]
pub struct Environment {
    vars: BTreeMap<OsString, OsString>,
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

    pub fn text(&self, name: &str) -> Option<&str> {
        self.get(name).and_then(OsStr::to_str)
    }

    pub fn home(&self) -> Option<&str> {
        self.text("HOME")
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
