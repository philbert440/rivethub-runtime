#[path = "tls_replay/audit.rs"]
mod audit;
#[path = "tls_replay/certificates.rs"]
mod certificates;
#[path = "tls_replay/compare.rs"]
mod compare;
#[path = "tls_replay/fixture.rs"]
mod fixture;
pub mod process;
#[path = "tls_replay/recording.rs"]
mod recording;

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error(transparent)]
    Format(#[from] std::fmt::Error),
    #[error(transparent)]
    System(#[from] nix::errno::Errno),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Certificate(#[from] rcgen::Error),
    #[error(transparent)]
    Request(#[from] reqwest::Error),
    #[error(transparent)]
    Method(#[from] axum::http::method::InvalidMethod),
    #[error(transparent)]
    Header(#[from] axum::http::header::ToStrError),
    #[error(transparent)]
    Task(#[from] tokio::task::JoinError),
    #[error(transparent)]
    Migrate(#[from] migrate::Error),
    #[error("process: {0}")]
    Process(String),
    #[error("fixture: {0}")]
    Fixture(String),
    #[error("{0}")]
    Mismatch(String),
}

#[test]
fn tls_skip_inventory_is_unique_reasoned_and_scoped() -> Result<(), Error> {
    recording::inventory(&recording::recording()?, &recording::skips()?)
}

#[sqlx::test(migrations = false)]
async fn recorded_tls_cases_against_composed_gateway(pool: sqlx::PgPool) -> Result<(), Error> {
    migrate::postgres_pool(&pool).await?;
    let recording = recording::recording()?;
    let skips = recording::skips()?;
    recording::inventory(&recording, &skips)?;
    let mut failures = Vec::new();
    for (name, run) in recording.runs {
        let kind = recording::RunKind::parse(&name)?;
        let fixture = fixture::prepare(&pool, kind).await?;
        let running = process::Running::start_with_environment(
            fixture.home.path(),
            &[("RIVETHUB_NODE_NAME", "golden-node")],
        )
        .await
        .map_err(|error| Error::Process(error.to_string()))?;
        let masks = testkit::json::Masks {
            port: running.address.port().to_string(),
            hostname: Some(
                nix::unistd::gethostname()?
                    .into_string()
                    .map_err(|_| Error::Fixture("hostname is not UTF-8".into()))?,
            ),
            tmp: Some(fixture.home.path().display().to_string()),
        };
        for case in run.cases {
            let id = format!("{name}/{}", case.id);
            let actual = compare::send(&fixture, running.address.port(), &case, &masks).await;
            if let Err(error) = audit::check(&id, &case, actual, &masks, skips.get(&id)) {
                failures.push(format!("{id}: {error}"));
            }
        }
        running
            .stop(nix::sys::signal::Signal::SIGTERM)
            .await
            .map_err(|error| Error::Process(error.to_string()))?;
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(Error::Mismatch(failures.join("\n")))
    }
}
