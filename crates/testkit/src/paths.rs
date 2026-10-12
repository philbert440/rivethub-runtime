use std::path::Path;
use std::path::PathBuf;

#[must_use]
pub fn golden_path(manifest_dir: &str, crate_relative: &str) -> PathBuf {
    Path::new(manifest_dir).join(crate_relative)
}
