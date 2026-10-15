mod files;
mod holder;
mod token;
mod validate;

use crate::{Error, Report};
use clap::{Args, Subcommand};
use config::Environment;
use kernel::ca::Principal;
use std::path::PathBuf;
use time::OffsetDateTime;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Subcommand)]
pub enum CertCommand {
    Renew(RenewArgs),
}

#[derive(Debug, Args)]
pub struct RenewArgs {
    #[arg(long)]
    pub principal: Principal,
    #[arg(long)]
    pub ca_url: url::Url,
    #[arg(long)]
    pub trust_root: PathBuf,
    #[arg(long)]
    pub token_file: Option<PathBuf>,
    #[arg(long)]
    pub state: Option<PathBuf>,
    #[arg(long)]
    pub once: bool,
}

pub(crate) async fn run(
    command: CertCommand,
    env: &Environment,
    shutdown: &CancellationToken,
) -> Result<Report, Error> {
    let CertCommand::Renew(mut args) = command;
    let directory = state_directory(env, args.state.take()).await?;
    renew(args, directory, shutdown).await?;
    Ok(Report::success(""))
}

async fn state_directory(env: &Environment, explicit: Option<PathBuf>) -> Result<PathBuf, Error> {
    if let Some(path) = explicit {
        return tokio::task::spawn_blocking(move || files::absolute_path(&path)).await?;
    }
    Ok(config::Paths::new(env)?.state_dir())
}

pub async fn renew(
    args: RenewArgs,
    directory: PathBuf,
    shutdown: &CancellationToken,
) -> Result<(), Error> {
    if shutdown.is_cancelled() {
        return Err(Error::Cancelled);
    }
    let RenewArgs {
        principal,
        ca_url,
        trust_root,
        token_file,
        state: _,
        once,
    } = args;
    let authority = connect(ca_url, &trust_root, shutdown).await?;
    let epoch = OffsetDateTime::now_utc();
    let started = tokio::time::Instant::now();
    let principal_uri = principal.spiffe_uri()?;
    let enrollment = match token_file {
        Some(path) => Some(read_enrollment(&path).await?),
        None => None,
    };
    let mut material = tokio::task::spawn_blocking(move || {
        holder::LocalHolder::open(&directory, &trust_root, enrollment, &principal_uri, epoch)
    })
    .await??;
    loop {
        if shutdown.is_cancelled() {
            return Ok(());
        }
        let now = current_time(epoch, started)?;
        if !once
            && let Some(renew_at) = material.renew_at()
            && wait_for_renewal(renew_at, now.unix_timestamp(), shutdown).await
        {
            return Ok(());
        }
        let Some(signed) = request(&authority, &material, shutdown).await? else {
            return Ok(());
        };
        material = tokio::task::spawn_blocking(move || {
            material.install(&signed, now)?;
            Ok::<_, Error>(material)
        })
        .await??;
        if once {
            return Ok(());
        }
    }
}

async fn connect(
    ca_url: url::Url,
    trust_root: &std::path::Path,
    shutdown: &CancellationToken,
) -> Result<stepca::Authority, Error> {
    let path = trust_root.to_path_buf();
    let root = tokio::task::spawn_blocking(move || files::read_external(&path)).await??;
    let authority = stepca::Authority::connect(stepca::Config::new(ca_url, root)?)?;
    Ok(authority.with_cancellation(shutdown.clone()))
}

async fn read_enrollment(path: &std::path::Path) -> Result<token::Enrollment, Error> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || token::read(&path)).await?
}

async fn request(
    authority: &stepca::Authority,
    material: &holder::LocalHolder,
    shutdown: &CancellationToken,
) -> Result<Option<stepca::SignedCertificate>, Error> {
    let pending = exchange(authority, material);
    tokio::pin!(pending);
    let signed = tokio::select! {
        biased;
        () = shutdown.cancelled() => return Ok(None),
        signed = &mut pending => signed,
    };
    match signed {
        Ok(signed) => Ok(Some(signed)),
        Err(Error::StepCa(stepca::Error::Cancelled)) => Ok(None),
        Err(error) => Err(error),
    }
}

async fn exchange(
    authority: &stepca::Authority,
    material: &holder::LocalHolder,
) -> Result<stepca::SignedCertificate, Error> {
    if let Some(token) = material.token().map(str::to_owned) {
        let csr = material.csr()?;
        return Ok(authority.sign(&csr, &token).await?);
    }
    let (certificate, key) = material.identity()?;
    let holder = stepca::Holder::from_pem(certificate, key)?;
    Ok(authority.renew(&holder).await?)
}

fn current_time(
    epoch: OffsetDateTime,
    started: tokio::time::Instant,
) -> Result<OffsetDateTime, Error> {
    let elapsed = time::Duration::try_from(started.elapsed())
        .map_err(|_| Error::PkiInput("renewal clock overflow"))?;
    epoch
        .checked_add(elapsed)
        .ok_or(Error::PkiInput("renewal clock overflow"))
}

async fn wait_for_renewal(deadline: i64, now: i64, shutdown: &CancellationToken) -> bool {
    let delay = deadline.saturating_sub(now).unsigned_abs();
    if deadline <= now {
        return shutdown.is_cancelled();
    }
    tokio::select! {
        biased;
        () = shutdown.cancelled() => true,
        () = tokio::time::sleep(std::time::Duration::from_secs(delay)) => false,
    }
}

#[cfg(test)]
#[path = "../../tests/pki_support/timing.rs"]
mod tests;
