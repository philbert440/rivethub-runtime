use crate::common;

use std::os::unix::fs::symlink;
use std::path::Path;
use std::path::PathBuf;

use common::answer;
use tempfile::TempDir;
use testkit::Request;
use testkit::Response;
use wire::gateway::GatewayError;

const APK: &str = "RivetHub-1.2.3.apk";
const APP_IMAGE: &str = "RivetHub-1.2.3.AppImage";

struct Feed {
    keep: TempDir,
    root: PathBuf,
    folder: PathBuf,
    app: axum::Router,
}

fn feed() -> Result<Feed, String> {
    let keep = tempfile::tempdir().map_err(|error| error.to_string())?;
    let root = keep.path().join("files");
    let folder = root.join("builds").join("rivethub");
    std::fs::create_dir_all(&folder).map_err(|error| error.to_string())?;
    let app = common::gateway_with_files(root.clone()).router(false);
    Ok(Feed {
        keep,
        root,
        folder,
        app,
    })
}

fn manifest_naming(files: &[&str]) -> String {
    let releases: Vec<String> = files
        .iter()
        .enumerate()
        .map(|(index, file)| format!(r#""platform{index}":{{"version":"1.2.3","file":"{file}"}}"#))
        .collect();
    format!(r#"{{{},"retired":null}}"#, releases.join(","))
}

fn put(path: &Path, bytes: &[u8]) -> Result<(), String> {
    std::fs::write(path, bytes).map_err(|error| error.to_string())
}

fn published(feed: &Feed, files: &[&str]) -> Result<(), String> {
    put(
        &feed.folder.join("latest.json"),
        manifest_naming(files).as_bytes(),
    )?;
    for file in files {
        put(
            &feed.folder.join(file),
            format!("bytes of {file}").as_bytes(),
        )?;
    }
    Ok(())
}

async fn get(app: &axum::Router, target: &str) -> Result<Response, String> {
    answer(
        app.clone(),
        Request {
            method: "GET".to_owned(),
            target: target.to_owned(),
            headers: vec![("host".to_owned(), "127.0.0.1:5174".to_owned())],
            body: Vec::new(),
            recipe: None,
            via: None,
        },
    )
    .await
}

fn header<'a>(response: &'a Response, name: &str) -> Option<&'a str> {
    response
        .headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

fn refused(response: &Response, status: u16, message: &str) -> Result<(), String> {
    let body = String::from_utf8_lossy(&response.body);
    if response.status != status {
        return Err(format!("status {} body {body}", response.status));
    }
    let parsed: GatewayError =
        serde_json::from_slice(&response.body).map_err(|error| format!("{error}: {body}"))?;
    if parsed.error == message {
        Ok(())
    } else {
        Err(format!("{} != {message}", parsed.error))
    }
}

#[tokio::test]
async fn the_manifest_and_each_build_it_names_are_served() -> Result<(), String> {
    let feed = feed()?;
    published(&feed, &[APK, APP_IMAGE])?;
    let manifest = get(&feed.app, "/api/updates/manifest").await?;
    assert_eq!(manifest.status, 200);
    assert_eq!(manifest.body, manifest_naming(&[APK, APP_IMAGE]).as_bytes());
    assert_eq!(header(&manifest, "content-type"), Some("application/json"));
    assert_eq!(header(&manifest, "cache-control"), Some("no-store"));
    for file in [APK, APP_IMAGE] {
        let build = get(&feed.app, &format!("/api/updates/{file}")).await?;
        let expected = format!("bytes of {file}");
        assert_eq!(build.status, 200, "{file}");
        assert_eq!(build.body, expected.as_bytes());
        assert_eq!(
            header(&build, "content-type"),
            Some("application/octet-stream")
        );
        assert_eq!(
            header(&build, "content-length"),
            Some(expected.len().to_string().as_str())
        );
        assert_eq!(
            header(&build, "content-disposition"),
            Some(format!("attachment; filename=\"{file}\"").as_str())
        );
    }
    Ok(())
}

#[tokio::test]
async fn a_file_the_manifest_does_not_name_is_not_served() -> Result<(), String> {
    let feed = feed()?;
    published(&feed, &[APK])?;
    put(&feed.folder.join("RivetHub-0.9.0.apk"), b"older")?;
    put(&feed.folder.join("latest.json.bak"), b"{}")?;
    for file in ["RivetHub-0.9.0.apk", "latest.json.bak", "latest.json"] {
        refused(
            &get(&feed.app, &format!("/api/updates/{file}")).await?,
            404,
            &format!("{file} is not named in the update manifest"),
        )?;
    }
    Ok(())
}

#[tokio::test]
async fn a_name_that_is_not_a_plain_basename_is_refused_even_when_listed() -> Result<(), String> {
    let feed = feed()?;
    put(&feed.root.join("secret"), b"SECRET")?;
    put(&feed.keep.path().join("outside"), b"OUTSIDE")?;
    published(&feed, &[APK])?;
    put(
        &feed.folder.join("latest.json"),
        manifest_naming(&[
            "../../secret",
            "../../../outside",
            ".hidden",
            "..",
            "%2e%2e",
        ])
        .as_bytes(),
    )?;
    for (target, decoded) in [
        ("/api/updates/..", ".."),
        ("/api/updates/%2e%2e", ".."),
        ("/api/updates/%2E%2E", ".."),
        ("/api/updates/..%2F..%2Fsecret", "../../secret"),
        ("/api/updates/..%2f..%2f..%2foutside", "../../../outside"),
        ("/api/updates/..%5Csecret", "..\\secret"),
        ("/api/updates/%252e%252e", "%2e%2e"),
        ("/api/updates/.hidden", ".hidden"),
        ("/api/updates/RivetHub.apk%00", "RivetHub.apk\0"),
    ] {
        refused(
            &get(&feed.app, target).await?,
            400,
            &format!("{decoded} is not a plain file name"),
        )
        .map_err(|error| format!("{target}: {error}"))?;
    }
    for target in [
        "/api/updates/../secret",
        "/api/updates/../../outside",
        "/api/updates/./latest.json",
    ] {
        let response = get(&feed.app, target).await?;
        assert_ne!(response.status, 200, "{target}");
        assert!(
            !String::from_utf8_lossy(&response.body).contains("SECRET")
                && !String::from_utf8_lossy(&response.body).contains("OUTSIDE"),
            "{target}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn symlinks_never_lead_out_of_the_feed() -> Result<(), String> {
    let feed = feed()?;
    let outside = feed.keep.path().join("outside.apk");
    put(&outside, b"OUTSIDE")?;
    published(&feed, &[APP_IMAGE])?;
    put(
        &feed.folder.join("latest.json"),
        manifest_naming(&[APK, APP_IMAGE]).as_bytes(),
    )?;
    symlink(&outside, feed.folder.join(APK)).map_err(|error| error.to_string())?;
    refused(
        &get(&feed.app, &format!("/api/updates/{APK}")).await?,
        403,
        "the update feed reaches outside its folder",
    )?;
    let manifest = feed.keep.path().join("latest.json");
    put(&manifest, manifest_naming(&[APP_IMAGE]).as_bytes())?;
    std::fs::remove_file(feed.folder.join("latest.json")).map_err(|error| error.to_string())?;
    symlink(&manifest, feed.folder.join("latest.json")).map_err(|error| error.to_string())?;
    refused(
        &get(&feed.app, "/api/updates/manifest").await?,
        403,
        "the update feed reaches outside its folder",
    )?;
    Ok(())
}

#[tokio::test]
async fn a_feed_folder_linked_from_elsewhere_is_refused() -> Result<(), String> {
    let feed = feed()?;
    published(&feed, &[APK])?;
    let elsewhere = feed.keep.path().join("elsewhere");
    std::fs::rename(feed.root.join("builds"), &elsewhere).map_err(|error| error.to_string())?;
    symlink(&elsewhere, feed.root.join("builds")).map_err(|error| error.to_string())?;
    for target in ["/api/updates/manifest", &format!("/api/updates/{APK}")] {
        refused(
            &get(&feed.app, target).await?,
            403,
            "the update feed reaches outside its folder",
        )?;
    }
    Ok(())
}

#[tokio::test]
async fn the_manifest_is_capped_at_one_mebibyte() -> Result<(), String> {
    let feed = feed()?;
    published(&feed, &[APK])?;
    let listed = manifest_naming(&[APK]);
    let mut padded = listed.clone().into_bytes();
    padded.resize(1024 * 1024, b' ');
    put(&feed.folder.join("latest.json"), &padded)?;
    assert_eq!(get(&feed.app, "/api/updates/manifest").await?.body, padded);
    assert_eq!(
        get(&feed.app, &format!("/api/updates/{APK}")).await?.status,
        200
    );
    padded.push(b' ');
    put(&feed.folder.join("latest.json"), &padded)?;
    for target in ["/api/updates/manifest", &format!("/api/updates/{APK}")] {
        refused(
            &get(&feed.app, target).await?,
            500,
            "the update manifest exceeds 1 MiB",
        )?;
    }
    Ok(())
}

#[tokio::test]
async fn a_node_without_a_feed_answers_not_found() -> Result<(), String> {
    let (gateway, _store) = common::gateway();
    let app = gateway.router(false);
    refused(
        &get(&app, "/api/updates/manifest").await?,
        404,
        "this node has no update feed",
    )?;
    let feed = feed()?;
    refused(
        &get(&feed.app, "/api/updates/manifest").await?,
        404,
        "no update manifest on this node",
    )?;
    put(
        &feed.folder.join("latest.json"),
        manifest_naming(&[APK]).as_bytes(),
    )?;
    refused(
        &get(&feed.app, &format!("/api/updates/{APK}")).await?,
        404,
        &format!("{APK} is not a file in the update feed"),
    )?;
    Ok(())
}
