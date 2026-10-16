use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};

use config::Environment;
use kernel::ca::Principal;
use tokio_util::sync::CancellationToken;
use url::Url;

use super::check::{self, Target};
use super::store::{self, Draft};
use super::{EffectAction, Finished, finish};
use crate::cert::{self, RenewArgs};
use crate::{Error, Report};

const DEFAULT_ROSTER: &str = "/rivet-shared/rivet-ca/mesh-nodes.conf";
const ENROLL_USAGE: &str = "Usage: rh mesh enroll <user@host> --name <node> [--advertise <host|ip>] [--hub-cmd rivethub-hub]\n";
const RENEW_USAGE: &str =
    "Usage: rh mesh renew <user@host> --name <node> [--hub-cmd rivethub-hub]\n";

pub(super) struct Enrollment<'a> {
    pub(super) args: &'a super::EnrollArgs,
    pub(super) ssh_user: Option<String>,
    pub(super) json: bool,
    pub(super) env: &'a Environment,
    pub(super) shutdown: &'a CancellationToken,
}

pub(super) struct Renewal<'a> {
    pub(super) args: &'a super::RenewArgs,
    pub(super) json: bool,
    pub(super) env: &'a Environment,
    pub(super) shutdown: &'a CancellationToken,
}

struct Material {
    url: Url,
    root: PathBuf,
    password: PathBuf,
    roster: PathBuf,
}

pub(super) async fn enroll(request: Enrollment<'_>) -> Result<Report, Error> {
    let Some(spec) = request.args.target.as_deref() else {
        return Ok(check::usage(ENROLL_USAGE));
    };
    let target = check::target(spec)?;
    let name = required_name(request.args.name.as_deref())?;
    check::hub_cmd(&request.args.hub_cmd)?;
    let host = check::advertise(request.args.advertise.as_deref())?;
    let roster = roster_path(request.args.roster.clone());
    let material = enroll_material(request.args, &roster)?;
    let state = prepare_state(request.args.state.clone(), request.env).await?;
    let agents = sign_node(&material, &name, &state, request.env, request.shutdown).await?;
    if request.shutdown.is_cancelled() {
        return Ok(Report::success(""));
    }
    let mesh = store::mesh_path(request.env).await?;
    let registry = store::open_writable(mesh.clone()).await?;
    let saved = store::save(
        &registry,
        store::node(Draft {
            name: name.clone(),
            host,
            agents,
            ssh_user: request.ssh_user,
        })?,
        request.shutdown,
    )
    .await?;
    let message = enroll_message(&name, &target, &state);
    finish(Finished {
        action: EffectAction::Enrolled,
        node: Some(name),
        state: Some(state),
        mesh,
        message,
        warnings: saved.warning.into_iter().collect(),
        json: request.json,
    })
}

pub(super) async fn renew(request: Renewal<'_>) -> Result<Report, Error> {
    let Some(spec) = request.args.target.as_deref() else {
        return Ok(check::usage(RENEW_USAGE));
    };
    let target = check::target(spec)?;
    let name = required_name(request.args.name.as_deref())?;
    check::hub_cmd(&request.args.hub_cmd)?;
    let Some(url) = request.args.ca_url.clone() else {
        return Err(Error::StepCaRenew);
    };
    let Some(root) = request.args.root.clone() else {
        return Err(Error::StepCaRenew);
    };
    let state = prepare_state(request.args.state.clone(), request.env).await?;
    let root = crate::ca::authority_root(root, url.clone()).await?;
    cert::renew(
        renew_args(name.clone(), url, root),
        state.clone(),
        request.shutdown,
    )
    .await?;
    if request.shutdown.is_cancelled() {
        return Ok(Report::success(""));
    }
    let mesh = store::mesh_path(request.env).await?;
    let message = renew_message(&name, &target, &state);
    finish(Finished {
        action: EffectAction::Renewed,
        node: Some(name),
        state: Some(state),
        mesh,
        message,
        warnings: Vec::new(),
        json: request.json,
    })
}

fn required_name(value: Option<&str>) -> Result<String, Error> {
    let Some(name) = value else {
        return Err(Error::NameRequired);
    };
    check::node_name(name)?;
    Ok(name.to_owned())
}

fn roster_path(explicit: Option<PathBuf>) -> PathBuf {
    match explicit {
        Some(path) => path,
        None => PathBuf::from(DEFAULT_ROSTER),
    }
}

fn enroll_material(args: &super::EnrollArgs, roster: &Path) -> Result<Material, Error> {
    let Some(url) = args.ca_url.clone() else {
        return Err(missing_enroll(roster));
    };
    let Some(root) = args.root.clone() else {
        return Err(missing_enroll(roster));
    };
    let Some(password) = args.provisioner_password_file.clone() else {
        return Err(missing_enroll(roster));
    };
    Ok(Material {
        url,
        root,
        password,
        roster: roster.to_path_buf(),
    })
}

fn missing_enroll(roster: &Path) -> Error {
    Error::StepCaEnroll {
        roster: roster.display().to_string(),
    }
}

async fn sign_node(
    material: &Material,
    name: &str,
    state: &Path,
    env: &Environment,
    shutdown: &CancellationToken,
) -> Result<Vec<String>, Error> {
    let roster = crate::ca::roster(material.roster.clone()).await?;
    let plan = pki::plan_cli(pki::CliToken::Node(name.to_owned()), Some(&roster))?;
    let agents = roster_agents(&roster, name)?;
    let root = crate::ca::authority_root(material.root.clone(), material.url.clone()).await?;
    let password = crate::ca::password_file(material.password.clone()).await?;
    let minted = pki::mint(
        &plan,
        &pki::Endpoint {
            url: material.url.clone(),
            root: root.clone(),
            password_file: password,
        },
        &pki::ProcessEnv::from_pairs(env.iter().map(|(key, value)| (key.clone(), value.clone()))),
        shutdown,
    )
    .await?;
    let token = write_token(state, &minted.token).await?;
    let renewed = cert::renew(
        renew_args_token(name, material.url.clone(), root, token.clone()),
        state.to_path_buf(),
        shutdown,
    )
    .await;
    let removed = remove_token(token).await;
    renewed?;
    removed?;
    Ok(agents)
}

fn roster_agents(roster: &pki::Roster, name: &str) -> Result<Vec<String>, Error> {
    match roster.agent_for_node(name)? {
        Some(agent) => Ok(vec![agent.to_owned()]),
        None => Ok(Vec::new()),
    }
}

fn renew_args(name: String, url: Url, root: PathBuf) -> RenewArgs {
    RenewArgs {
        principal: Principal::Node { node: name },
        ca_url: url,
        trust_root: root,
        token_file: None,
        state: None,
        once: true,
    }
}

fn renew_args_token(name: &str, url: Url, root: PathBuf, token: PathBuf) -> RenewArgs {
    RenewArgs {
        principal: Principal::Node {
            node: name.to_owned(),
        },
        ca_url: url,
        trust_root: root,
        token_file: Some(token),
        state: None,
        once: true,
    }
}

async fn prepare_state(explicit: Option<PathBuf>, env: &Environment) -> Result<PathBuf, Error> {
    let path = match explicit {
        Some(path) => path,
        None => config::Paths::new(env)?.state_dir(),
    };
    tokio::task::spawn_blocking(move || prepare_state_path(&path)).await?
}

fn prepare_state_path(path: &Path) -> Result<PathBuf, Error> {
    if !path.is_absolute() || path.components().any(|part| part == Component::ParentDir) {
        return Err(Error::StatePath);
    }
    let normal = config::normalize(path);
    reject_shared(&normal)?;
    ensure_directory(&normal)?;
    let canonical = fs::canonicalize(&normal)?;
    if canonical != normal {
        return Err(Error::PkiInput(
            "holder path resolves outside its directory",
        ));
    }
    reject_shared(&canonical)?;
    Ok(canonical)
}

fn reject_shared(path: &Path) -> Result<(), Error> {
    let shared = Path::new("/rivet-shared");
    if path == shared || path.starts_with(shared) {
        Err(Error::SharedKey)
    } else {
        Ok(())
    }
}

fn ensure_directory(path: &Path) -> Result<(), Error> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(Error::PkiInput("symlinks are forbidden in the holder path"));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::DirBuilder::new().mode(0o700).create(&current)?;
                fs::set_permissions(&current, fs::Permissions::from_mode(0o700))?;
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

async fn write_token(directory: &Path, token: &str) -> Result<PathBuf, Error> {
    let directory = directory.to_path_buf();
    let token = token.to_owned();
    tokio::task::spawn_blocking(move || write_token_file(&directory, &token)).await?
}

fn write_token_file(directory: &Path, token: &str) -> Result<PathBuf, Error> {
    let path = directory.join("enrollment.token");
    if !path.starts_with(directory) {
        return Err(Error::PkiInput(
            "enrollment token escapes its state directory",
        ));
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(nix::fcntl::OFlag::O_NOFOLLOW.bits())
        .open(&path)?;
    file.write_all(token.as_bytes())?;
    file.sync_all()?;
    Ok(path)
}

async fn remove_token(path: PathBuf) -> Result<(), Error> {
    tokio::task::spawn_blocking(move || match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(Error::from(error)),
    })
    .await?
}

fn enroll_message(name: &str, target: &Target, state: &Path) -> String {
    format!(
        "Enrolled {name} through step-ca.\n\
The hub address {}@{} was checked and not contacted.\n\
The one-time token came from the roster and POST /1.0/sign issued the certificate.\n\
The holder key is in {}/tls.\n\
rivethub-hub and rivet-ca.sh were not run.\n\
Private keys were not written to /rivet-shared.\n\
The node is recorded in mesh.json.\n",
        target.user,
        target.host,
        state.display()
    )
}

fn renew_message(name: &str, target: &Target, state: &Path) -> String {
    format!(
        "Renewed {name} through step-ca mTLS (rh cert renew).\n\
The hub address {}@{} was checked and not contacted.\n\
rivet-ca.sh is retired.\n\
The holder key stayed in {}/tls.\n\
mesh.json was not rewritten.\n",
        target.user,
        target.host,
        state.display()
    )
}
