use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use tempfile::TempDir;
use xtask::check;
use xtask::code;
use xtask::dupfixture;
use xtask::labpath;
use xtask::lang;
use xtask::manifests;
use xtask::patterns;
use xtask::privacy;
use xtask::toolconfig;

struct Repo {
    root: PathBuf,
    _guard: TempDir,
}

impl Repo {
    fn new() -> std::io::Result<Self> {
        let guard = tempfile::tempdir()?;
        let root = guard.path().to_path_buf();
        let _ = git(&root, &["init", "-q"])?;
        Ok(Self {
            root,
            _guard: guard,
        })
    }

    fn root(&self) -> &Path {
        &self.root
    }

    fn write(&self, rel: &str, content: &str) -> std::io::Result<()> {
        let path = self.root.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, content)
    }

    fn track(&self) -> std::io::Result<()> {
        git(&self.root, &["add", "-A"]).map(|_| ())
    }
}

fn git(root: &Path, args: &[&str]) -> std::io::Result<String> {
    let output = std::process::Command::new("git")
        .current_dir(root)
        .args(args)
        .output()?;
    if !output.status.success() {
        return Err(std::io::Error::other(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

const ALLOW: &str = r#"
extensions = ["md", "rs", "toml"]
paths = ["LICENSE", "coverage-floor.txt"]
globs = ["crates/*/tests/golden/*.json", "xtask/data/*.json"]
"#;

fn seed_languages(repo: &Repo) -> Result<(), Box<dyn std::error::Error>> {
    repo.write("xtask/languages.toml", ALLOW)?;
    repo.write("docs/PATTERNS.md", "")?;
    repo.write("README.md", "runtime\n")?;
    Ok(())
}

#[test]
fn allow_list_passes() -> Result<(), Box<dyn std::error::Error>> {
    let repo = Repo::new()?;
    seed_languages(&repo)?;
    repo.write("crates/wire/src/lib.rs", "pub fn start() {}\n")?;
    repo.write("crates/wire/Cargo.toml", "[package]\nname = \"wire\"\n")?;
    repo.write("crates/wire/tests/golden/echo.json", "{}\n")?;
    repo.write("LICENSE", "Apache-2.0\n")?;
    repo.write("coverage-floor.txt", "0\n")?;
    repo.track()?;
    let mut findings = Vec::new();
    lang::check(repo.root(), &mut findings)?;
    assert!(
        findings.is_empty(),
        "the allow-list must pass, got: {findings:?}"
    );
    Ok(())
}

#[test]
fn second_language_fails() -> Result<(), Box<dyn std::error::Error>> {
    for (file, body) in [
        ("tools/hook.ts", "export const a = 1\n"),
        ("scripts/plugin.py", "print('x')\n"),
        ("scripts/run.sh", "echo x\n"),
        ("package.json", "{}\n"),
        ("crates/wire/src/extra.js", "var a = 1\n"),
    ] {
        let repo = Repo::new()?;
        seed_languages(&repo)?;
        repo.write(file, body)?;
        repo.track()?;
        let mut findings = Vec::new();
        lang::check(repo.root(), &mut findings)?;
        assert!(
            findings.iter().any(|finding| finding.contains(file)),
            "{file} must fail the allow-list, got: {findings:?}"
        );
    }
    Ok(())
}

#[test]
fn installed_hooks_reject_and_accept_commits() -> Result<(), Box<dyn std::error::Error>> {
    let repo = Repo::new()?;
    seed_languages(&repo)?;
    repo.write(
        "Cargo.toml",
        "[workspace]\nresolver = \"3\"\nmembers = [\"crates/*\"]\n",
    )?;
    repo.write(
        "crates/wire/Cargo.toml",
        "[package]\nname = \"wire\"\nedition = \"2021\"\nversion = \"0.1.0\"\n",
    )?;
    repo.write("crates/wire/src/lib.rs", "pub fn start() {}\n")?;
    repo.write("xtask/data/privacy-denylist.json", "{\"sha256\": []}")?;
    repo.write("xtask/data/secret-denylist.json", "{\"sha256\": []}")?;
    let digest = privacy::sha256("test user <test@example.com>");
    repo.write(
        "xtask/data/house-identities.json",
        &format!("{{\"sha256\": [\"{digest}\"]}}"),
    )?;
    repo.track()?;
    git(repo.root(), &["config", "user.name", "Test User"])?;
    git(repo.root(), &["config", "user.email", "test@example.com"])?;
    let hook_binary = std::env::vars_os()
        .find(|(name, _)| name == std::ffi::OsStr::new("CARGO_BIN_EXE_xtask"))
        .map(|(_, value)| value)
        .ok_or("the xtask binary is missing (CARGO_BIN_EXE_xtask)")?;
    check::install_hooks_from(repo.root(), Path::new(&hook_binary))?;
    assert!(repo.root().join("target/git-hooks/pre-commit").exists());
    assert!(repo.root().join("target/git-hooks/commit-msg").exists());

    let bad = git(
        repo.root(),
        &[
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "build: x\n\nCo-authored-by: Claude <noreply@example.com>\n",
        ],
    );
    assert!(
        bad.is_err(),
        "an attribution trailer must be rejected by the installed commit-msg hook"
    );

    let good = git(
        repo.root(),
        &[
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "build: install repository hooks",
        ],
    );
    assert!(
        good.is_ok(),
        "a house commit must pass the installed hooks: {good:?}"
    );
    Ok(())
}

#[test]
fn comment_fires_and_passes() {
    let bad = "// note\npub fn a() {}\n";
    let mut findings = Vec::new();
    code::check_source(&mut findings, "crates/mcp/src/lib.rs", bad);
    assert!(
        findings
            .iter()
            .any(|finding| finding.starts_with("COMMENT ")),
        "a comment must be reported, got: {findings:?}"
    );

    let good = "pub fn a() {}\n";
    let mut findings = Vec::new();
    code::check_source(&mut findings, "crates/mcp/src/lib.rs", good);
    assert!(findings.is_empty(), "clean code must pass: {findings:?}");

    let strings =
        "pub const A: &str = \"// not a comment\";\npub fn b() -> bool { a().contains(\"/*\") }\n";
    let mut findings = Vec::new();
    code::check_source(&mut findings, "crates/mcp/src/lib.rs", strings);
    assert!(
        findings.is_empty(),
        "comment marks inside string literals must pass: {findings:?}"
    );
}

#[test]
fn size_ceiling_fires() -> Result<(), Box<dyn std::error::Error>> {
    let repo = Repo::new()?;
    repo.write("docs/PATTERNS.md", "")?;
    let mut big = String::new();
    for index in 0..610 {
        let _ = write!(big, "pub fn item_{index}() {{}}\n\n");
    }
    repo.write("crates/mcp/src/big.rs", &big)?;
    repo.write("crates/mcp/src/small.rs", "pub fn a() {}\n")?;
    repo.track()?;
    let mut findings = Vec::new();
    code::check_code(repo.root(), &mut findings)?;
    assert!(
        findings
            .iter()
            .any(|finding| finding.starts_with("SIZE crates/mcp/src/big.rs")),
        "a 610-line file must exceed the ceiling, got: {findings:?}"
    );
    assert!(
        !findings.iter().any(|finding| finding.contains("small.rs")),
        "small files pass: {findings:?}"
    );
    Ok(())
}

#[test]
fn naming_file_names_fires() -> Result<(), Box<dyn std::error::Error>> {
    let repo = Repo::new()?;
    repo.write("docs/PATTERNS.md", "")?;
    repo.write("crates/mcp/src/den.rs", "pub fn a() {}\n")?;
    repo.write("crates/mcp/src/gateway.rs", "pub fn a() {}\n")?;
    repo.track()?;
    let mut findings = Vec::new();
    code::check_code(repo.root(), &mut findings)?;
    assert!(
        findings
            .iter()
            .any(|finding| finding.starts_with("NAMING crates/mcp/src/den.rs")),
        "a den.rs file name must be reported, got: {findings:?}"
    );
    assert!(
        !findings
            .iter()
            .any(|finding| finding.contains("gateway.rs")),
        "gateway.rs passes: {findings:?}"
    );
    Ok(())
}

fn config_repo(repo: &Repo) -> Result<(), Box<dyn std::error::Error>> {
    repo.write(
        "Cargo.toml",
        "[workspace]\nresolver = \"3\"\nmembers = [\"crates/*\"]\nexclude = [\"lints\"]\n",
    )?;
    repo.write("clippy.toml", "allow-unwrap-in-tests = true\n")?;
    repo.write("rustfmt.toml", "edition = \"2024\"\n")?;
    repo.write("deny.toml", "[graph]\nall-features = true\n")?;
    repo.write(
        ".cargo/config.toml",
        "[alias]\nxtask = \"run -q -p xtask --\"\n",
    )?;
    repo.write("rust-toolchain.toml", "[toolchain]\nchannel = \"1.99.0\"\n")?;
    Ok(())
}

#[test]
fn toolconfig_root_only_files_pass() -> Result<(), Box<dyn std::error::Error>> {
    let repo = Repo::new()?;
    config_repo(&repo)?;
    repo.write("crates/wire/Cargo.toml", &manifest("wire", ""))?;
    repo.write("crates/wire/src/lib.rs", "")?;
    repo.track()?;
    let mut findings = Vec::new();
    toolconfig::check(repo.root(), &mut findings)?;
    assert!(
        findings.is_empty(),
        "tool files at the workspace root pass: {findings:?}"
    );
    Ok(())
}

#[test]
fn toolconfig_nested_clippy_toml_fires() -> Result<(), Box<dyn std::error::Error>> {
    let repo = Repo::new()?;
    config_repo(&repo)?;
    repo.write("crates/wire/Cargo.toml", &manifest("wire", ""))?;
    repo.write("crates/wire/clippy.toml", "disallowed-types = []\n")?;
    repo.track()?;
    let mut findings = Vec::new();
    toolconfig::check(repo.root(), &mut findings)?;
    assert_eq!(findings.len(), 1, "exactly one finding: {findings:?}");
    assert!(
        findings
            .first()
            .is_some_and(|finding| finding.contains("crates/wire/clippy.toml")),
        "the finding names the file: {findings:?}"
    );
    Ok(())
}

#[test]
fn toolconfig_member_without_lints_fires() -> Result<(), Box<dyn std::error::Error>> {
    let repo = Repo::new()?;
    config_repo(&repo)?;
    repo.write("crates/wire/Cargo.toml", "[package]\nname = \"wire\"\n")?;
    repo.track()?;
    let mut findings = Vec::new();
    toolconfig::check(repo.root(), &mut findings)?;
    assert_eq!(findings.len(), 1, "exactly one finding: {findings:?}");
    assert!(
        findings.first().is_some_and(|finding| {
            finding.contains("crates/wire/Cargo.toml")
                && finding.contains("missing [lints] workspace = true")
        }),
        "the finding names the manifest: {findings:?}"
    );
    Ok(())
}

#[test]
fn toolconfig_member_own_lints_table_fires() -> Result<(), Box<dyn std::error::Error>> {
    let repo = Repo::new()?;
    config_repo(&repo)?;
    repo.write(
        "crates/wire/Cargo.toml",
        "[package]\nname = \"wire\"\n\n[lints]\nworkspace = true\n\n[lints.clippy]\nunwrap_used = \"deny\"\n",
    )?;
    repo.track()?;
    let mut findings = Vec::new();
    toolconfig::check(repo.root(), &mut findings)?;
    assert_eq!(findings.len(), 1, "exactly one finding: {findings:?}");
    assert!(
        findings
            .first()
            .is_some_and(|finding| finding.contains("own [lints.clippy] table")),
        "the finding names the table: {findings:?}"
    );
    Ok(())
}

#[test]
fn toolconfig_excluded_directory_is_its_own_root() -> Result<(), Box<dyn std::error::Error>> {
    let repo = Repo::new()?;
    config_repo(&repo)?;
    repo.write("lints/Cargo.toml", "[package]\nname = \"rr_lints\"\n")?;
    repo.write(
        "lints/rust-toolchain.toml",
        "[toolchain]\nchannel = \"nightly\"\n",
    )?;
    repo.track()?;
    let mut findings = Vec::new();
    toolconfig::check(repo.root(), &mut findings)?;
    assert!(
        findings.is_empty(),
        "an excluded workspace root owns its tool files: {findings:?}"
    );
    Ok(())
}

#[test]
fn toolconfig_nested_below_excluded_root_fires() -> Result<(), Box<dyn std::error::Error>> {
    let repo = Repo::new()?;
    config_repo(&repo)?;
    repo.write("lints/inner/clippy.toml", "disallowed-types = []\n")?;
    repo.track()?;
    let mut findings = Vec::new();
    toolconfig::check(repo.root(), &mut findings)?;
    assert_eq!(findings.len(), 1, "exactly one finding: {findings:?}");
    assert!(
        findings
            .first()
            .is_some_and(|finding| finding.contains("lints/inner/clippy.toml")),
        "the finding names the file: {findings:?}"
    );
    Ok(())
}

#[test]
fn toolconfig_current_repository_passes() -> Result<(), Box<dyn std::error::Error>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("xtask must live inside the repository")?;
    let mut findings = Vec::new();
    toolconfig::check(root, &mut findings)?;
    assert!(
        findings.is_empty(),
        "the current repository passes: {findings:?}"
    );
    Ok(())
}

fn manifest(crate_name: &str, extra: &str) -> String {
    format!(
        "[package]\nname = \"{crate_name}\"\nedition.workspace = true\nlicense.workspace = true\n\
         version.workspace = true\npublish.workspace = true\nrust-version.workspace = true\n\
         repository.workspace = true\n{extra}\n[lints]\nworkspace = true\n"
    )
}

fn workspace_toml() -> String {
    "[workspace]\nresolver = \"3\"\nmembers = [\"crates/*\"]\n\n\
     [workspace.package]\nedition = \"2024\"\nlicense = \"Apache-2.0\"\npublish = false\n\
     version = \"0.1.0\"\nrust-version = \"1.99\"\n\
     repository = \"https://example.com/rivethub-runtime\"\n\n\
     [workspace.lints.rust]\nunsafe_code = \"forbid\"\n\n\
     [workspace.dependencies]\nthiserror = \"2\"\n"
        .to_owned()
}

#[test]
fn manifest_inheritance_is_enforced() -> Result<(), Box<dyn std::error::Error>> {
    let good = Repo::new()?;
    good.write("Cargo.toml", &workspace_toml())?;
    good.write(
        "crates/wire/Cargo.toml",
        &manifest("wire", "[dependencies]\nthiserror.workspace = true\n"),
    )?;
    good.write("crates/wire/src/lib.rs", "")?;
    good.track()?;
    let mut findings = Vec::new();
    manifests::manifests(good.root(), &mut findings)?;
    assert!(
        findings.is_empty(),
        "inherited manifests pass: {findings:?}"
    );

    let bad = Repo::new()?;
    bad.write("Cargo.toml", &workspace_toml())?;
    bad.write(
        "crates/wire/Cargo.toml",
        &manifest("misnamed", "[dependencies]\nthiserror = \"2\"\n")
            .replace("[lints]\nworkspace = true\n", ""),
    )?;
    bad.write("crates/wire/src/lib.rs", "")?;
    bad.track()?;
    let mut findings = Vec::new();
    manifests::manifests(bad.root(), &mut findings)?;
    assert!(
        findings
            .iter()
            .any(|finding| finding.contains("package.name must equal the directory name")),
        "a misnamed package is reported: {findings:?}"
    );
    assert!(
        findings
            .iter()
            .any(|finding| finding.contains("missing [lints] workspace = true")),
        "a crate without [lints] workspace is reported: {findings:?}"
    );
    assert!(
        findings.iter().any(|finding| finding
            .contains("dependencies.thiserror must come from [workspace.dependencies]")),
        "an out-of-workspace dependency is reported: {findings:?}"
    );
    Ok(())
}

#[test]
fn manifest_member_depending_on_the_binary_fires() -> Result<(), Box<dyn std::error::Error>> {
    let bad = Repo::new()?;
    bad.write("Cargo.toml", &workspace_toml())?;
    bad.write("crates/rivethub/Cargo.toml", &manifest("rivethub", ""))?;
    bad.write("crates/rivethub/src/main.rs", "fn main() {}\n")?;
    bad.write(
        "crates/cli/Cargo.toml",
        &manifest(
            "cli",
            "[dependencies]\nrivethub = { path = \"../rivethub\" }\n",
        ),
    )?;
    bad.write("crates/cli/src/lib.rs", "")?;
    bad.track()?;
    let mut findings = Vec::new();
    manifests::manifests(bad.root(), &mut findings)?;
    assert!(
        findings
            .iter()
            .any(|finding| finding.starts_with("MANIFEST cli")
                && finding.contains("must not depend on the rivethub binary")),
        "the member linking the binary is reported: {findings:?}"
    );

    let good = Repo::new()?;
    good.write("Cargo.toml", &workspace_toml())?;
    good.write("crates/rivethub/Cargo.toml", &manifest("rivethub", ""))?;
    good.write("crates/rivethub/src/main.rs", "fn main() {}\n")?;
    good.write("crates/gateway/Cargo.toml", &manifest("gateway", ""))?;
    good.write("crates/gateway/src/lib.rs", "")?;
    good.track()?;
    let mut findings = Vec::new();
    manifests::manifests(good.root(), &mut findings)?;
    assert!(
        findings.is_empty(),
        "a member free of the binary passes: {findings:?}"
    );
    Ok(())
}

#[test]
fn lint_policy_fires_and_passes() -> Result<(), Box<dyn std::error::Error>> {
    let policy =
        "[lints.rust]\nunsafe_code = \"forbid\"\n\n[lints.clippy]\nunwrap_used = \"deny\"\n";
    let repo = Repo::new()?;
    repo.write("xtask/lints-policy.toml", policy)?;
    repo.write(
        "Cargo.toml",
        "[workspace]\nresolver = \"3\"\nmembers = []\n\n[workspace.lints.rust]\nunsafe_code = \"forbid\"\n\n[workspace.lints.clippy]\nunwrap_used = \"deny\"\n",
    )?;
    let mut findings = Vec::new();
    manifests::lint_policy(repo.root(), &mut findings)?;
    assert!(findings.is_empty(), "matching policy passes: {findings:?}");

    repo.write(
        "Cargo.toml",
        "[workspace]\nresolver = \"3\"\nmembers = []\n\n[workspace.lints.rust]\nunsafe_code = \"forbid\"\n\n[workspace.lints.clippy]\nunwrap_used = \"allow\"\n",
    )?;
    let mut findings = Vec::new();
    manifests::lint_policy(repo.root(), &mut findings)?;
    assert!(
        findings
            .iter()
            .any(|finding| finding.starts_with("LINTPOLICY ")),
        "a lint change outside the policy file fails: {findings:?}"
    );
    Ok(())
}

#[test]
fn identity_foreign_fails_house_passes() -> Result<(), Box<dyn std::error::Error>> {
    let repo = Repo::new()?;
    let digest = privacy::sha256("test user <test@example.com>");
    repo.write(
        "xtask/data/house-identities.json",
        &format!("{{\"sha256\": [\"{digest}\"]}}"),
    )?;
    repo.track()?;
    let mut findings = Vec::new();
    manifests::commit_message(
        repo.root(),
        &mut findings,
        "build: a house commit\n",
        Some(("Test User", "test@example.com")),
    )?;
    assert!(
        findings.is_empty(),
        "the house identity passes: {findings:?}"
    );

    let mut findings = Vec::new();
    manifests::commit_message(
        repo.root(),
        &mut findings,
        "build: a foreign commit\n",
        Some(("Foreign Bot", "bot@foreign.dev")),
    )?;
    assert!(
        findings
            .iter()
            .any(|finding| finding.contains("is not a house identity")),
        "a foreign identity fails: {findings:?}"
    );

    let mut findings = Vec::new();
    manifests::commit_message(
        repo.root(),
        &mut findings,
        "build: x\n\nCo-authored-by: Claude <noreply@anthropic.com>\n",
        None,
    )?;
    assert!(
        findings
            .iter()
            .any(|finding| finding.contains("Co-authored-by")),
        "an attribution trailer fails: {findings:?}"
    );
    Ok(())
}

#[test]
fn commits_mode_reports_range() -> Result<(), Box<dyn std::error::Error>> {
    let repo = Repo::new()?;
    let digest = privacy::sha256("test user <test@example.com>");
    repo.write(
        "xtask/data/house-identities.json",
        &format!("{{\"sha256\": [\"{digest}\"]}}"),
    )?;
    repo.track()?;
    let output = std::process::Command::new("git")
        .current_dir(repo.root())
        .args([
            "-c",
            "user.name=Test User",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "feat: seeded",
        ])
        .env_remove("GIT_AUTHOR_NAME")
        .env_remove("GIT_AUTHOR_EMAIL")
        .env_remove("GIT_COMMITTER_NAME")
        .env_remove("GIT_COMMITTER_EMAIL")
        .output()?;
    assert!(
        output.status.success(),
        "git commit failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut findings = Vec::new();
    let count = manifests::commits(repo.root(), "HEAD", &mut findings)?;
    assert_eq!(count, 1);
    assert!(findings.is_empty(), "house commits pass: {findings:?}");
    Ok(())
}

fn patterns_registry() -> String {
    let mut text = String::from(
        "# Patterns\n\n## COMMENT\nAnti-pattern: comments. Pattern: none. Check: xtask `COMMENT`.\n\n\
         ## UNTYPED\nAnti-pattern: raw serde values. Pattern: typed structs. Check: dylint `untyped`, clippy `disallowed-types`.\n\n",
    );
    let _ = write!(text, "## COUNTTYPE\n... Check: dylint `counttype`.\n");
    for lint in patterns::dylint_lints() {
        if *lint != "untyped" && *lint != "counttype" {
            let _ = write!(
                text,
                "## {name}\n... Check: dylint `{lint}`.\n",
                name = lint.to_ascii_uppercase()
            );
        }
    }
    for check in patterns::checks() {
        if *check != "COMMENT" {
            let _ = write!(
                text,
                "## {name}\n... Check: xtask `{check}`.\n",
                name = check.to_ascii_uppercase()
            );
        }
    }
    text
}

#[test]
fn complete_registry_passes() -> Result<(), Box<dyn std::error::Error>> {
    let repo = Repo::new()?;
    repo.write("docs/PATTERNS.md", &patterns_registry())?;
    repo.track()?;
    let mut findings = Vec::new();
    patterns::check(repo.root(), &mut findings)?;
    assert!(
        findings.is_empty(),
        "complete registry passes: {findings:?}"
    );
    Ok(())
}

#[test]
fn missing_entry_fires() -> Result<(), Box<dyn std::error::Error>> {
    let repo = Repo::new()?;
    repo.write(
        "docs/PATTERNS.md",
        "## COMMENT\n... Check: xtask `COMMENT`.\n\n## UNTYPED\n... Check: dylint `untyped`.\n",
    )?;
    repo.track()?;
    let mut findings = Vec::new();
    patterns::check(repo.root(), &mut findings)?;
    assert!(
        findings
            .iter()
            .any(|finding| finding.contains("dylint lint `counttype`")),
        "a lint without an entry is reported: {findings:?}"
    );
    assert!(
        findings
            .iter()
            .any(|finding| finding.contains("check `LANGUAGES`")),
        "a check without an entry is reported: {findings:?}"
    );
    Ok(())
}

#[test]
fn entry_without_check_line_fires() -> Result<(), Box<dyn std::error::Error>> {
    let repo = Repo::new()?;
    repo.write(
        "docs/PATTERNS.md",
        "## COMMENT\n... Check: xtask `COMMENT`.\n\n## STRSET\nAnti-pattern: string tables. Pattern: enums.\n",
    )?;
    repo.track()?;
    let mut findings = Vec::new();
    patterns::check(repo.root(), &mut findings)?;
    assert!(
        findings
            .iter()
            .any(|finding| finding.contains("STRSET: entry has no Check: line")),
        "an entry without a Check line is reported: {findings:?}"
    );
    Ok(())
}

#[test]
fn dupfixture_cross_crate_duplicate_fires() -> Result<(), Box<dyn std::error::Error>> {
    let repo = Repo::new()?;
    repo.write(
        "crates/wire/tests/golden/capture/writer/results.json",
        "{\"single\":{\"ok\":true}}\n",
    )?;
    repo.write(
        "crates/testkit/tests/fixtures/capture/writer/results.json",
        "{\"single\":{\"ok\":true}}\n",
    )?;
    repo.write(
        "crates/wire/tests/golden/capture/claude-hook/hook-ts.json",
        "{\"entries\":[]}\n",
    )?;
    repo.write(
        "crates/capture/tests/golden/capture/claude-hook/hook-ts.json",
        "{\"entries\":[]}\n",
    )?;
    repo.track()?;
    let mut findings = Vec::new();
    dupfixture::check(repo.root(), &mut findings)?;
    assert_eq!(findings.len(), 2, "both pairs fire: {findings:?}");
    assert!(
        findings
            .iter()
            .all(|finding| finding.starts_with("DUPFIXTURE ")),
        "findings carry the DUPFIXTURE label: {findings:?}"
    );
    assert!(
        findings.iter().any(|finding| {
            finding.contains("crates/testkit/tests/fixtures/capture/writer/results.json")
                && finding.contains("crates/wire/tests/golden/capture/writer/results.json")
        }),
        "the writer pair is named: {findings:?}"
    );
    assert!(
        findings.iter().any(|finding| {
            finding.contains("crates/capture/tests/golden/capture/claude-hook/hook-ts.json")
                && finding.contains("crates/wire/tests/golden/capture/claude-hook/hook-ts.json")
        }),
        "the claude-hook pair is named: {findings:?}"
    );
    Ok(())
}

#[test]
fn dupfixture_one_crate_copies_pass() -> Result<(), Box<dyn std::error::Error>> {
    let repo = Repo::new()?;
    let chunks = "{\"chunks\":[]}\n";
    repo.write(
        "crates/wire/tests/golden/providers/vllm/A-text/chunks.json",
        chunks,
    )?;
    repo.write(
        "crates/wire/tests/golden/providers/ollama/A-text/chunks.json",
        chunks,
    )?;
    repo.write(
        "crates/wire/tests/fixtures/provider/A-text/chunks.json",
        chunks,
    )?;
    repo.write("crates/wire/tests/golden/DEVIATIONS.md", "# deviations\n")?;
    repo.write(
        "crates/config/tests/golden/DEVIATIONS.md",
        "# other deviations\n",
    )?;
    repo.track()?;
    let mut findings = Vec::new();
    dupfixture::check(repo.root(), &mut findings)?;
    assert!(
        findings.is_empty(),
        "identical files inside one crate and differing files across crates pass: {findings:?}"
    );
    Ok(())
}

#[test]
fn labpath_fires_and_passes() -> Result<(), Box<dyn std::error::Error>> {
    let needle = concat!("/rivet-shared", "/plans/rust-rewrite/golden");
    let repo = Repo::new()?;
    repo.write("docs/PATTERNS.md", "")?;
    repo.write(
        "crates/memory-postgres/tests/capture_pg.rs",
        &format!("const GOLDEN_PG: &str = \"{needle}\";\n"),
    )?;
    repo.write(
        "xtask/src/notes.md",
        &format!("re-record from {needle}/capture/pg\n"),
    )?;
    repo.write(
        "crates/config/src/paths.rs",
        "const DEFAULT_SHARED_DIR: &str = \"/rivet-shared\";\n",
    )?;
    repo.write(
        "crates/wire/tests/golden/gateway/http-tls.json",
        &format!("{{\"note\": \"the inventory ({needle}/inventory/gateway-api.md)\"}}\n"),
    )?;
    repo.write(
        "crates/testkit/tests/fixtures/capture/pg/README.md",
        &format!("re-record from {needle}/capture/pg\n"),
    )?;
    repo.write("docs/CONTRACT.md", &format!("reference: {needle}/\n"))?;
    repo.track()?;
    let mut findings = Vec::new();
    let count = labpath::check(repo.root(), &mut findings)?;
    assert_eq!(count, 2, "one crates/ and one xtask/ finding: {findings:?}");
    assert!(
        findings.iter().any(
            |finding| finding.starts_with("LABPATH crates/memory-postgres/tests/capture_pg.rs")
        ),
        "a test naming the plans tree is reported: {findings:?}"
    );
    assert!(
        findings
            .iter()
            .any(|finding| finding.starts_with("LABPATH xtask/src/notes.md")),
        "an xtask file naming the plans tree is reported: {findings:?}"
    );
    assert!(
        !findings
            .iter()
            .any(|finding| finding.contains("crates/config/src/paths.rs")),
        "`/rivet-shared` on its own stays legal: {findings:?}"
    );
    assert!(
        !findings
            .iter()
            .any(|finding| finding.contains("tests/golden/")
                || finding.contains("tests/fixtures/")),
        "recorded data keeps its recorded bytes: {findings:?}"
    );
    assert!(
        !findings
            .iter()
            .any(|finding| finding.starts_with("LABPATH docs/")),
        "files outside crates/ and xtask/ are not scanned: {findings:?}"
    );
    Ok(())
}

#[test]
fn labpath_current_repository_passes() -> Result<(), Box<dyn std::error::Error>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("xtask must live inside the repository")?;
    let mut findings = Vec::new();
    labpath::check(root, &mut findings)?;
    assert!(
        findings.is_empty(),
        "the current repository passes: {findings:?}"
    );
    Ok(())
}

#[test]
fn denylisted_value_is_reported() -> Result<(), Box<dyn std::error::Error>> {
    let repo = Repo::new()?;
    let uuid = "0b6e10c2-c155-bbb6-f9ea-9141ccaca9aa";
    let digest = privacy::sha256(uuid);
    repo.write(
        "xtask/data/privacy-denylist.json",
        &format!("{{\"sha256\": [\"{digest}\"]}}"),
    )?;
    repo.write("xtask/data/secret-denylist.json", "{\"sha256\": []}")?;
    repo.track()?;
    let mut findings = Vec::new();
    let priv_deny = privacy::load_hashes(repo.root(), "privacy-denylist.json", &mut findings)?;
    let sec_deny = privacy::load_hashes(repo.root(), "secret-denylist.json", &mut findings)?;

    let empty: HashSet<String> = HashSet::new();
    let mut findings = Vec::new();
    privacy::scan_text(
        &mut findings,
        &format!("let id = \"{uuid}\";\n"),
        "crates/wire/src/lib.rs",
        &empty,
        &empty,
        None,
    );
    assert!(findings.is_empty(), "empty denylists report nothing");

    let mut findings = Vec::new();
    privacy::scan_text(
        &mut findings,
        &format!("let id = \"{uuid}\";\n"),
        "crates/wire/src/lib.rs",
        &priv_deny,
        &sec_deny,
        None,
    );
    assert!(
        findings
            .iter()
            .any(|finding| finding.contains("denylist-uuid")),
        "a denylisted uuid is reported: {findings:?}"
    );

    let mut findings = Vec::new();
    privacy::scan_text(
        &mut findings,
        "let address = \"192.168.1.20\";\n", // gate-allow: the payload is the finding
        "crates/wire/src/lib.rs",
        &priv_deny,
        &sec_deny,
        None,
    );
    assert!(
        findings
            .iter()
            .any(|finding| finding.contains("private-ip")),
        "a private address is reported: {findings:?}"
    );

    let mut findings = Vec::new();
    privacy::scan_text(
        &mut findings,
        "curl -H \"Authorization: Bearer sk-abcdefghijklmnopqrstuvwx\"\n", // gate-allow: the payload is the finding
        "crates/wire/src/lib.rs",
        &priv_deny,
        &sec_deny,
        None,
    );
    assert!(
        findings.iter().any(|finding| finding.contains("secret")),
        "an API key shape is reported: {findings:?}"
    );

    let mut findings = Vec::new();
    privacy::scan_text(
        &mut findings,
        &format!("see /home/someone/notes\n{uuid}\n"),
        "docs/notes.md",
        &priv_deny,
        &sec_deny,
        None,
    );
    assert!(
        findings.iter().any(|finding| finding.contains("home-path")),
        "a foreign home path in a marker file is reported: {findings:?}"
    );
    Ok(())
}

#[test]
fn skip_scan_paths() {
    assert!(privacy::skip_scan("Cargo.lock"));
    assert!(privacy::skip_scan("xtask/data/privacy-denylist.json"));
    assert!(privacy::skip_scan("crates/wire/logo.png"));
    assert!(!privacy::skip_scan("crates/wire/src/lib.rs"));
}
