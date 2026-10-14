use std::fs;
use std::io;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

use kernel::transcript::TranscriptReaderError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Child {
    Directory,
    File,
    Absent,
    Ignored,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirState {
    Ready,
    Absent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Root {
    canonical: PathBuf,
}

impl Root {
    pub fn open(path: &Path) -> Result<Self, TranscriptReaderError> {
        let canonical = fs::canonicalize(path).map_err(unavailable)?;
        let metadata = fs::metadata(&canonical).map_err(unavailable)?;
        if !metadata.is_dir() {
            return Err(refused(path, "store root is not a directory"));
        }
        Ok(Self { canonical })
    }

    #[must_use]
    pub fn canonical(&self) -> &Path {
        &self.canonical
    }

    pub fn classify(&self, path: &Path) -> Result<Child, TranscriptReaderError> {
        match fs::symlink_metadata(path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Child::Absent),
            Err(error) => Err(io_read(path, error)),
            Ok(metadata) if metadata.file_type().is_symlink() => self.linked(path),
            Ok(metadata) => self.classify_metadata(path, &metadata),
        }
    }

    pub fn file(&self, relative: &Path) -> Result<PathBuf, TranscriptReaderError> {
        if !relative_normal(relative) {
            return Err(refused(relative, "is not a contained relative path"));
        }
        let candidate = self.canonical.join(relative);
        match self.classify(&candidate)? {
            Child::File => Ok(candidate),
            Child::Absent => Err(missing(&candidate)),
            Child::Directory | Child::Ignored => Err(refused(&candidate, "is not a regular file")),
        }
    }

    pub fn children(&self, dir: &Path) -> Result<Vec<PathBuf>, TranscriptReaderError> {
        let mut paths = Vec::new();
        for entry in fs::read_dir(dir).map_err(unavailable)? {
            paths.push(entry.map_err(unavailable)?.path());
        }
        paths.sort();
        Ok(paths)
    }

    pub fn directories(&self, dir: &Path) -> Result<Vec<PathBuf>, TranscriptReaderError> {
        self.select(dir, Child::Directory)
    }

    pub fn files(&self, dir: &Path) -> Result<Vec<PathBuf>, TranscriptReaderError> {
        self.select(dir, Child::File)
    }

    fn select(&self, dir: &Path, kind: Child) -> Result<Vec<PathBuf>, TranscriptReaderError> {
        let mut matched = Vec::new();
        for path in self.children(dir)? {
            if self.classify(&path)? == kind {
                matched.push(path);
            }
        }
        Ok(matched)
    }

    fn linked(&self, path: &Path) -> Result<Child, TranscriptReaderError> {
        let resolved = fs::canonicalize(path).map_err(|error| io_read(path, error))?;
        if !resolved.starts_with(&self.canonical) {
            return Err(refused(path, "symlinks out of the store"));
        }
        let metadata = fs::metadata(&resolved).map_err(|error| io_read(path, error))?;
        if metadata.is_dir() {
            Ok(Child::Directory)
        } else if metadata.is_file() {
            Ok(Child::File)
        } else {
            Ok(Child::Ignored)
        }
    }

    fn classify_metadata(
        &self,
        path: &Path,
        metadata: &fs::Metadata,
    ) -> Result<Child, TranscriptReaderError> {
        let resolved = fs::canonicalize(path).map_err(|error| io_read(path, error))?;
        if !resolved.starts_with(&self.canonical) {
            return Err(refused(path, "escapes the store"));
        }
        if metadata.is_dir() {
            Ok(Child::Directory)
        } else if metadata.is_file() {
            Ok(Child::File)
        } else {
            Ok(Child::Ignored)
        }
    }
}

#[must_use]
pub fn single_component(native: &str) -> bool {
    let mut components = Path::new(native).components();
    let normal = matches!(components.next(), Some(Component::Normal(_)));
    !native.is_empty() && normal && components.next().is_none()
}

pub fn session_component(native: &str) -> Result<(), TranscriptReaderError> {
    if single_component(native) {
        Ok(())
    } else {
        Err(refused(Path::new(native), "is not a single session id"))
    }
}

pub fn is_not_found(error: &TranscriptReaderError) -> bool {
    let source = match error {
        TranscriptReaderError::Unavailable(source) | TranscriptReaderError::Read { source, .. } => {
            source
        }
    };
    source
        .downcast_ref::<io::Error>()
        .is_some_and(|error| error.kind() == io::ErrorKind::NotFound)
}

pub fn dir_state(root: &Root, path: &Path) -> Result<DirState, TranscriptReaderError> {
    match root.classify(path)? {
        Child::Directory => Ok(DirState::Ready),
        Child::Absent => Ok(DirState::Absent),
        Child::File | Child::Ignored => Err(refused(path, "is not a directory")),
    }
}

pub fn jsonl(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension == "jsonl")
}

pub fn utf8_stem(path: &Path) -> Result<String, TranscriptReaderError> {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .map(str::to_owned)
        .ok_or_else(|| refused(path, "file name is not utf-8"))
}

fn relative_normal(relative: &Path) -> bool {
    !relative.as_os_str().is_empty()
        && !relative.is_absolute()
        && relative
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
}

fn unavailable(error: io::Error) -> TranscriptReaderError {
    TranscriptReaderError::Unavailable(Box::new(error))
}

fn io_read(path: &Path, error: io::Error) -> TranscriptReaderError {
    TranscriptReaderError::Read {
        message: format!("transcript {}: {error}", path.display()),
        source: Box::new(error),
    }
}

pub fn missing(path: &Path) -> TranscriptReaderError {
    io_read(
        path,
        io::Error::new(io::ErrorKind::NotFound, "transcript file is missing"),
    )
}

pub fn refused(path: &Path, detail: &str) -> TranscriptReaderError {
    TranscriptReaderError::Read {
        message: format!("transcript path {} {detail}", path.display()),
        source: Box::new(io::Error::new(io::ErrorKind::PermissionDenied, detail)),
    }
}
