use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;
use wire::JsonObject;

use crate::hooks::HookError;

#[derive(Debug, Clone)]
pub struct WorkspaceFence {
    roots: Arc<Vec<PathBuf>>,
    tools: BTreeSet<String>,
    working_dir: Option<PathBuf>,
}

#[derive(Deserialize)]
struct PathArguments {
    path: PathBuf,
}

impl WorkspaceFence {
    pub async fn new(
        roots: Vec<PathBuf>,
        tools: BTreeSet<String>,
        working_dir: Option<PathBuf>,
    ) -> Result<Self, HookError> {
        let (roots, working_dir) = tokio::task::spawn_blocking(move || {
            let roots: Result<Vec<_>, HookError> = roots
                .into_iter()
                .map(|root| {
                    let root = std::fs::canonicalize(normalize(&root)?)?;
                    if !std::fs::metadata(&root)?.is_dir() {
                        return Err(HookError::NotDirectory(root));
                    }
                    Ok(root)
                })
                .collect();
            let roots = roots?;
            let working_dir = working_dir
                .map(|path| resolve(&normalize(&path)?))
                .transpose()?;
            if let Some(path) = &working_dir {
                contained(path, &roots)?;
            }
            Ok::<_, HookError>((roots, working_dir))
        })
        .await??;
        Ok(Self {
            roots: Arc::new(roots),
            tools,
            working_dir,
        })
    }

    pub async fn check(&self, tool: &str, args: &JsonObject) -> Result<(), HookError> {
        if !self.tools.contains(tool) {
            return Ok(());
        }
        let args: PathArguments = serde_json::from_value(serde_json::to_value(args)?)?;
        if args
            .path
            .components()
            .next()
            .is_some_and(|component| component.as_os_str().as_encoded_bytes().starts_with(b"~"))
        {
            return Err(HookError::OutsideWorkspace(args.path));
        }
        let path = if args.path.is_absolute() {
            args.path
        } else {
            self.working_dir
                .as_ref()
                .ok_or_else(|| HookError::OutsideWorkspace(args.path.clone()))?
                .join(args.path)
        };
        let path = normalize(&path)?;
        contained(&path, &self.roots)?;
        let roots = Arc::clone(&self.roots);
        tokio::task::spawn_blocking(move || contained(&resolve(&path)?, &roots)).await?
    }
}

fn normalize(path: &Path) -> Result<PathBuf, HookError> {
    if !path.is_absolute() {
        return Err(HookError::OutsideWorkspace(path.to_path_buf()));
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir | Component::Prefix(_) => {
                return Err(HookError::OutsideWorkspace(path.to_path_buf()));
            }
            Component::CurDir => {}
            Component::RootDir | Component::Normal(_) => normalized.push(component.as_os_str()),
        }
    }
    Ok(normalized)
}

fn contained(path: &Path, roots: &[PathBuf]) -> Result<(), HookError> {
    if roots.iter().any(|root| path.starts_with(root)) {
        Ok(())
    } else {
        Err(HookError::OutsideWorkspace(path.to_path_buf()))
    }
}

fn resolve(path: &Path) -> Result<PathBuf, HookError> {
    let mut ancestor = path;
    let mut missing = Vec::new();
    loop {
        match std::fs::symlink_metadata(ancestor) {
            Ok(_) => {
                let mut resolved = std::fs::canonicalize(ancestor)?;
                for name in missing.into_iter().rev() {
                    resolved.push(name);
                }
                return Ok(resolved);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                missing.push(
                    ancestor
                        .file_name()
                        .ok_or_else(|| HookError::OutsideWorkspace(path.to_path_buf()))?,
                );
                ancestor = ancestor
                    .parent()
                    .ok_or_else(|| HookError::OutsideWorkspace(path.to_path_buf()))?;
            }
            Err(error) => return Err(error.into()),
        }
    }
}
