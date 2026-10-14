use std::path::Path;
use std::path::PathBuf;

use kernel::transcript::TranscriptReaderError;

use crate::store_path::Root;
use crate::store_path::is_not_found;
use crate::store_path::session_component;

pub(crate) fn database_file(
    store: &Path,
    file_name: &str,
) -> Result<Option<PathBuf>, TranscriptReaderError> {
    let root = match Root::open(store) {
        Ok(root) => root,
        Err(error) if is_not_found(&error) => return Ok(None),
        Err(error) => return Err(error),
    };
    match root.file(Path::new(file_name)) {
        Ok(path) => Ok(Some(path)),
        Err(error) if is_not_found(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

pub(crate) fn require_component(native_id: &str) -> Result<(), TranscriptReaderError> {
    session_component(native_id)
}
