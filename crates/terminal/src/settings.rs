use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::Path;
use std::path::PathBuf;

use config::Environment;

use crate::session::RosterKey;
use crate::session::SessionKey;

pub const TERM_NAME: &str = "xterm-256color";

#[derive(Debug, Clone)]
pub struct TerminalSettings {
    pub limits: config::TermSettings,
    pub mux: config::TermMuxChoice,
    pub url: Option<String>,
    pub token: Option<String>,
    pub ca_path: PathBuf,
    pub node_name: String,
    pub process_env: Environment,
}

impl TerminalSettings {
    #[must_use]
    pub fn from_config(config: &config::Config, env: &Environment, node_name: String) -> Self {
        let gateway = config.gateway.as_ref();
        Self {
            limits: env.term_settings(),
            mux: env.term_mux_choice(),
            url: gateway.map(gateway_url),
            token: gateway.and_then(|gateway| gateway.token.clone()),
            ca_path: shared_ca_path(env),
            node_name,
            process_env: env.clone(),
        }
    }
    pub fn child_env(
        &self,
        session: Option<&SessionKey>,
        roster_key: &RosterKey,
        cwd: &Path,
        entry_env: &BTreeMap<String, String>,
    ) -> Result<BTreeMap<String, OsString>, crate::Error> {
        let mut env: BTreeMap<String, OsString> = self
            .process_env
            .iter()
            .map(|(name, value)| {
                let name = name
                    .to_str()
                    .ok_or(crate::Error::EnvironmentName)?
                    .to_owned();
                Ok((name, value.clone()))
            })
            .collect::<Result<_, crate::Error>>()?;
        for name in [
            "RIVETHUB_USER_ID",
            "RIVETHUB_USER_TOKEN",
            "RIVETOS_USER_ID",
            "RIVETOS_USER_TOKEN",
        ] {
            env.remove(name);
        }
        for (name, value) in entry_env {
            env.insert(name.clone(), OsString::from(value));
        }
        for name in [
            "RIVETHUB_USER_ID",
            "RIVETHUB_USER_TOKEN",
            "RIVETOS_USER_ID",
            "RIVETOS_USER_TOKEN",
        ] {
            env.remove(name);
        }
        env.insert("TERM".to_owned(), OsString::from(TERM_NAME));
        env.insert("COLORTERM".to_owned(), OsString::from("truecolor"));
        env.insert("LANG".to_owned(), OsString::from("C.UTF-8"));
        env.insert("PWD".to_owned(), cwd.as_os_str().to_owned());
        if let Some(path) = self.local_bin_path(&env) {
            env.insert("PATH".to_owned(), path);
        }
        for (name, value) in self.rivethub_env(session, roster_key) {
            env.insert(name, OsString::from(value));
        }
        Ok(env)
    }

    #[must_use]
    pub fn rivethub_env(
        &self,
        session: Option<&SessionKey>,
        roster_key: &RosterKey,
    ) -> BTreeMap<String, String> {
        let mut env = BTreeMap::new();
        if let Some(session) = session {
            env.insert(
                "RIVETHUB_SESSION_KEY".to_owned(),
                session.as_str().to_owned(),
            );
            env.insert("RIVETHUB_SESSION".to_owned(), session.as_str().to_owned());
        }
        if let Some(url) = &self.url {
            env.insert("RIVETHUB_URL".to_owned(), url.clone());
        }
        if let Some(token) = &self.token {
            env.insert("RIVETHUB_TOKEN".to_owned(), token.clone());
        }
        env.insert(
            "RIVETHUB_CA".to_owned(),
            self.ca_path.to_string_lossy().into_owned(),
        );
        env.insert(
            "RIVETHUB_NAME".to_owned(),
            format!("{}:{roster_key}", self.node_name),
        );
        env
    }

    fn local_bin_path(&self, env: &BTreeMap<String, OsString>) -> Option<OsString> {
        let home = self.process_env.home()?;
        let current = env.get("PATH")?;
        let mut path = current.to_owned();
        path.push(":");
        path.push(home);
        path.push("/.local/bin");
        Some(path)
    }
}

fn gateway_url(gateway: &config::model::GatewaySection) -> String {
    let scheme = if gateway.tls_cert.is_some() || gateway.tls_key.is_some() {
        "https"
    } else {
        "http"
    };
    format!("{scheme}://127.0.0.1:{}", gateway.port)
}

fn shared_ca_path(env: &Environment) -> PathBuf {
    let shared = config::shared_dir(env);
    shared
        .join("rivet-ca")
        .join("intermediate")
        .join("chain.pem")
}
