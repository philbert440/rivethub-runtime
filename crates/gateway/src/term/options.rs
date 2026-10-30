use std::path::{Component, Path, PathBuf};

use super::TerminalOptions;
use crate::error::ApiError;

pub(super) fn validate_options(mut options: TerminalOptions) -> Result<TerminalOptions, ApiError> {
    options.roots = existing_roots(&options.roots)?;
    if options.max_ptys == 0
        || !options
            .commands
            .iter()
            .any(|command| command.id == options.default)
    {
        return Err(ApiError::BadRequest(
            "invalid terminal configuration".into(),
        ));
    }
    for command in &mut options.commands {
        if command.argv.is_empty() {
            return Err(ApiError::BadRequest(
                "invalid terminal command or directory".into(),
            ));
        }
        if !command.cwd.is_absolute() {
            return Err(ApiError::BadRequest(
                "invalid terminal command or directory".into(),
            ));
        }
    }
    Ok(options)
}

fn existing_roots(roots: &[PathBuf]) -> Result<Vec<PathBuf>, ApiError> {
    let mut existing: Vec<PathBuf> = Vec::new();
    for root in roots {
        match root.canonicalize() {
            Ok(root) if !existing.contains(&root) => existing.push(root),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                tracing::warn!(
                    "terminal root {} does not exist; skipping it",
                    root.display()
                );
            }
            Err(error) => return Err(ApiError::Io(error)),
        }
    }
    if existing.is_empty() {
        return Err(ApiError::BadRequest("no terminal root exists".into()));
    }
    Ok(existing)
}

pub(super) fn contained_directory(
    roots: &[PathBuf],
    directory: &Path,
) -> Result<PathBuf, ApiError> {
    if !directory.is_absolute()
        || directory
            .components()
            .any(|part| part == Component::ParentDir)
    {
        return Err(ApiError::BadRequest(
            "invalid terminal command or directory".into(),
        ));
    }
    let directory = match directory.canonicalize() {
        Ok(directory) => directory,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(ApiError::BadRequest(
                "working directory does not exist".into(),
            ));
        }
        Err(error) => return Err(ApiError::Io(error)),
    };
    if !roots.iter().any(|root| directory.starts_with(root)) {
        return Err(ApiError::Forbidden(
            "working directory is outside the terminal roots".into(),
        ));
    }
    if !directory.is_dir() {
        return Err(ApiError::BadRequest(
            "working directory is not a directory".into(),
        ));
    }
    Ok(directory)
}

#[cfg(test)]
mod tests {
    use super::{ApiError, contained_directory};

    #[test]
    fn a_directory_under_any_root_is_allowed_and_one_outside_every_root_is_forbidden() {
        let base = tempfile::tempdir().expect("base");
        let base = base.path().canonicalize().expect("canonical base");
        let [first, second, outside] = ["first", "second", "outside"].map(|name| base.join(name));
        let project = second.join("project");
        for directory in [&first, &project, &outside] {
            std::fs::create_dir_all(directory).expect("directory");
        }
        let roots = [first, second];
        assert_eq!(
            contained_directory(&roots, &project).expect("inside the second root"),
            project
        );
        assert!(matches!(
            contained_directory(&roots, &outside),
            Err(ApiError::Forbidden(_))
        ));
    }
}
