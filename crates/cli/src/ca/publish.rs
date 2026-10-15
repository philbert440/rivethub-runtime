use std::path::PathBuf;

use tokio_util::sync::CancellationToken;

use super::paths::Material;
use super::step;
use crate::{Error, Report};

pub(super) async fn crl(
    destination: Option<PathBuf>,
    material: &Material,
    shutdown: &CancellationToken,
) -> Result<Report, Error> {
    if let Some(path) = destination.clone() {
        tokio::task::spawn_blocking(move || super::paths::check_crl_destination(&path)).await??;
    }
    let pem = step::connect(material, shutdown)?.crl().await?;
    if let Some(path) = destination {
        let body = pem.clone();
        tokio::task::spawn_blocking(move || super::paths::publish_crl(&path, body.as_bytes()))
            .await??;
    }
    Ok(Report::success(pem))
}
