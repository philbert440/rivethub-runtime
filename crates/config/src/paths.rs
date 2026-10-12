use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

use crate::environment::Environment;

const DEFAULT_SHARED_DIR: &str = "/rivet-shared";
const DEFAULT_INSTALL_ROOT: &str = "/opt/rivetos";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigLocation {
    Current(PathBuf),
    Legacy(PathBuf),
}

impl ConfigLocation {
    pub fn path(&self) -> &Path {
        match self {
            Self::Current(path) | Self::Legacy(path) => path,
        }
    }

    pub fn is_legacy(&self) -> bool {
        matches!(self, Self::Legacy(_))
    }
}

pub fn shared_dir(env: &Environment) -> PathBuf {
    PathBuf::from(
        configured_value(env, "RIVETHUB_SHARED_DIR", "RIVETOS_SHARED_DIR")
            .unwrap_or_else(|| DEFAULT_SHARED_DIR.to_owned()),
    )
}

pub fn shared_path(env: &Environment, segments: &[&str]) -> PathBuf {
    join(&shared_dir(env), segments)
}

pub fn install_root(env: &Environment) -> PathBuf {
    PathBuf::from(
        configured_value(env, "RIVETHUB_INSTALL_ROOT", "RIVETOS_INSTALL_ROOT")
            .unwrap_or_else(|| DEFAULT_INSTALL_ROOT.to_owned()),
    )
}

pub fn install_path(env: &Environment, segments: &[&str]) -> PathBuf {
    join(&install_root(env), segments)
}

fn configured_value(env: &Environment, new_name: &str, legacy_name: &str) -> Option<String> {
    let raw = env
        .text(new_name)
        .or_else(|| env.text(legacy_name))?
        .trim()
        .to_owned();
    if raw.is_empty() { None } else { Some(raw) }
}

fn join(root: &Path, segments: &[&str]) -> PathBuf {
    if segments.is_empty() {
        return root.to_path_buf();
    }
    let mut path = root.to_path_buf();
    for segment in segments {
        path.push(segment);
    }
    normalize(&path)
}

fn normalize(path: &Path) -> PathBuf {
    let absolute = path.is_absolute();
    let mut parts: Vec<&OsStr> = Vec::new();
    for component in path.components() {
        match component {
            Component::RootDir | Component::CurDir => {}
            Component::ParentDir => {
                let pop = parts.last().is_some_and(|last| *last != "..");
                if pop {
                    parts.pop();
                } else if !absolute {
                    parts.push(OsStr::new(".."));
                }
            }
            Component::Prefix(prefix) => parts.push(prefix.as_os_str()),
            Component::Normal(part) => parts.push(part),
        }
    }
    let mut normalized = PathBuf::new();
    if absolute {
        normalized.push(Component::RootDir.as_os_str());
    }
    for part in parts {
        normalized.push(part);
    }
    if !absolute && normalized.as_os_str().is_empty() {
        normalized.push(".");
    }
    normalized
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    home: Option<PathBuf>,
}

impl Paths {
    pub fn new(env: &Environment) -> Self {
        Self {
            home: env.home().map(PathBuf::from),
        }
    }

    fn base(&self) -> &Path {
        self.home.as_deref().unwrap_or_else(|| Path::new("."))
    }

    pub fn home(&self) -> Option<&Path> {
        self.home.as_deref()
    }

    pub fn hub_home(&self) -> PathBuf {
        self.base().join(".rivethub")
    }

    pub fn legacy_home(&self) -> PathBuf {
        self.base().join(".rivetos")
    }

    pub fn state_dir(&self) -> PathBuf {
        self.hub_home()
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.state_dir().join("logs")
    }

    pub fn env_file(&self) -> PathBuf {
        self.state_dir().join(".env")
    }

    pub fn pid_file(&self) -> PathBuf {
        self.state_dir().join("rivethub.pid")
    }

    pub fn owner_lock(&self, data_dir: &Path) -> PathBuf {
        data_dir.join("rivethub-owner.lock")
    }

    pub fn config_candidates(&self) -> Vec<ConfigLocation> {
        let base = self.base();
        vec![
            ConfigLocation::Current(base.join(".rivethub").join("config.yaml")),
            ConfigLocation::Current(base.join(".rivethub").join("config.yml")),
            ConfigLocation::Legacy(base.join(".rivetos").join("config.yaml")),
            ConfigLocation::Legacy(base.join(".rivetos").join("config.yml")),
            ConfigLocation::Current(PathBuf::from("config.yaml")),
        ]
    }
}
