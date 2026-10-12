use std::collections::BTreeMap;

#[derive(Debug, Clone, Default)]
pub struct Environment {
    vars: BTreeMap<String, String>,
}

impl Environment {
    pub fn from_vars<V, I>(vars: I) -> Self
    where
        V: Into<String>,
        I: IntoIterator<Item = (V, V)>,
    {
        Self {
            vars: vars
                .into_iter()
                .map(|(key, value)| (key.into(), value.into()))
                .collect(),
        }
    }

    pub fn from_process() -> Self {
        Self {
            vars: std::env::vars_os()
                .map(|(key, value)| {
                    (
                        key.to_string_lossy().into_owned(),
                        value.to_string_lossy().into_owned(),
                    )
                })
                .collect(),
        }
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.vars.get(name).map(String::as_str)
    }

    pub fn home(&self) -> Option<&str> {
        self.get("HOME")
    }

    pub fn term_mux(&self) -> Option<&str> {
        match self
            .get("RIVETHUB_TERM_MUX")
            .or_else(|| self.get("RIVETOS_TERM_MUX"))
        {
            Some("tmux") | None => None,
            Some(value) => Some(value),
        }
    }

    pub fn legacy_notice(&self, new_name: &str, legacy_name: &str) -> Option<String> {
        if self.get(new_name).is_some() || self.get(legacy_name).is_none() {
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
                if self.get(&name).is_some() {
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
                if self.get(&name).is_some() {
                    warnings.push(format!(
                        "{name} is deprecated — terminal injection settings were removed with tmux support and the variable is ignored."
                    ));
                }
            }
        }
        for name in ["RIVETHUB_TERM_MUX", "RIVETOS_TERM_MUX"] {
            if self.get(name) == Some("tmux") {
                warnings.push(format!(
                    "{name}=tmux is ignored — herdr is the only terminal multiplexer and the value is treated as unset."
                ));
            }
        }
        warnings
    }
}
