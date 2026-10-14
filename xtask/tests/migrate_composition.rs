use std::fs;

use cargo_metadata::MetadataCommand;
use xtask::manifests::migrations_leave_the_runtime;

fn findings(dependencies: &[(&str, &str)]) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::write(
        root.path().join("Cargo.toml"),
        "[workspace]\nmembers = [\"cli\", \"rivethub\", \"gateway\", \"migrate\"]\nresolver = \"3\"\n",
    )?;
    for name in ["cli", "rivethub", "gateway", "migrate"] {
        let package = root.path().join(name);
        fs::create_dir_all(package.join("src"))?;
        fs::write(package.join("src/lib.rs"), "")?;
        let mut manifest =
            format!("[package]\nname = {name:?}\nversion = \"0.1.0\"\nedition = \"2024\"\n");
        for (owner, entries) in dependencies {
            if *owner == name {
                manifest.push_str(entries);
            }
        }
        fs::write(package.join("Cargo.toml"), manifest)?;
    }
    let metadata = MetadataCommand::new()
        .manifest_path(root.path().join("Cargo.toml"))
        .no_deps()
        .exec()?;
    let mut findings = Vec::new();
    migrations_leave_the_runtime(&metadata, &mut findings);
    Ok(findings)
}

#[test]
fn cli_may_compose_schema_verification() -> Result<(), Box<dyn std::error::Error>> {
    let findings = findings(&[
        ("rivethub", "[dependencies]\ncli = { path = \"../cli\" }\n"),
        (
            "cli",
            "[dependencies]\nschema = { package = \"migrate\", path = \"../migrate\" }\n",
        ),
    ])?;
    assert!(findings.is_empty(), "{findings:?}");
    Ok(())
}

#[test]
fn cli_does_not_excuse_a_direct_runtime_dependency() -> Result<(), Box<dyn std::error::Error>> {
    let findings = findings(&[
        (
            "rivethub",
            "[dependencies]\ncli = { path = \"../cli\" }\nmigrate = { path = \"../migrate\" }\n",
        ),
        (
            "cli",
            "[dependencies]\nmigrate = { path = \"../migrate\" }\n",
        ),
    ])?;
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert!(
        findings
            .first()
            .is_some_and(|finding| finding.starts_with("MANIFEST rivethub:"))
    );
    Ok(())
}

#[test]
fn cli_does_not_hide_an_indirect_runtime_dependency() -> Result<(), Box<dyn std::error::Error>> {
    let findings = findings(&[
        ("rivethub", "[dependencies]\ncli = { path = \"../cli\" }\n"),
        (
            "cli",
            "[dependencies]\nmigrate = { path = \"../migrate\" }\ngateway = { path = \"../gateway\" }\n",
        ),
        (
            "gateway",
            "[dependencies]\nschema = { package = \"migrate\", path = \"../migrate\" }\n",
        ),
    ])?;
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert!(
        findings
            .first()
            .is_some_and(|finding| finding.starts_with("MANIFEST rivethub:"))
    );
    Ok(())
}

#[test]
fn runtime_test_dependencies_remain_legal() -> Result<(), Box<dyn std::error::Error>> {
    let findings = findings(&[
        (
            "rivethub",
            "[dependencies]\ncli = { path = \"../cli\" }\n[dev-dependencies]\nmigrate = { path = \"../migrate\" }\n",
        ),
        (
            "cli",
            "[dependencies]\nmigrate = { path = \"../migrate\" }\n",
        ),
    ])?;
    assert!(findings.is_empty(), "{findings:?}");
    Ok(())
}
