use crate::environment::Environment;

const DEFAULT_SHARED_DIR: &str = "/rivet-shared";
const DEFAULT_INSTALL_ROOT: &str = "/opt/rivetos";

pub fn shared_dir(env: &Environment) -> String {
    configured_value(env, "RIVETHUB_SHARED_DIR", "RIVETOS_SHARED_DIR")
        .unwrap_or_else(|| DEFAULT_SHARED_DIR.to_owned())
}

pub fn shared_path(env: &Environment, segments: &[&str]) -> String {
    posix_join(&shared_dir(env), segments)
}

pub fn install_root(env: &Environment) -> String {
    configured_value(env, "RIVETHUB_INSTALL_ROOT", "RIVETOS_INSTALL_ROOT")
        .unwrap_or_else(|| DEFAULT_INSTALL_ROOT.to_owned())
}

pub fn install_path(env: &Environment, segments: &[&str]) -> String {
    posix_join(&install_root(env), segments)
}

fn configured_value(env: &Environment, new_name: &str, legacy_name: &str) -> Option<String> {
    let raw = env
        .get(new_name)
        .or_else(|| env.get(legacy_name))?
        .trim()
        .to_owned();
    if raw.is_empty() { None } else { Some(raw) }
}

fn posix_join(root: &str, segments: &[&str]) -> String {
    if segments.is_empty() {
        return root.to_owned();
    }
    let mut path = root.to_owned();
    for segment in segments {
        path.push('/');
        path.push_str(segment);
    }
    posix_normalize(&path)
}

fn posix_normalize(path: &str) -> String {
    let absolute = path.starts_with('/');
    let mut parts: Vec<String> = Vec::new();
    for piece in path.split('/') {
        match piece {
            "" | "." => {}
            ".." => {
                let pop = parts.last().is_some_and(|last| last.as_str() != "..");
                if pop {
                    parts.pop();
                } else if !absolute {
                    parts.push("..".to_owned());
                }
            }
            other => parts.push(other.to_owned()),
        }
    }
    let joined = parts.join("/");
    if absolute {
        format!("/{joined}")
    } else if joined.is_empty() {
        ".".to_owned()
    } else {
        joined
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    home: Option<String>,
}

impl Paths {
    pub fn new(env: &Environment) -> Self {
        Self {
            home: env.home().map(str::to_owned),
        }
    }

    fn base(&self) -> &str {
        self.home.as_deref().unwrap_or(".")
    }

    pub fn home(&self) -> Option<&str> {
        self.home.as_deref()
    }

    pub fn hub_home(&self) -> String {
        format!("{}/.rivethub", self.base())
    }

    pub fn legacy_home(&self) -> String {
        format!("{}/.rivetos", self.base())
    }

    pub fn state_dir(&self) -> String {
        self.hub_home()
    }

    pub fn logs_dir(&self) -> String {
        format!("{}/logs", self.state_dir())
    }

    pub fn env_file(&self) -> String {
        format!("{}/.env", self.state_dir())
    }

    pub fn pid_file(&self) -> String {
        format!("{}/rivethub.pid", self.state_dir())
    }

    pub fn owner_lock(&self, data_dir: &str) -> String {
        format!("{data_dir}/rivethub-owner.lock")
    }

    pub fn config_candidates(&self) -> Vec<(String, bool)> {
        let base = self.base();
        vec![
            (format!("{base}/.rivethub/config.yaml"), false),
            (format!("{base}/.rivethub/config.yml"), false),
            (format!("{base}/.rivetos/config.yaml"), true),
            (format!("{base}/.rivetos/config.yml"), true),
            ("./config.yaml".to_owned(), false),
        ]
    }
}
