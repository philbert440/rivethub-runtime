use std::path::Path;

use gateway::Settings;

use super::common;

const INDEX: &str = "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\"><title>golden hub</title></head><body>golden index</body></html>\n";
const SUB_INDEX: &str = "<!doctype html>\n<p>sub index</p>\n";
const HASHED: &str = "console.log(\"hashed asset app-abcdef12\")\n";
const PLAIN_JS: &str = "console.log(\"plain asset\")\n";
const NINE: &str = "console.log(\"nine-char suffix is not a content hash\")\n";
const CSS: &str = "body { color: #222; }\n";
const SVG: &str = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"1\" height=\"1\"/>\n";
const MANIFEST: &str = "{\"name\":\"golden\"}\n";
const NOTES: &str = "unknown extension\n";
const UPPER: &str = "upper-case extension, not a real png\n";
const LICENSE: &str = "golden fixture file without an extension\n";
const PIXEL_PNG: &[u8] = &[
    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0,
    0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84, 120, 218, 99, 100, 248, 207, 80, 15, 0, 3,
    134, 1, 128, 90, 52, 125, 107, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Plain,
    Static,
    Layout,
    Mesh,
    Redirect,
    NoStatic,
}

pub(crate) fn kind(id: &str) -> Kind {
    if is_layout(id) {
        Kind::Layout
    } else if id.starts_with("plain-redirect/") {
        Kind::Redirect
    } else if id.starts_with("plain-nostatic/") {
        Kind::NoStatic
    } else if is_mesh(id) {
        Kind::Mesh
    } else if is_static_surface(id) {
        Kind::Static
    } else {
        Kind::Plain
    }
}

pub(crate) fn is_layout(id: &str) -> bool {
    id.starts_with("plain-main/layout-") || id == "plain-main/body-too-large-layout"
}

fn is_mesh(id: &str) -> bool {
    id.starts_with("plain-main/mesh-")
        || id == "plain-main/capabilities"
        || id == "plain-main/capabilities-post"
}

fn is_static_surface(id: &str) -> bool {
    id.starts_with("plain-main/static-")
        || id.starts_with("plain-main/alias-")
        || id == "plain-main/ws-path-http"
}

pub(crate) fn write_static(root: &Path) -> Result<(), String> {
    let assets = root.join("assets");
    let sub = root.join("sub");
    std::fs::create_dir_all(&assets).map_err(|error| format!("assets dir: {error}"))?;
    std::fs::create_dir_all(&sub).map_err(|error| format!("sub dir: {error}"))?;
    write(&root.join("index.html"), INDEX.as_bytes())?;
    write(&sub.join("index.html"), SUB_INDEX.as_bytes())?;
    write(&assets.join("app-abcdef12.js"), HASHED.as_bytes())?;
    write(&assets.join("plain.js"), PLAIN_JS.as_bytes())?;
    write(&assets.join("app-abcdef123.js"), NINE.as_bytes())?;
    write(&root.join("style.css"), CSS.as_bytes())?;
    write(&root.join("logo.svg"), SVG.as_bytes())?;
    write(&root.join("manifest.json"), MANIFEST.as_bytes())?;
    write(&root.join("pixel.png"), PIXEL_PNG)?;
    write(&root.join("notes.xyz"), NOTES.as_bytes())?;
    write(&root.join("IMAGE.PNG"), UPPER.as_bytes())?;
    write(&root.join("LICENSE"), LICENSE.as_bytes())?;
    Ok(())
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    std::fs::write(path, bytes).map_err(|error| format!("{}: {error}", path.display()))
}

pub(crate) fn static_settings(static_dir: &Path) -> Settings {
    let mut settings = common::settings();
    settings.static_dir = Some(static_dir.to_path_buf());
    settings
}

pub(crate) fn layout_settings(state_dir: &Path) -> Settings {
    let mut settings = common::settings();
    settings.state_dir = state_dir.to_path_buf();
    settings
}

pub(crate) fn redirect_settings(static_dir: &Path, scratch: &Path) -> Result<Settings, String> {
    let shared = scratch.join("shared");
    let legacy = scratch.join("legacy");
    std::fs::create_dir_all(&shared).map_err(|error| format!("shared: {error}"))?;
    std::fs::create_dir_all(&legacy).map_err(|error| format!("legacy: {error}"))?;
    let mut settings = static_settings(static_dir);
    settings.root_redirect = Some("/wiki".to_owned());
    settings.mesh_file = None;
    settings.shared_dir = shared;
    settings.legacy_mesh_dir = legacy;
    settings.state_dir = scratch.join("state");
    Ok(settings)
}

pub(crate) fn nostatic_settings(scratch: &Path) -> Result<Settings, String> {
    let shared = scratch.join("shared");
    let legacy = scratch.join("legacy");
    std::fs::create_dir_all(&shared).map_err(|error| format!("shared: {error}"))?;
    std::fs::create_dir_all(&legacy).map_err(|error| format!("legacy: {error}"))?;
    let mut settings = common::settings();
    settings.static_dir = None;
    settings.mesh_file = None;
    settings.shared_dir = shared;
    settings.legacy_mesh_dir = legacy;
    settings.state_dir = scratch.join("state");
    Ok(settings)
}

pub(crate) fn mesh_settings(
    id: &str,
    scratch: &Path,
    local_port: Option<u16>,
) -> Result<Settings, String> {
    let mut settings = common::settings();
    let path = scratch.join("mesh.json");
    match mesh_body(id, local_port)? {
        MeshBody::Missing => {
            settings.mesh_file = Some(scratch.join("absent.json"));
        }
        MeshBody::Bytes(bytes) => {
            write(&path, &bytes)?;
            settings.mesh_file = Some(path);
        }
    }
    Ok(settings)
}

enum MeshBody {
    Missing,
    Bytes(Vec<u8>),
}

fn mesh_body(id: &str, local_port: Option<u16>) -> Result<MeshBody, String> {
    let body = match id {
        "plain-main/mesh-valid-before-ingest" => {
            let port = local_port.ok_or_else(|| {
                "mesh-valid-before-ingest needs a listener for the local gateway url".to_owned()
            })?;
            MeshBody::Bytes(super::common::roster::recorded(port)?)
        }
        "plain-main/mesh-invalid-json" => MeshBody::Bytes(b"{".to_vec()),
        "plain-main/mesh-root-array" => MeshBody::Bytes(br#"[{"id":"x"}]"#.to_vec()),
        "plain-main/mesh-flat-array" => MeshBody::Bytes(super::common::roster::flat()?),
        "plain-main/mesh-nodes-number" => MeshBody::Bytes(br#"{"nodes":1}"#.to_vec()),
        "plain-main/mesh-empty-object"
        | "plain-main/capabilities"
        | "plain-main/capabilities-post" => MeshBody::Bytes(b"{}".to_vec()),
        _ => MeshBody::Missing,
    };
    Ok(body)
}

pub(crate) fn expand(mut request: testkit::Request) -> Result<testkit::Request, String> {
    let Some(recipe) = request.recipe.clone() else {
        return Ok(request);
    };
    request.body = repeated_byte(&recipe)?;
    Ok(request)
}

fn repeated_byte(recipe: &str) -> Result<Vec<u8>, String> {
    let (count, unit) = recipe
        .split_once(" x ")
        .ok_or_else(|| format!("recipe {recipe}"))?;
    let count: usize = count
        .parse()
        .map_err(|error| format!("recipe count: {error}"))?;
    if unit.trim() != "\"a\"" {
        return Err(format!("recipe unit {unit}"));
    }
    Ok(vec![b'a'; count])
}
