use crate::common;

use std::os::unix::fs::symlink;
use std::path::Path;
use std::path::PathBuf;
use std::time::UNIX_EPOCH;

use common::answer;
use tempfile::TempDir;
use testkit::Request;
use testkit::Response;
use wire::gateway::GatewayError;
use wire::gateway::files::FileChanged;
use wire::gateway::files::FileEntry;
use wire::gateway::files::FileKind;
use wire::gateway::files::FileList;

struct Tree {
    keep: TempDir,
    root: PathBuf,
    app: axum::Router,
}

fn tree() -> Result<Tree, String> {
    let keep = tempfile::tempdir().map_err(|error| error.to_string())?;
    let root = keep.path().join("files");
    std::fs::create_dir(&root).map_err(|error| error.to_string())?;
    let gateway = common::gateway_with_files(root.clone());
    Ok(Tree {
        keep,
        root,
        app: gateway.router(false),
    })
}

fn put(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    std::fs::write(path, bytes).map_err(|error| error.to_string())
}

fn read_bytes(path: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|error| error.to_string())
}

fn link_to(target: &Path, path: &Path) -> Result<(), String> {
    symlink(target, path).map_err(|error| error.to_string())
}

fn canonical(path: &Path) -> Result<String, String> {
    let full = std::fs::canonicalize(path).map_err(|error| error.to_string())?;
    match full.to_str() {
        Some(text) => Ok(text.to_owned()),
        None => Err("path is not utf-8".to_owned()),
    }
}

fn filed(path: &Path, name: &str, kind: FileKind, follow: bool) -> Result<FileEntry, String> {
    let meta = if follow {
        std::fs::metadata(path)
    } else {
        std::fs::symlink_metadata(path)
    }
    .map_err(|error| error.to_string())?;
    let since = meta
        .modified()
        .map_err(|error| error.to_string())?
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?;
    let mtime = i64::try_from(since.as_millis()).map_err(|error| error.to_string())?;
    Ok(FileEntry {
        name: name.to_owned(),
        kind,
        size: meta.len(),
        mtime,
    })
}

async fn call(
    app: &axum::Router,
    method: &str,
    target: &str,
    body: &[u8],
) -> Result<Response, String> {
    answer(
        app.clone(),
        Request {
            method: method.to_owned(),
            target: target.to_owned(),
            headers: vec![("host".to_owned(), "127.0.0.1:5174".to_owned())],
            body: body.to_vec(),
            recipe: None,
            via: None,
        },
    )
    .await
}

fn expect_error(response: &Response, status: u16, message: &str) -> Result<(), String> {
    if response.status != status {
        let body = String::from_utf8_lossy(&response.body);
        return Err(format!("status {} body {body}", response.status));
    }
    let parsed: GatewayError =
        serde_json::from_slice(&response.body).map_err(|error| error.to_string())?;
    if parsed.error != message {
        return Err(format!("{} != {message}", parsed.error));
    }
    Ok(())
}

fn listed(response: &Response) -> Result<FileList, String> {
    if response.status != 200 {
        let body = String::from_utf8_lossy(&response.body);
        return Err(format!("status {} body {body}", response.status));
    }
    serde_json::from_slice(&response.body).map_err(|error| error.to_string())
}

fn changed_path(response: &Response) -> Result<String, String> {
    if response.status != 200 {
        let body = String::from_utf8_lossy(&response.body);
        return Err(format!("status {} body {body}", response.status));
    }
    let parsed: FileChanged =
        serde_json::from_slice(&response.body).map_err(|error| error.to_string())?;
    if !parsed.ok {
        return Err("change was not ok".to_owned());
    }
    Ok(parsed.path)
}

fn header<'a>(response: &'a Response, name: &str) -> Result<&'a str, String> {
    response
        .headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
        .ok_or_else(|| format!("missing {name}"))
}

fn populate(tree: &Tree) -> Result<(), String> {
    put(&tree.root.join("ok.txt"), b"yes")?;
    put(&tree.root.join("note~1.txt"), b"tilde")?;
    put(&tree.root.join(".gitignore"), b"git")?;
    put(&tree.root.join("foo.part-12-3-4"), b"visible")?;
    put(&tree.root.join(".keep.part-1-2"), b"hidden")?;
    put(&tree.root.join("nested/leaf.txt"), b"leaf")?;
    put(&tree.root.join("files-evil/inside.txt"), b"inside")?;
    let evil = tree.keep.path().join("files-evil");
    std::fs::create_dir(&evil).map_err(|error| error.to_string())?;
    put(&evil.join("secret"), b"secret")?;
    let outside_dir = tree.keep.path().join("outside-dir");
    put(&outside_dir.join("canary"), b"CANARY")?;
    link_to(&evil.join("secret"), &tree.root.join("out"))?;
    link_to(&evil, &tree.root.join("outdir"))?;
    link_to(&tree.root.join("nested"), &tree.root.join("aliasdir"))?;
    link_to(&tree.root.join("ok.txt"), &tree.root.join("alias.txt"))?;
    link_to(&tree.root.join("missing-target"), &tree.root.join("broken"))?;
    Ok(())
}

#[tokio::test]
async fn list_sorts_directories_first_and_hides_part_files() -> Result<(), String> {
    let tree = tree()?;
    populate(&tree)?;
    let body = listed(&call(&tree.app, "GET", "/files/list", &[]).await?)?;
    let expected = FileList {
        path: String::new(),
        root: canonical(&tree.root)?,
        entries: vec![
            filed(&tree.root.join("aliasdir"), "aliasdir", FileKind::Dir, true)?,
            filed(
                &tree.root.join("files-evil"),
                "files-evil",
                FileKind::Dir,
                false,
            )?,
            filed(&tree.root.join("nested"), "nested", FileKind::Dir, false)?,
            filed(
                &tree.root.join(".gitignore"),
                ".gitignore",
                FileKind::File,
                false,
            )?,
            filed(
                &tree.root.join("alias.txt"),
                "alias.txt",
                FileKind::File,
                true,
            )?,
            filed(&tree.root.join("broken"), "broken", FileKind::File, false)?,
            filed(
                &tree.root.join("foo.part-12-3-4"),
                "foo.part-12-3-4",
                FileKind::File,
                false,
            )?,
            filed(
                &tree.root.join("note~1.txt"),
                "note~1.txt",
                FileKind::File,
                false,
            )?,
            filed(&tree.root.join("ok.txt"), "ok.txt", FileKind::File, false)?,
            filed(&tree.root.join("out"), "out", FileKind::File, false)?,
            filed(&tree.root.join("outdir"), "outdir", FileKind::File, false)?,
        ],
    };
    assert_eq!(body, expected);
    let nested = listed(&call(&tree.app, "GET", "/files/list?path=nested", &[]).await?)?;
    assert_eq!(nested.path, "nested");
    assert_eq!(nested.root, canonical(&tree.root)?);
    assert_eq!(
        nested.entries,
        vec![filed(
            &tree.root.join("nested/leaf.txt"),
            "leaf.txt",
            FileKind::File,
            false
        )?]
    );
    let missing = call(&tree.app, "GET", "/files/list?path=missing", &[]).await?;
    expect_error(&missing, 404, "no such directory")?;
    let file = call(&tree.app, "GET", "/files/list?path=ok.txt", &[]).await?;
    expect_error(&file, 404, "no such directory")?;
    Ok(())
}

#[tokio::test]
async fn escapes_are_refused_and_contained_names_download() -> Result<(), String> {
    let tree = tree()?;
    populate(&tree)?;
    let refused = [
        "/files/list?path=out",
        "/files/download?path=out",
        "/files/list?path=outdir",
        "/files/download?path=outdir/secret",
        "/files/download?path=../files-evil/secret",
        "/files/download?path=/etc/passwd",
        "/files/download?path=~/secret",
        "/files/download?path=~",
        "/files/list?path=%2e%2e%2ffiles-evil",
        "/files/download?path=a%5Cb",
        "/files/download?path=%00",
    ];
    for target in refused {
        let response = call(&tree.app, "GET", target, &[]).await?;
        expect_error(&response, 403, "path escapes the files root")?;
    }
    assert_eq!(
        read_bytes(&tree.keep.path().join("files-evil/secret"))?,
        b"secret"
    );
    let ok = call(&tree.app, "GET", "/files/download?path=ok.txt", &[]).await?;
    assert_eq!(ok.status, 200);
    assert_eq!(ok.body, b"yes");
    assert_eq!(header(&ok, "content-type")?, "text/plain; charset=utf-8");
    assert_eq!(
        header(&ok, "content-disposition")?,
        "inline; filename=\"ok.txt\""
    );
    assert_eq!(header(&ok, "x-content-type-options")?, "nosniff");
    let nested = call(
        &tree.app,
        "GET",
        "/files/download?path=nested/leaf.txt",
        &[],
    )
    .await?;
    assert_eq!(nested.body, b"leaf");
    let tilde = call(&tree.app, "GET", "/files/download?path=note~1.txt", &[]).await?;
    assert_eq!(tilde.body, b"tilde");
    let inside = call(
        &tree.app,
        "GET",
        "/files/download?path=files-evil/inside.txt",
        &[],
    )
    .await?;
    assert_eq!(inside.body, b"inside");
    let followed = call(&tree.app, "GET", "/files/download?path=alias.txt", &[]).await?;
    assert_eq!(followed.body, b"yes");
    put(&tree.root.join("foo/~"), b"tilde-name")?;
    let named = call(&tree.app, "GET", "/files/download?path=foo%2F~", &[]).await?;
    assert_eq!(named.status, 200);
    assert_eq!(named.body, b"tilde-name");
    let directory = call(&tree.app, "GET", "/files/download?path=nested", &[]).await?;
    expect_error(&directory, 404, "no such file")?;
    let broken = call(&tree.app, "GET", "/files/download?path=broken", &[]).await?;
    expect_error(&broken, 404, "no such file")?;
    Ok(())
}

#[tokio::test]
async fn inline_types_are_a_whitelist() -> Result<(), String> {
    let tree = tree()?;
    let cases = [
        (
            "page.html",
            "application/octet-stream",
            "attachment; filename=\"page.html\"",
        ),
        (
            "pic.svg",
            "application/octet-stream",
            "attachment; filename=\"pic.svg\"",
        ),
        ("pic.png", "image/png", "inline; filename=\"pic.png\""),
        ("shot.PNG", "image/png", "inline; filename=\"shot.PNG\""),
        (
            "note.md",
            "text/plain; charset=utf-8",
            "inline; filename=\"note.md\"",
        ),
        (
            "page.HTML",
            "application/octet-stream",
            "attachment; filename=\"page.HTML\"",
        ),
    ];
    for (name, content_type, disposition) in cases {
        put(&tree.root.join(name), name.as_bytes())?;
        let response = call(
            &tree.app,
            "GET",
            &format!("/files/download?path={name}"),
            &[],
        )
        .await?;
        assert_eq!(response.status, 200);
        assert_eq!(response.body, name.as_bytes());
        assert_eq!(header(&response, "content-type")?, content_type);
        assert_eq!(header(&response, "content-disposition")?, disposition);
        assert_eq!(header(&response, "x-content-type-options")?, "nosniff");
    }
    put(&tree.root.join("café.txt"), b"cafe")?;
    let utf8 = call(&tree.app, "GET", "/files/download?path=caf%C3%A9.txt", &[]).await?;
    assert_eq!(utf8.body, b"cafe");
    assert_eq!(
        header(&utf8, "content-disposition")?,
        "inline; filename*=UTF-8''caf%C3%A9.txt"
    );
    let status = std::process::Command::new("mkfifo")
        .arg(tree.root.join("pipe"))
        .status()
        .map_err(|error| error.to_string())?;
    assert!(status.success());
    let pipe = call(&tree.app, "GET", "/files/download?path=pipe", &[]).await?;
    expect_error(&pipe, 404, "no such file")?;
    let big = tree.root.join("big.bin");
    let sparse = std::fs::File::create(&big).map_err(|error| error.to_string())?;
    sparse
        .set_len((1024 * 1024 * 1024) + 1)
        .map_err(|error| error.to_string())?;
    let over = call(&tree.app, "GET", "/files/download?path=big.bin", &[]).await?;
    expect_error(&over, 413, "download exceeds 1 GiB cap")?;
    Ok(())
}

#[tokio::test]
async fn upload_keeps_bytes_until_overwrite() -> Result<(), String> {
    let tree = tree()?;
    let created = call(&tree.app, "POST", "/files/upload?name=hello.txt", b"hi").await?;
    assert_eq!(changed_path(&created)?, "hello.txt");
    assert_eq!(read_bytes(&tree.root.join("hello.txt"))?, b"hi");
    let clash = call(&tree.app, "POST", "/files/upload?name=hello.txt", b"no").await?;
    expect_error(&clash, 409, "file exists")?;
    let flagged = call(
        &tree.app,
        "POST",
        "/files/upload?name=hello.txt&overwrite=true",
        b"no",
    )
    .await?;
    expect_error(&flagged, 409, "file exists")?;
    assert_eq!(read_bytes(&tree.root.join("hello.txt"))?, b"hi");
    let replaced = call(
        &tree.app,
        "POST",
        "/files/upload?name=hello.txt&overwrite=1",
        b"yo",
    )
    .await?;
    assert_eq!(changed_path(&replaced)?, "hello.txt");
    let downloaded = call(&tree.app, "GET", "/files/download?path=hello.txt", &[]).await?;
    assert_eq!(downloaded.body, b"yo");
    let names = std::fs::read_dir(&tree.root).map_err(|error| error.to_string())?;
    for item in names {
        let item = item.map_err(|error| error.to_string())?;
        let name = item
            .file_name()
            .into_string()
            .map_err(|raw| raw.to_string_lossy().into_owned())?;
        assert!(!name.contains(".part-"), "{name}");
    }
    Ok(())
}

#[tokio::test]
async fn upload_does_not_follow_a_symlink_at_the_temp_name() -> Result<(), String> {
    let tree = tree()?;
    let outside = tree.keep.path().join("secret");
    put(&outside, b"SECRET")?;
    let name = "planted.txt";
    let pid = std::process::id();
    for seq in 1..=128 {
        link_to(
            &outside,
            &tree.root.join(format!(".{name}.part-{pid}-{seq}")),
        )?;
    }
    let response = call(
        &tree.app,
        "POST",
        &format!("/files/upload?name={name}"),
        b"pwned",
    )
    .await?;
    expect_error(&response, 403, "path escapes the files root")?;
    assert_eq!(read_bytes(&outside)?, b"SECRET");
    assert!(!tree.root.join(name).exists());
    Ok(())
}

#[tokio::test]
async fn upload_rejects_bad_names_and_escapes() -> Result<(), String> {
    let tree = tree()?;
    populate(&tree)?;
    let bad = [
        "/files/upload",
        "/files/upload?name=",
        "/files/upload?name=.",
        "/files/upload?name=..",
        "/files/upload?name=a%2Fb",
        "/files/upload?name=a%5Cb",
        "/files/upload?name=%00",
    ];
    for target in bad {
        let response = call(&tree.app, "POST", target, b"x").await?;
        expect_error(&response, 422, "invalid file name")?;
    }
    let missing = call(
        &tree.app,
        "POST",
        "/files/upload?dir=missing&name=a.txt",
        b"x",
    )
    .await?;
    expect_error(&missing, 404, "no such directory")?;
    let escaped = call(&tree.app, "POST", "/files/upload?dir=..&name=a.txt", b"x").await?;
    expect_error(&escaped, 403, "path escapes the files root")?;
    let linked = call(
        &tree.app,
        "POST",
        "/files/upload?dir=outdir&name=a.txt",
        b"x",
    )
    .await?;
    expect_error(&linked, 403, "path escapes the files root")?;
    assert_eq!(
        read_bytes(&tree.keep.path().join("files-evil/secret"))?,
        b"secret"
    );
    let made = call(&tree.app, "POST", "/files/mkdir?name=docs", &[]).await?;
    assert_eq!(changed_path(&made)?, "docs");
    let onto = call(&tree.app, "POST", "/files/upload?name=docs", b"x").await?;
    expect_error(&onto, 409, "directory exists")?;
    let hidden = call(&tree.app, "POST", "/files/upload?name=.x.part-1-2", b"zz").await?;
    assert_eq!(changed_path(&hidden)?, ".x.part-1-2");
    let body = listed(&call(&tree.app, "GET", "/files/list", &[]).await?)?;
    assert!(body.entries.iter().all(|entry| entry.name != ".x.part-1-2"));
    assert_eq!(read_bytes(&tree.root.join(".x.part-1-2"))?, b"zz");
    Ok(())
}

#[tokio::test]
async fn mkdir_rejects_bad_names_and_collisions() -> Result<(), String> {
    let tree = tree()?;
    let bad = [
        "/files/mkdir",
        "/files/mkdir?name=",
        "/files/mkdir?name=.",
        "/files/mkdir?name=..",
        "/files/mkdir?name=a%2Fb",
        "/files/mkdir?name=a%5Cb",
        "/files/mkdir?name=%00",
    ];
    for target in bad {
        let response = call(&tree.app, "POST", target, &[]).await?;
        expect_error(&response, 422, "invalid directory name")?;
    }
    let missing = call(
        &tree.app,
        "POST",
        "/files/mkdir?dir=missing&name=child",
        &[],
    )
    .await?;
    expect_error(&missing, 404, "no such directory")?;
    let escaped = call(&tree.app, "POST", "/files/mkdir?dir=..&name=child", &[]).await?;
    expect_error(&escaped, 403, "path escapes the files root")?;
    let made = call(&tree.app, "POST", "/files/mkdir?name=docs", &[]).await?;
    assert_eq!(changed_path(&made)?, "docs");
    let again = call(&tree.app, "POST", "/files/mkdir?name=docs", &[]).await?;
    expect_error(&again, 409, "directory exists")?;
    let child = call(&tree.app, "POST", "/files/mkdir?dir=docs&name=child", &[]).await?;
    assert_eq!(changed_path(&child)?, "docs/child");
    let body = listed(&call(&tree.app, "GET", "/files/list?path=docs", &[]).await?)?;
    assert_eq!(body.entries.len(), 1);
    assert_eq!(
        body.entries.first().map(|entry| entry.name.as_str()),
        Some("child")
    );
    assert_eq!(
        body.entries.first().map(|entry| entry.kind),
        Some(FileKind::Dir)
    );
    put(&tree.root.join("taken.txt"), b"t")?;
    let file = call(&tree.app, "POST", "/files/mkdir?name=taken.txt", &[]).await?;
    expect_error(&file, 409, "directory exists")?;
    Ok(())
}

#[tokio::test]
async fn rename_moves_and_refuses_root_and_into_self() -> Result<(), String> {
    let tree = tree()?;
    put(&tree.root.join("a.txt"), b"alpha")?;
    let missing = call(&tree.app, "POST", "/files/rename", &[]).await?;
    expect_error(&missing, 422, "from and to are required")?;
    let half = call(&tree.app, "POST", "/files/rename?from=a.txt", &[]).await?;
    expect_error(&half, 422, "from and to are required")?;
    let empty = call(&tree.app, "POST", "/files/rename?from=&to=b.txt", &[]).await?;
    expect_error(&empty, 422, "from and to are required")?;
    let same = call(&tree.app, "POST", "/files/rename?from=a.txt&to=a.txt", &[]).await?;
    expect_error(&same, 422, "from and to are the same path")?;
    let root = call(&tree.app, "POST", "/files/rename?from=.&to=z", &[]).await?;
    expect_error(&root, 403, "cannot rename the files root")?;
    let onto_root = call(&tree.app, "POST", "/files/rename?from=a.txt&to=.", &[]).await?;
    expect_error(&onto_root, 403, "cannot rename the files root")?;
    let absent = call(&tree.app, "POST", "/files/rename?from=missing&to=z", &[]).await?;
    expect_error(&absent, 404, "no such path")?;
    let parent = call(&tree.app, "POST", "/files/rename?from=a.txt&to=nope/z", &[]).await?;
    expect_error(&parent, 404, "destination directory does not exist")?;
    std::fs::create_dir(tree.root.join("docs")).map_err(|error| error.to_string())?;
    put(&tree.root.join("docs/keep.txt"), b"k")?;
    let into = call(
        &tree.app,
        "POST",
        "/files/rename?from=docs&to=docs%2Finner",
        &[],
    )
    .await?;
    expect_error(&into, 422, "cannot move a directory into itself")?;
    assert!(tree.root.join("docs/keep.txt").is_file());
    let onto_dir = call(&tree.app, "POST", "/files/rename?from=a.txt&to=docs", &[]).await?;
    expect_error(&onto_dir, 409, "directory exists")?;
    assert_eq!(read_bytes(&tree.root.join("a.txt"))?, b"alpha");
    let moved = call(
        &tree.app,
        "POST",
        "/files/rename?from=docs&to=docs-old",
        &[],
    )
    .await?;
    assert_eq!(changed_path(&moved)?, "docs-old");
    assert_eq!(read_bytes(&tree.root.join("docs-old/keep.txt"))?, b"k");
    let file = call(&tree.app, "POST", "/files/rename?from=a.txt&to=b.txt", &[]).await?;
    assert_eq!(changed_path(&file)?, "b.txt");
    assert_eq!(read_bytes(&tree.root.join("b.txt"))?, b"alpha");
    Ok(())
}

#[tokio::test]
async fn rename_through_a_symlink_into_the_directory_is_refused() -> Result<(), String> {
    let tree = tree()?;
    std::fs::create_dir(tree.root.join("docs")).map_err(|error| error.to_string())?;
    put(&tree.root.join("docs/keep.txt"), b"k")?;
    link_to(&tree.root.join("docs"), &tree.root.join("alias"))?;
    let into = call(
        &tree.app,
        "POST",
        "/files/rename?from=docs&to=alias%2Finner",
        &[],
    )
    .await?;
    expect_error(&into, 422, "cannot move a directory into itself")?;
    assert_eq!(read_bytes(&tree.root.join("docs/keep.txt"))?, b"k");
    assert!(tree.root.join("docs").is_dir());
    Ok(())
}

#[tokio::test]
async fn delete_requires_recursive_one() -> Result<(), String> {
    let tree = tree()?;
    put(&tree.root.join("gone.txt"), b"g")?;
    let required = call(&tree.app, "DELETE", "/files/delete", &[]).await?;
    expect_error(&required, 422, "path is required")?;
    let empty = call(&tree.app, "DELETE", "/files/delete?path=", &[]).await?;
    expect_error(&empty, 422, "path is required")?;
    let root = call(&tree.app, "DELETE", "/files/delete?path=.", &[]).await?;
    expect_error(&root, 403, "cannot delete the files root")?;
    let absent = call(&tree.app, "DELETE", "/files/delete?path=missing", &[]).await?;
    expect_error(&absent, 404, "no such path")?;
    let file = call(&tree.app, "DELETE", "/files/delete?path=gone.txt", &[]).await?;
    assert_eq!(changed_path(&file)?, "gone.txt");
    assert!(!tree.root.join("gone.txt").exists());
    let made = call(&tree.app, "POST", "/files/mkdir?name=empty", &[]).await?;
    assert_eq!(changed_path(&made)?, "empty");
    let removed = call(&tree.app, "DELETE", "/files/delete?path=empty", &[]).await?;
    assert_eq!(changed_path(&removed)?, "empty");
    std::fs::create_dir(tree.root.join("full")).map_err(|error| error.to_string())?;
    put(&tree.root.join("full/a.txt"), b"a")?;
    let blocked = call(&tree.app, "DELETE", "/files/delete?path=full", &[]).await?;
    expect_error(&blocked, 409, "directory not empty (pass recursive=1)")?;
    let word = call(
        &tree.app,
        "DELETE",
        "/files/delete?path=full&recursive=true",
        &[],
    )
    .await?;
    expect_error(&word, 409, "directory not empty (pass recursive=1)")?;
    assert!(tree.root.join("full/a.txt").is_file());
    let gone = call(
        &tree.app,
        "DELETE",
        "/files/delete?path=full&recursive=1",
        &[],
    )
    .await?;
    assert_eq!(changed_path(&gone)?, "full");
    assert!(!tree.root.join("full").exists());
    Ok(())
}

#[tokio::test]
async fn delete_of_a_symlink_leaves_the_outside_target() -> Result<(), String> {
    let tree = tree()?;
    let outside = tree.keep.path().join("secret");
    put(&outside, b"SECRET")?;
    link_to(&outside, &tree.root.join("link"))?;
    let listed_root = listed(&call(&tree.app, "GET", "/files/list", &[]).await?)?;
    assert_eq!(
        listed_root.entries,
        vec![filed(
            &tree.root.join("link"),
            "link",
            FileKind::File,
            false
        )?]
    );
    let fetched = call(&tree.app, "GET", "/files/download?path=link", &[]).await?;
    expect_error(&fetched, 403, "path escapes the files root")?;
    assert_eq!(read_bytes(&outside)?, b"SECRET");
    let removed = call(&tree.app, "DELETE", "/files/delete?path=link", &[]).await?;
    assert_eq!(changed_path(&removed)?, "link");
    assert!(!tree.root.join("link").exists());
    assert_eq!(read_bytes(&outside)?, b"SECRET");
    let outside_dir = tree.keep.path().join("outside-dir");
    put(&outside_dir.join("canary"), b"CANARY")?;
    std::fs::create_dir(tree.root.join("box")).map_err(|error| error.to_string())?;
    link_to(&outside, &tree.root.join("box/link"))?;
    link_to(&outside_dir, &tree.root.join("box/dirlink"))?;
    let gone = call(
        &tree.app,
        "DELETE",
        "/files/delete?path=box&recursive=1",
        &[],
    )
    .await?;
    assert_eq!(changed_path(&gone)?, "box");
    assert!(!tree.root.join("box").exists());
    assert_eq!(read_bytes(&outside)?, b"SECRET");
    assert_eq!(read_bytes(&outside_dir.join("canary"))?, b"CANARY");
    Ok(())
}

#[tokio::test]
async fn disabled_wins_over_method_and_unknown_paths_are_not_found() -> Result<(), String> {
    let disabled = common::gateway().0.router(false);
    let posted = call(&disabled, "POST", "/files/list", &[]).await?;
    expect_error(&posted, 503, "files browser disabled on this node")?;
    let unknown = call(&disabled, "GET", "/files/nope?path=../x", &[]).await?;
    expect_error(&unknown, 503, "files browser disabled on this node")?;
    for target in ["/files", "/files/", "/api/files", "/api/files/"] {
        let response = call(&disabled, "GET", target, &[]).await?;
        expect_error(&response, 404, "not found")?;
    }
    let empty = common::gateway_with_files(PathBuf::new()).router(false);
    let off = call(&empty, "GET", "/files/list", &[]).await?;
    expect_error(&off, 503, "files browser disabled on this node")?;
    let keep = tempfile::tempdir().map_err(|error| error.to_string())?;
    let missing = keep.path().join("missing-files-root");
    let unavailable = common::gateway_with_files(missing.clone()).router(false);
    let response = call(&unavailable, "GET", "/api/files/list", &[]).await?;
    expect_error(
        &response,
        503,
        &format!("files root not available: {}", missing.display()),
    )?;
    let tree = tree()?;
    let method = call(&tree.app, "POST", "/files/list", &[]).await?;
    expect_error(&method, 405, "method not allowed")?;
    let wrong = call(&tree.app, "GET", "/files/delete?path=a", &[]).await?;
    expect_error(&wrong, 405, "method not allowed")?;
    for target in ["/files", "/files/", "/api/files", "/files/nope"] {
        let response = call(&tree.app, "GET", target, &[]).await?;
        expect_error(&response, 404, "not found")?;
    }
    let alias = listed(&call(&tree.app, "GET", "/api/files/list", &[]).await?)?;
    assert_eq!(alias.path, "");
    assert_eq!(alias.root, canonical(&tree.root)?);
    assert_eq!(alias.entries, Vec::new());
    Ok(())
}
