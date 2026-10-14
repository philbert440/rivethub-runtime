mod state;
mod transport;
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
    let path = crate::ca::state_path(env, args.state.take(), "cert").await?;
    renew(args, path, shutdown).await?;
    Ok(Report::success(""))
}

pub async fn renew(
    args: RenewArgs,
    directory: PathBuf,
    shutdown: &CancellationToken,
) -> Result<(), Error> {
    if shutdown.is_cancelled() {
        return Err(Error::Cancelled);
    }
    let endpoint = transport::endpoint(args.ca_url)?;
    let principal = args.principal.spiffe_uri()?;
    let epoch = OffsetDateTime::now_utc();
    let started = tokio::time::Instant::now();
    let mut state = tokio::task::spawn_blocking(move || {
        state::Holder::open(
            &std::path::absolute(directory)?,
            &args.trust_root,
            args.token_file.as_deref(),
            &principal,
            epoch,
        )
    })
    .await??;
    loop {
        if shutdown.is_cancelled() {
            return Ok(());
        }
        let now = current_time(epoch, started)?;
        if !args.once
            && let Some(renew_at) = state.renew_at
            && wait_for_renewal(renew_at, now.unix_timestamp(), shutdown).await
        {
            return Ok(());
        }
        state = tokio::task::spawn_blocking(move || {
            state.prepare()?;
            Ok::<_, Error>(state)
        })
        .await??;
        let response = tokio::select! {
            biased;
            () = shutdown.cancelled() => return Ok(()),
            response = transport::sign(&endpoint, &state) => response?,
        };
        let now = current_time(epoch, started)?;
        state = tokio::task::spawn_blocking(move || {
            state.install(response, now)?;
            Ok::<_, Error>(state)
        })
        .await??;
        if args.once {
            return Ok(());
        }
    }
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
