use std::path::{Path, PathBuf};

use wire::plugin::manifest::PluginManifest;
use wire::plugin::{PluginId, RouteName};

use super::super::manifest::check_manifest;
use super::super::registry::{MANIFEST_FILE, read_manifest};

pub(super) const GUIDE_FILE: &str = "GUIDE.md";

pub(super) struct Sources<'a> {
    pub(super) templates: &'a Path,
    pub(super) workshop: &'a Path,
}

pub(super) struct Named<'a> {
    pub(super) id: &'a PluginId,
    pub(super) name: &'a str,
    pub(super) description: &'a str,
}

pub(super) struct Template {
    pub(super) name: String,
    pub(super) found: Result<PluginManifest, String>,
    pub(super) files: Vec<String>,
}

pub(super) struct Made {
    pub(super) folder: PathBuf,
    pub(super) template: String,
    pub(super) files: Vec<String>,
}

pub(super) async fn templates(root: &Path) -> Result<Vec<Template>, String> {
    let shown = |error: std::io::Error| format!("{}: {error}", root.display());
    let mut entries = match tokio::fs::read_dir(root).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(shown(error)),
    };
    let mut found = Vec::new();
    while let Some(entry) = entries.next_entry().await.map_err(shown)? {
        if !entry.file_type().await.map_err(shown)?.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        found.push(Template {
            found: read_manifest(&entry.path())
                .await
                .map_err(|error| error.to_string()),
            files: listing(&entry.path()).await?,
            name,
        });
    }
    found.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(found)
}

pub(super) async fn write(
    sources: &Sources<'_>,
    template: &str,
    named: &Named<'_>,
) -> Result<Made, String> {
    if named.name.is_empty() {
        return Err("name: give the plug-in a name".to_owned());
    }
    if named.description.is_empty() {
        return Err("description: say in one sentence what the plug-in does".to_owned());
    }
    let source = template_folder(sources.templates, template).await?;
    let mut manifest = read_manifest(&source)
        .await
        .map_err(|error| format!("template {template}: {error}"))?;
    manifest.id = named.id.clone();
    manifest.name = named.name.to_owned();
    manifest.description = named.description.to_owned();
    check_manifest(&manifest).map_err(|problem| problem.to_string())?;
    let mut text = serde_json::to_vec_pretty(&manifest).map_err(|error| error.to_string())?;
    text.push(b'\n');
    let folder = root(sources.workshop).await?.join(named.id.as_ref());
    match tokio::fs::create_dir(&folder).await {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(format!(
                "{} already exists: edit it and call plugins-install, or choose another id",
                folder.display()
            ));
        }
        Err(error) => return Err(format!("{}: {error}", folder.display())),
    }
    let copied = copy_tree(&source, &folder, template).await;
    let written = match copied {
        Ok(()) => tokio::fs::write(folder.join(MANIFEST_FILE), &text)
            .await
            .map_err(|error| format!("{}: {error}", folder.display())),
        Err(problem) => Err(problem),
    };
    if let Err(problem) = written {
        tokio::fs::remove_dir_all(&folder)
            .await
            .map_err(|error| format!("{problem}; {}: {error}", folder.display()))?;
        return Err(problem);
    }
    Ok(Made {
        files: listing(&folder).await?,
        folder,
        template: template.to_owned(),
    })
}

pub(super) async fn folder(workshop: &Path, id: &PluginId) -> Result<PathBuf, String> {
    let folder = root(workshop).await?.join(id.as_ref());
    real_folder(&folder, "start a plug-in with plugins-scaffold").await?;
    Ok(folder)
}

async fn template_folder(templates: &Path, template: &str) -> Result<PathBuf, String> {
    let name =
        RouteName::try_from(template.to_owned()).map_err(|error| format!("template: {error}"))?;
    let folder = templates.join(name.as_ref());
    real_folder(
        &folder,
        "plugins-templates lists the templates this node holds",
    )
    .await?;
    Ok(folder)
}

async fn real_folder(folder: &Path, next: &str) -> Result<(), String> {
    match tokio::fs::symlink_metadata(folder).await {
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => Err(format!(
            "{} is not a folder; only real folders are used, never links",
            folder.display()
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Err(format!("{} does not exist: {next}", folder.display()))
        }
        Err(error) => Err(format!("{}: {error}", folder.display())),
    }
}

async fn copy_tree(from: &Path, to: &Path, template: &str) -> Result<(), String> {
    let shown = |path: &Path, error: std::io::Error| format!("{}: {error}", path.display());
    let mut pending = vec![(from.to_path_buf(), to.to_path_buf())];
    while let Some((source, target)) = pending.pop() {
        let mut entries = tokio::fs::read_dir(&source)
            .await
            .map_err(|error| shown(&source, error))?;
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|error| shown(&source, error))?
        {
            let kind = entry
                .file_type()
                .await
                .map_err(|error| shown(&entry.path(), error))?;
            let into = target.join(entry.file_name());
            if kind.is_dir() {
                tokio::fs::create_dir(&into)
                    .await
                    .map_err(|error| shown(&into, error))?;
                pending.push((entry.path(), into));
            } else if kind.is_file() {
                tokio::fs::copy(entry.path(), &into)
                    .await
                    .map_err(|error| shown(&into, error))?;
            } else {
                return Err(format!(
                    "template {template} holds {}, which is not a plain file or folder",
                    entry.path().display()
                ));
            }
        }
    }
    Ok(())
}

async fn listing(folder: &Path) -> Result<Vec<String>, String> {
    let shown = |error: std::io::Error| format!("{}: {error}", folder.display());
    let mut entries = tokio::fs::read_dir(folder).await.map_err(shown)?;
    let mut names = Vec::new();
    while let Some(entry) = entries.next_entry().await.map_err(shown)? {
        let mut name = entry.file_name().to_string_lossy().into_owned();
        if entry.file_type().await.map_err(shown)?.is_dir() {
            name.push('/');
        }
        names.push(name);
    }
    names.sort();
    Ok(names)
}

async fn root(workshop: &Path) -> Result<PathBuf, String> {
    tokio::fs::create_dir_all(workshop)
        .await
        .map_err(|error| format!("{}: {error}", workshop.display()))?;
    tokio::fs::canonicalize(workshop)
        .await
        .map_err(|error| format!("{}: {error}", workshop.display()))
}
