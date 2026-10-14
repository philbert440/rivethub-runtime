use std::io::ErrorKind;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

#[derive(Debug)]
pub(super) enum FenceError {
    Escape,
    Missing,
    Io(std::io::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Follow {
    Target,
    Entry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Final {
    MustExist,
    MayBeNew,
}

#[derive(Debug)]
pub(super) struct Located {
    pub absolute: PathBuf,
    pub relative: PathBuf,
}

pub(super) fn locate(
    root: &Path,
    raw: &str,
    follow: Follow,
    final_component: Final,
) -> Result<Located, FenceError> {
    let relative = relative_from(raw)?;
    let absolute = walk(root, &relative, follow, final_component)?;
    if !absolute.starts_with(root) {
        return Err(FenceError::Escape);
    }
    Ok(Located { absolute, relative })
}

pub(super) fn relative_text(relative: &Path) -> Result<String, FenceError> {
    let mut parts = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(part) => {
                let Some(text) = part.to_str() else {
                    return Err(FenceError::Escape);
                };
                parts.push(text.to_owned());
            }
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(FenceError::Escape);
            }
        }
    }
    Ok(parts.join("/"))
}

fn relative_from(raw: &str) -> Result<PathBuf, FenceError> {
    if raw.contains('\0') || raw.contains('\\') || raw.starts_with('/') || raw.starts_with('~') {
        return Err(FenceError::Escape);
    }
    let mut relative = PathBuf::new();
    for component in Path::new(raw).components() {
        match component {
            Component::Normal(part) => {
                let Some(text) = part.to_str() else {
                    return Err(FenceError::Escape);
                };
                relative.push(text);
            }
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(FenceError::Escape);
            }
        }
    }
    Ok(relative)
}

fn walk(
    root: &Path,
    relative: &Path,
    follow: Follow,
    final_component: Final,
) -> Result<PathBuf, FenceError> {
    let parts = components_of(relative)?;
    let mut current = root.to_path_buf();
    for (index, part) in parts.iter().enumerate() {
        let last = index + 1 == parts.len();
        let next = current.join(part);
        if !next.starts_with(root) {
            return Err(FenceError::Escape);
        }
        current = step(&next, root, last, follow, final_component)?;
    }
    Ok(current)
}

fn components_of(relative: &Path) -> Result<Vec<String>, FenceError> {
    let mut parts = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(part) => {
                let Some(text) = part.to_str() else {
                    return Err(FenceError::Escape);
                };
                parts.push(text.to_owned());
            }
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(FenceError::Escape);
            }
        }
    }
    Ok(parts)
}

fn step(
    next: &Path,
    root: &Path,
    last: bool,
    follow: Follow,
    final_component: Final,
) -> Result<PathBuf, FenceError> {
    match std::fs::symlink_metadata(next) {
        Ok(meta) if meta.file_type().is_symlink() && !(last && follow == Follow::Entry) => {
            follow_link(next, root)
        }
        Ok(_) => Ok(next.to_path_buf()),
        Err(error)
            if error.kind() == ErrorKind::NotFound
                && last
                && final_component == Final::MayBeNew =>
        {
            Ok(next.to_path_buf())
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Err(FenceError::Missing),
        Err(error) => Err(FenceError::Io(error)),
    }
}

fn follow_link(path: &Path, root: &Path) -> Result<PathBuf, FenceError> {
    match std::fs::canonicalize(path) {
        Ok(target) if target.starts_with(root) => Ok(target),
        Ok(_) => Err(FenceError::Escape),
        Err(error) if error.kind() == ErrorKind::NotFound => Err(FenceError::Missing),
        Err(error) => Err(FenceError::Io(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::FenceError;
    use super::relative_from;

    #[test]
    fn refusal_and_ordinary_names() {
        assert!(matches!(relative_from("../x"), Err(FenceError::Escape)));
        assert!(matches!(
            relative_from("/etc/passwd"),
            Err(FenceError::Escape)
        ));
        assert!(matches!(relative_from("~/secret"), Err(FenceError::Escape)));
        assert!(matches!(relative_from("a\\b"), Err(FenceError::Escape)));
        assert!(matches!(relative_from("a\0b"), Err(FenceError::Escape)));
        let notes = relative_from("notes/file~1.txt").expect("tilde inside a name");
        assert_eq!(notes, std::path::PathBuf::from("notes/file~1.txt"));
        let dotted = relative_from("sub/./file").expect("current dir component");
        assert_eq!(dotted, std::path::PathBuf::from("sub/file"));
    }
}
