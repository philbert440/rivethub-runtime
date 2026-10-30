#![cfg(test)]

use crate::support;

use cli::Exit;
use sqlx::ConnectOptions as _;
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn propose_refuses_the_live_database_under_another_url(pool: PgPool) {
    let live: url::Url = pool.connect_options().as_ref().to_url_lossy();
    let name = live.path().trim_start_matches('/').to_owned();
    let mut letters = name.bytes();
    let first = letters.next().expect("a database name");
    let rest = String::from_utf8(letters.collect()).expect("utf-8 name");
    let mut disguised = live.clone();
    disguised.set_path(&format!("/%{first:02X}{rest}"));
    let home = support::home();
    let environment = support::environment(&home).with_override("RIVETHUB_PG_URL", live.as_str());
    let refused = propose(&environment, disguised.as_str()).await;
    assert_eq!(refused.exit, Exit::Failure, "{}", refused.stdout);
    assert!(refused.stderr.contains(LIVE), "{}", refused.stderr);
    let mut elsewhere = live.clone();
    elsewhere.set_path("/postgres");
    let other = propose(&environment, elsewhere.as_str()).await;
    assert!(!other.stderr.contains(LIVE), "{}", other.stderr);
}

const LIVE: &str = "refusing to write cleanup proposals to the node's live memory database";

async fn propose(environment: &config::Environment, database: &str) -> cli::Report {
    let out = tempfile::tempdir().expect("out folder");
    let parts = [
        "cleanup",
        "--propose",
        "--database",
        database,
        "--out",
        out.path().to_str().expect("utf-8 path"),
        "--model-url",
        "http://127.0.0.1:9/v1",
        "--model",
        "unused",
    ];
    let args: Vec<String> = parts.iter().map(|part| (*part).to_owned()).collect();
    cli::memory::execute(&args, environment, &CancellationToken::new(), false).await
}
