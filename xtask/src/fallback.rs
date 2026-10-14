use std::collections::BTreeSet;
use std::io::Write as _;
use std::path::Path;
use std::process::Command;

use crate::error::{Error, Result};
use crate::rustsrc::Hit;

pub fn collect(root: &Path) -> Result<Vec<Hit>> {
    let output = Command::new("cargo")
        .current_dir(root)
        .args([
            "dylint",
            "--all",
            "--workspace",
            "--",
            "--all-targets",
            "--all-features",
            "--message-format=json",
        ])
        .env("DYLINT_RUSTFLAGS", "-Wfallback")
        .env("SQLX_OFFLINE", "true")
        .output()?;
    let mut stderr = std::io::stderr().lock();
    stderr.write_all(&output.stderr)?;
    let mut hits = BTreeSet::new();
    let mut rejected_warning = false;
    for message in cargo_metadata::Message::parse_stream(output.stdout.as_slice()) {
        if let cargo_metadata::Message::CompilerMessage(message) = message? {
            let diagnostic = message.message;
            if diagnostic
                .code
                .as_ref()
                .is_some_and(|code| code.code == "fallback")
            {
                let Some(text) = diagnostic
                    .message
                    .strip_prefix("FALLBACK ")
                    .and_then(|text| text.split(" → ").next())
                else {
                    return Err(Error::Finding(
                        "FALLBACK diagnostic has no fingerprint".to_owned(),
                    ));
                };
                for span in diagnostic.spans.iter().filter(|span| span.is_primary) {
                    let absolute = if Path::new(&span.file_name).is_absolute() {
                        std::path::PathBuf::from(&span.file_name)
                    } else {
                        root.join(&span.file_name)
                    };
                    let file = absolute
                        .strip_prefix(root)
                        .map_err(|error| Error::Finding(error.to_string()))?
                        .to_string_lossy()
                        .into_owned();
                    if crate::rustsrc::is_scanned(&file) {
                        hits.insert((file, span.line_start, span.column_start, text.to_owned()));
                    }
                }
            } else {
                rejected_warning |=
                    diagnostic.level == cargo_metadata::diagnostic::DiagnosticLevel::Warning;
                if let Some(rendered) = diagnostic.rendered {
                    stderr.write_all(rendered.as_bytes())?;
                }
            }
        }
    }
    if rejected_warning {
        return Err(Error::Command {
            command: "cargo dylint --all --workspace".to_owned(),
            stderr: "non-baseline compiler warnings are errors".to_owned(),
        });
    }
    if !output.status.success() {
        return Err(Error::Command {
            command: "cargo dylint --all --workspace".to_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    Ok(hits
        .into_iter()
        .map(|(file, line, _, text)| Hit { file, line, text })
        .collect())
}
