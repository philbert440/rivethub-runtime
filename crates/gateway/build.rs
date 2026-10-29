use std::io;
use std::path::Path;
use std::process::Command;

fn git(args: &[&str]) -> Result<String, Box<dyn std::error::Error>> {
    let output = Command::new("git").args(args).output()?;
    if !output.status.success() {
        return Err(io::Error::other("git could not resolve the build revision").into());
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=../../.git");
    if Path::new("../../.git").try_exists()? {
        println!(
            "cargo:rerun-if-changed={}",
            git(&["rev-parse", "--git-path", "logs/HEAD"])?
        );
        println!(
            "cargo:rustc-env=RIVETHUB_BUILD_REVISION={}",
            git(&["rev-parse", "--short", "HEAD"])?
        );
    }
    Ok(())
}
