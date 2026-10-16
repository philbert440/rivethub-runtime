use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use serde::Deserialize;
use sha2::{Digest as _, Sha256};
use tokio_util::sync::CancellationToken;
use url::Url;

use super::{UpdateError, files};

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<Asset>,
}

#[derive(Debug, Deserialize)]
struct Asset {
    name: String,
    browser_download_url: Url,
}

pub(super) fn validate_version(version: &str) -> Result<(), UpdateError> {
    if version.is_empty()
        || !version.starts_with(|c: char| c.is_ascii_alphanumeric())
        || !version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        || version.contains("..")
    {
        return Err(UpdateError::Version(version.to_owned()));
    }
    Ok(())
}

pub(super) async fn stage(
    api: &Url,
    version: Option<&str>,
    directory: &Path,
    shutdown: &CancellationToken,
) -> Result<String, UpdateError> {
    let client = reqwest::Client::builder()
        .user_agent("RivetHub updater")
        .https_only(api.scheme() == "https")
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(300))
        .build()?;
    let mut endpoint = api.clone();
    {
        let mut segments = endpoint
            .path_segments_mut()
            .map_err(|()| UpdateError::Release)?;
        segments.pop_if_empty();
        if let Some(version) = version {
            validate_version(version)?;
            segments.push("tags").push(version);
        } else {
            segments.push("latest");
        }
    }
    let body = download(&client, endpoint, 1024 * 1024, shutdown).await?;
    let release: Release = serde_json::from_slice(&body).map_err(|_| UpdateError::Release)?;
    if release.draft
        || release.prerelease
        || version.is_some_and(|version| release.tag_name != version)
    {
        return Err(UpdateError::Release);
    }
    validate_version(&release.tag_name)?;
    let manifest = asset(&release, "SHA256SUMS")?;
    let manifest = download(&client, manifest, 16 * 1024, shutdown).await?;
    let checksums = checksums(&manifest)?;
    for name in ["rh", "rivethub-migrate"] {
        let asset_name = format!("{name}-linux-x86_64");
        let expected = checksums.get(&asset_name).ok_or(UpdateError::Manifest)?;
        let bytes = download(
            &client,
            asset(&release, &asset_name)?,
            512 * 1024 * 1024,
            shutdown,
        )
        .await?;
        let actual: [u8; 32] = Sha256::digest(&bytes).into();
        if actual != *expected {
            return Err(UpdateError::Checksum(asset_name));
        }
        let path = directory.join(name);
        tokio::task::spawn_blocking(move || files::write_executable(&path, &bytes)).await??;
    }
    Ok(release.tag_name)
}

fn asset(release: &Release, name: &str) -> Result<Url, UpdateError> {
    let mut matching = release.assets.iter().filter(|asset| asset.name == name);
    let asset = matching.next().ok_or(UpdateError::Release)?;
    if matching.next().is_some() {
        return Err(UpdateError::Release);
    }
    Ok(asset.browser_download_url.clone())
}

fn checksums(bytes: &[u8]) -> Result<BTreeMap<String, [u8; 32]>, UpdateError> {
    let text = std::str::from_utf8(bytes).map_err(|_| UpdateError::Manifest)?;
    let mut entries = BTreeMap::new();
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let hash = fields.next().ok_or(UpdateError::Manifest)?;
        let name = fields.next().ok_or(UpdateError::Manifest)?;
        if fields.next().is_some() {
            return Err(UpdateError::Manifest);
        }
        let mut bytes = [0; 32];
        hex::decode_to_slice(hash, &mut bytes).map_err(|_| UpdateError::Manifest)?;
        if entries.insert(name.to_owned(), bytes).is_some() {
            return Err(UpdateError::Manifest);
        }
    }
    Ok(entries)
}

async fn download(
    client: &reqwest::Client,
    url: Url,
    limit: usize,
    shutdown: &CancellationToken,
) -> Result<Vec<u8>, UpdateError> {
    let operation = async {
        let mut response = client.get(url).send().await?.error_for_status()?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if chunk.len() > limit.saturating_sub(bytes.len()) {
                return Err(UpdateError::AssetSize);
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    };
    tokio::select! {
        biased;
        () = shutdown.cancelled() => Err(UpdateError::Cancelled),
        result = operation => result,
    }
}
