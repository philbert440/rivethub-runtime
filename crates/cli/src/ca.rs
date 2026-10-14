use crate::{Error, Report};
use clap::{Args, Subcommand};
use config::{Environment, Paths};
use kernel::ca::{CertificateAuthority, Principal};
use pki::{ImportLayout, Pki};
use std::path::PathBuf;
use tokio_util::sync::CancellationToken;
use x509_parser::prelude::FromDer;

#[derive(Debug, Args)]
pub struct CaArgs {
    #[arg(long, global = true)]
    pub state: Option<PathBuf>,
    #[arg(long, global = true)]
    pub crl_out: Option<PathBuf>,
    #[command(subcommand)]
    pub command: CaCommand,
}

#[derive(Debug, Subcommand)]
pub enum CaCommand {
    Init {
        #[arg(long)]
        root_key_out: PathBuf,
    },
    Import {
        directory: PathBuf,
    },
    Token {
        principal: Principal,
    },
    Revoke {
        target: String,
    },
    Crl,
    Status,
}

pub(crate) async fn state_path(
    env: &Environment,
    explicit: Option<PathBuf>,
    name: &str,
) -> Result<PathBuf, Error> {
    if let Some(path) = explicit {
        return Ok(tokio::task::spawn_blocking(move || std::path::absolute(path)).await??);
    }
    let paths = Paths::new(env);
    if paths.home().is_none() {
        return Err(Error::PkiInput("HOME is required for PKI state"));
    }
    let base = paths.state_dir();
    let base = tokio::task::spawn_blocking(move || pki::files::directory(&base, true)).await??;
    Ok(base.join(name))
}

pub(crate) async fn run(
    args: CaArgs,
    env: &Environment,
    shutdown: &CancellationToken,
) -> Result<Report, Error> {
    let state = state_path(env, args.state, "ca").await?;
    let authority = open(&args.command, state, shutdown).await?;
    let authority = match args.crl_out {
        Some(path) => {
            let path = tokio::task::spawn_blocking(move || std::path::absolute(path)).await??;
            authority.with_crl_publication(path).await?
        }
        None => authority,
    };
    let port: &dyn CertificateAuthority = &authority;
    match args.command {
        CaCommand::Init { .. } | CaCommand::Import { .. } => {
            Ok(Report::success("Certificate authority ready.\n"))
        }
        CaCommand::Token { principal } => {
            let token = port.issue_token(principal).await?;
            Ok(Report::success(format!("{}\n", token.0)))
        }
        CaCommand::Revoke { target } => {
            if target.starts_with("spiffe:") {
                port.revoke(target.parse()?).await?;
                Ok(Report::success("Principal revoked.\n"))
            } else {
                authority.revoke_serial(target).await?;
                Ok(Report::success("Certificate revoked.\n"))
            }
        }
        CaCommand::Crl => Ok(Report::success(port.crl().await?.0)),
        CaCommand::Status => {
            let chain = port.chain().await?;
            let text = tokio::task::spawn_blocking(move || status(&chain)).await??;
            Ok(Report::success(text))
        }
    }
}

async fn open(
    command: &CaCommand,
    state: PathBuf,
    shutdown: &CancellationToken,
) -> Result<Pki, Error> {
    match command {
        CaCommand::Init { root_key_out } => {
            let path = root_key_out.clone();
            let path = tokio::task::spawn_blocking(move || std::path::absolute(path)).await??;
            Ok(Pki::initialize(state, path, shutdown.clone()).await?)
        }
        CaCommand::Import { directory } => {
            let path = directory.clone();
            let path = tokio::task::spawn_blocking(move || std::path::absolute(path)).await??;
            Ok(Pki::import(path, state, ImportLayout::openssl(), shutdown.clone()).await?)
        }
        CaCommand::Token { .. } | CaCommand::Revoke { .. } | CaCommand::Crl | CaCommand::Status => {
            Ok(Pki::open(state, shutdown.clone()).await?)
        }
    }
}

fn status(chain: &[String]) -> Result<String, Error> {
    use std::fmt::Write;
    let mut output = String::from("Certificate authority ready.\n");
    for pem in chain {
        let der = pki::certificate(pem)?;
        let (_, certificate) = x509_parser::certificate::X509Certificate::from_der(&der)
            .map_err(|_| Error::PkiInput("invalid CA certificate"))?;
        writeln!(
            output,
            "{}: expires {}",
            certificate.subject(),
            certificate.validity().not_after
        )
        .map_err(|_| Error::PkiInput("cannot format CA status"))?;
    }
    Ok(output)
}
