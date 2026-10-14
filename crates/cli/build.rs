use std::io;
use std::path::Path;
use std::process::Command;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=../../.git");
    if Path::new("../../.git").try_exists()? {
        let output = Command::new("git")
            .args(["rev-parse", "--short", "HEAD"])
            .output()?;
        if !output.status.success() {
            return Err(io::Error::other("git could not resolve the build revision").into());
        }
        let revision = String::from_utf8(output.stdout)?;
        println!(
            "cargo:rustc-env=RIVETHUB_BUILD_REVISION={}",
            revision.trim()
        );
    }
    Ok(())
}
