mod server;

use std::ffi::{OsStr, OsString};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use cli::{Cli, Dispatch, Report};
use config::Environment;
use tokio_util::sync::CancellationToken;

pub(crate) use server::Replay;

pub(crate) type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

const STEP: &str = r#"#!/bin/sh
{
  printf '%s\n' "$0"
  for arg do
    printf '%s\n' "$arg"
  done
} > "$RIVETHUB_TEST_ARGV"
if [ -n "${RIVETHUB_TEST_PAUSE:-}" ]; then
  read -r _ < "$RIVETHUB_TEST_PAUSE"
fi
if [ "${RIVETHUB_TEST_FAIL:-}" = 1 ]; then
  printf '%s\n' 'provisioner password rejected' >&2
  exit 1
fi
if [ "${RIVETHUB_TEST_FAIL:-}" = blank ]; then
  printf '\n'
  exit 0
fi
printf '%s\n' 'one-time-token'
"#;

pub(crate) struct Fixture {
    pub(crate) home: tempfile::TempDir,
    pub(crate) bin: PathBuf,
    pub(crate) argv: PathBuf,
    pub(crate) root: PathBuf,
    pub(crate) password: PathBuf,
    pub(crate) roster: PathBuf,
}

impl Fixture {
    pub(crate) fn new() -> Result<Self> {
        let home = tempfile::tempdir()?;
        let bin = home.path().join("bin");
        std::fs::create_dir(&bin)?;
        let step = bin.join("step");
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o755)
            .open(&step)?;
        std::io::Write::write_all(&mut file, STEP.as_bytes())?;
        let root = home.path().join("root.pem");
        write_root(&root)?;
        let password = home.path().join("provisioner.password");
        std::fs::write(&password, "secret\n")?;
        let roster = home.path().join("roster.txt");
        std::fs::copy(roster_fixture(), &roster)?;
        Ok(Self {
            argv: home.path().join("argv.txt"),
            home,
            bin,
            root,
            password,
            roster,
        })
    }

    pub(crate) fn environment(&self) -> Environment {
        Environment::from_vars([
            pair("HOME", self.home.path().as_os_str()),
            pair("PATH", self.bin.as_os_str()),
            pair("RIVETHUB_TEST_ARGV", self.argv.as_os_str()),
        ])
    }

    pub(crate) async fn run(
        &self,
        args: impl IntoIterator<Item = impl AsRef<OsStr>>,
    ) -> Result<Report> {
        invoke(&self.environment(), &CancellationToken::new(), args).await
    }

    pub(crate) fn environment_with_path(&self, path: &Path) -> Environment {
        Environment::from_vars([
            pair("HOME", self.home.path().as_os_str()),
            pair("PATH", path.as_os_str()),
            pair("RIVETHUB_TEST_ARGV", self.argv.as_os_str()),
        ])
    }

    pub(crate) fn argv(&self) -> Result<Vec<String>> {
        let text = std::fs::read_to_string(&self.argv)?;
        Ok(text.lines().map(str::to_owned).collect())
    }
}

pub(crate) async fn invoke(
    env: &Environment,
    shutdown: &CancellationToken,
    args: impl IntoIterator<Item = impl AsRef<OsStr>>,
) -> Result<Report> {
    let owned = std::iter::once(OsString::from("rh"))
        .chain(args.into_iter().map(|arg| arg.as_ref().to_owned()))
        .collect::<Vec<_>>();
    let command = match Cli::parse_args(owned) {
        Ok(command) => command,
        Err(report) => return Ok(report),
    };
    match cli::dispatch(command, env, shutdown).await {
        Dispatch::Complete(report) => Ok(report),
        Dispatch::Gateway { .. }
        | Dispatch::Worker { .. }
        | Dispatch::Mcp { .. }
        | Dispatch::Local(_) => Err("unexpected service launch".into()),
    }
}

pub(crate) fn assert_argv(actual: &[String], expected: &[&str]) -> Result {
    let mut parts = actual.iter();
    let program = parts.next().ok_or("step program")?;
    assert!(program == "step" || program.ends_with("/step"), "{program}");
    let rest: Vec<&str> = parts.map(String::as_str).collect();
    assert_eq!(rest, expected);
    Ok(())
}

pub(crate) fn utf8(path: &Path) -> Result<String> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| "non-UTF8 test path".into())
}

pub(crate) fn canonical_url(raw: &str) -> Result<String> {
    Ok(url::Url::parse(raw)?.as_str().to_owned())
}

fn pair(name: &str, value: &OsStr) -> (OsString, OsString) {
    (OsString::from(name), value.to_owned())
}

fn roster_fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ca/roster.txt")
}

fn write_root(path: &Path) -> Result<()> {
    let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)?;
    let mut params = rcgen::CertificateParams::default();
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    params.not_before = rcgen::date_time_ymd(2026, 1, 1);
    params.not_after = rcgen::date_time_ymd(2027, 1, 1);
    let mut name = rcgen::DistinguishedName::new();
    name.push(rcgen::DnType::CommonName, "Example Test Root");
    params.distinguished_name = name;
    std::fs::write(path, params.self_signed(&key)?.pem())?;
    Ok(())
}
