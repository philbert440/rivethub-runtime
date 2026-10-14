use std::path::Path;

use crate::baseline::Baseline;
use crate::error::Result;

pub fn check(root: &Path, baseline: &Baseline, findings: &mut Vec<String>) -> Result<()> {
    crate::rustsrc::check(root, baseline, "ENV", findings)
}
