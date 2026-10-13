use axum::Router;
use axum::routing::get;
use rusqlite::params;
use tempfile::tempdir;
use testkit::copy_fixture;
use testkit::golden_path;
use testkit::open_fixture;
use testkit::query_json;
use testkit::rows_eq;
use testkit::sqlite_version;
use testkit::stub_server;

#[tokio::test]
async fn stub_server_answers_router_routes() {
    use tokio::io::AsyncReadExt as _;
    use tokio::io::AsyncWriteExt as _;
    let app = Router::new().route("/ping", get(|| async { "pong" }));
    let stub = stub_server(app).await.unwrap();
    let mut stream = tokio::net::TcpStream::connect(stub.addr).await.unwrap();
    stream
        .write_all(b"GET /ping HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.unwrap();
    let text = String::from_utf8_lossy(&response);
    assert!(text.starts_with("HTTP/1.1 200 OK"), "{text}");
    assert!(text.ends_with("pong"), "{text}");
    stub.close().await;
}

#[test]
fn tempdir_creates_a_dropped_directory() {
    let dir = tempdir().unwrap();
    assert!(dir.path().is_dir());
    std::fs::write(dir.path().join("probe"), b"x").unwrap();
    assert!(dir.path().join("probe").is_file());
}

#[test]
fn golden_path_joins_under_the_given_manifest_dir() {
    let path = golden_path(env!("CARGO_MANIFEST_DIR"), "tests/fixtures");
    assert!(path.is_absolute());
    assert_eq!(path.file_name().unwrap(), "fixtures");
    assert!(path.join("http.json").is_file());
}

#[test]
fn fixture_db_copies_and_queries_rows() {
    let dir = tempdir().unwrap();
    let original = dir.path().join("orig.sqlite");
    let conn = rusqlite::Connection::open(&original).unwrap();
    conn.execute_batch(
        "CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT, weight REAL, data BLOB);
         INSERT INTO items VALUES (1, 'acme', 0.5, x'00ff10');",
    )
    .unwrap();
    drop(conn);
    let fixture = open_fixture(&original).unwrap();
    fixture
        .conn
        .execute("INSERT INTO items VALUES (2, 'other', NULL, NULL)", [])
        .unwrap();
    let rows = query_json(
        &fixture.conn,
        "SELECT id, name, weight, data FROM items ORDER BY id",
        params![],
    )
    .unwrap();
    let expected = vec![
        serde_json::json!({
            "id": 1,
            "name": "acme",
            "weight": 0.5,
            "data": {"$blob_hex": "00ff10"}
        }),
        serde_json::json!({
            "id": 2,
            "name": "other",
            "weight": null,
            "data": null
        }),
    ];
    assert!(rows_eq(&rows, &expected), "{rows:?}");
    assert_eq!(
        fixture
            .conn
            .query_row("SELECT 1", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn copy_fixture_copies_into_a_private_directory() {
    let dir = tempdir().unwrap();
    let original = dir.path().join("orig.sqlite");
    let conn = rusqlite::Connection::open(&original).unwrap();
    conn.execute_batch(
        "CREATE TABLE items (id INTEGER PRIMARY KEY);
         INSERT INTO items VALUES (1);",
    )
    .unwrap();
    drop(conn);
    let copy = copy_fixture(&original).unwrap();
    assert!(copy.path().is_file());
    assert_ne!(copy.path(), original.as_path());
    assert_eq!(copy.path().file_name().unwrap(), "orig.sqlite");
    let count: i64 = rusqlite::Connection::open(copy.path())
        .unwrap()
        .query_row("SELECT count(*) FROM items", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn sqlite_version_is_reported() {
    assert_ne!(sqlite_version(), "");
}
