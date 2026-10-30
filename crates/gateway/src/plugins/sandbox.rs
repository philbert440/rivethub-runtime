use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use strum::IntoEnumIterator as _;
use wire::plugin::manifest::Permission;

const RESOLVER: &str = "/etc/resolv.conf";

#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::EnumIter)]
enum SystemPath {
    Usr,
    Bin,
    Sbin,
    Lib,
    Lib32,
    Lib64,
    Etc,
    Proc,
    Sys,
    DevNull,
    DevZero,
    DevRandom,
    DevUrandom,
}

impl SystemPath {
    const fn path(self) -> &'static str {
        match self {
            Self::Usr => "/usr",
            Self::Bin => "/bin",
            Self::Sbin => "/sbin",
            Self::Lib => "/lib",
            Self::Lib32 => "/lib32",
            Self::Lib64 => "/lib64",
            Self::Etc => "/etc",
            Self::Proc => "/proc",
            Self::Sys => "/sys",
            Self::DevNull => "/dev/null",
            Self::DevZero => "/dev/zero",
            Self::DevRandom => "/dev/random",
            Self::DevUrandom => "/dev/urandom",
        }
    }

    const fn writable(self) -> bool {
        matches!(self, Self::DevNull)
    }
}

pub(super) const HOST_PERMISSIONS: [Permission; 4] = [
    Permission::HostFilesRead,
    Permission::HostFilesWrite,
    Permission::HostNetwork,
    Permission::HostExec,
];

#[derive(Debug, Clone)]
pub(super) struct Folders {
    pub(super) install: PathBuf,
    pub(super) data: PathBuf,
    pub(super) run: PathBuf,
    pub(super) api: PathBuf,
}

#[derive(Debug, Clone)]
pub(super) struct Request {
    pub(super) permissions: Vec<Permission>,
    pub(super) folders: Folders,
    pub(super) program: PathBuf,
    pub(super) search: Option<std::ffi::OsString>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Policy {
    pub(super) read: Vec<PathBuf>,
    pub(super) write: Vec<PathBuf>,
    pub(super) own: Vec<PathBuf>,
    pub(super) connect: Vec<PathBuf>,
    pub(super) network: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Confinement {
    Full,
    Partial,
    Unconfined,
}

impl Confinement {
    pub(super) const fn describe(self) -> &'static str {
        match self {
            Self::Full => "confined by Landlock",
            Self::Partial => "partly confined by Landlock: this kernel lacks some of its rules",
            Self::Unconfined => "not confined: this kernel has no Landlock",
        }
    }
}

pub(super) fn asks_host(requested: &[Permission]) -> Option<Permission> {
    requested
        .iter()
        .copied()
        .find(|permission| HOST_PERMISSIONS.contains(permission))
}

pub(super) fn unavailable(permission: Permission) -> String {
    format!(
        "this node's kernel has no Landlock, so rh cannot confine a plug-in that asks for {permission}; it cannot be enabled here"
    )
}

#[must_use]
pub(super) fn policy(
    permissions: &[Permission],
    folders: &Folders,
    program: Option<&Path>,
    resolver: Option<&Path>,
) -> Policy {
    let granted = |permission| permissions.contains(&permission);
    let network = granted(Permission::HostNetwork);
    let mut read: Vec<PathBuf> = SystemPath::iter()
        .map(|system| PathBuf::from(system.path()))
        .collect();
    read.push(folders.install.clone());
    if let Some(program) = program {
        read.push(program.to_path_buf());
        if let Some(prefix) = installation(program) {
            read.push(prefix);
        }
    }
    if network && let Some(resolver) = resolver {
        read.push(resolver.to_path_buf());
    }
    if granted(Permission::HostFilesRead) {
        read.push(PathBuf::from("/"));
    }
    let mut write: Vec<PathBuf> = SystemPath::iter()
        .filter(|system| system.writable())
        .map(|system| PathBuf::from(system.path()))
        .collect();
    if granted(Permission::HostFilesWrite) {
        write.push(PathBuf::from("/"));
    }
    Policy {
        read,
        write,
        own: vec![folders.data.clone(), folders.run.clone()],
        connect: vec![folders.api.clone()],
        network,
    }
}

fn installation(program: &Path) -> Option<PathBuf> {
    let folder = program.parent()?;
    (folder.file_name() == Some(OsStr::new("bin")))
        .then(|| folder.parent().map(Path::to_path_buf))
        .flatten()
}

fn located(program: &Path, search: Option<&OsStr>) -> Result<PathBuf, String> {
    let found = if program.components().count() > 1 {
        program.to_path_buf()
    } else {
        search
            .into_iter()
            .flat_map(std::env::split_paths)
            .map(|folder| folder.join(program))
            .find(|candidate| candidate.is_file())
            .ok_or_else(|| format!("{} is not on PATH", program.display()))?
    };
    std::fs::canonicalize(&found).map_err(|error| format!("{}: {error}", found.display()))
}

pub(super) fn resolve(request: &Request) -> Result<Policy, String> {
    let program = located(&request.program, request.search.as_deref())?;
    #[cfg_attr(
        dylint_lib = "rr_lints",
        expect(
            fallback,
            reason = "PATTERNS FALLBACK: a real absence, a node without a resolver configuration has none for host.network to allow"
        )
    )]
    let resolver = std::fs::canonicalize(RESOLVER).ok();
    Ok(policy(
        &request.permissions,
        &request.folders,
        Some(&program),
        resolver.as_deref(),
    ))
}

#[cfg(target_os = "linux")]
mod landlock_rules {
    use landlock::{
        ABI, Access, AccessFs, AccessNet, CompatLevel, Compatible as _, Ruleset, RulesetAttr as _,
        RulesetCreatedAttr as _, RulesetError, RulesetStatus, Scope, path_beneath_rules,
    };

    use super::{Confinement, Policy};

    const ABI_KNOWN: ABI = ABI::V9;

    pub(in crate::plugins) fn available() -> bool {
        Ruleset::default()
            .set_compatibility(CompatLevel::HardRequirement)
            .handle_access(AccessFs::from_all(ABI::V1))
            .and_then(Ruleset::create)
            .is_ok()
    }

    pub(in crate::plugins) fn restrict(policy: &Policy) -> Result<Confinement, RulesetError> {
        let abi = ABI_KNOWN;
        let mut ruleset = Ruleset::default().handle_access(AccessFs::from_all(abi))?;
        if !policy.network {
            ruleset = ruleset.handle_access(AccessNet::from_all(abi))?;
        }
        let scope = if policy.network {
            Scope::Signal.into()
        } else {
            Scope::from_all(abi)
        };
        let status = ruleset
            .scope(scope)?
            .create()?
            .add_rules(path_beneath_rules(&policy.read, AccessFs::from_read(abi)))?
            .add_rules(path_beneath_rules(&policy.write, AccessFs::from_write(abi)))?
            .add_rules(path_beneath_rules(&policy.own, AccessFs::from_all(abi)))?
            .add_rules(path_beneath_rules(&policy.connect, AccessFs::ResolveUnix))?
            .restrict_self()?;
        Ok(match status.ruleset {
            RulesetStatus::FullyEnforced => Confinement::Full,
            RulesetStatus::PartiallyEnforced => Confinement::Partial,
            RulesetStatus::NotEnforced => Confinement::Unconfined,
        })
    }
}

#[cfg(target_os = "linux")]
pub(super) use landlock_rules::{available, restrict};

#[cfg(not(target_os = "linux"))]
pub(super) const fn available() -> bool {
    false
}

#[cfg(not(target_os = "linux"))]
pub(super) fn restrict(_policy: &Policy) -> Result<Confinement, std::convert::Infallible> {
    Ok(Confinement::Unconfined)
}
