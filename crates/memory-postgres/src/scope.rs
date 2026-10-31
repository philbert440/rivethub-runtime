use kernel::tenancy::Scope;
use sqlx::PgConnection;
use sqlx::postgres::PgPoolOptions;

const MAINTENANCE_ON: &str = "on";
const MAINTENANCE_OFF: &str = "off";

struct Context {
    principal: String,
    spaces: String,
    maintenance: &'static str,
}

fn context(scope: Option<Scope>) -> Context {
    match scope {
        Some(Scope::Viewer(viewer)) => Context {
            principal: viewer.principal().as_str().to_owned(),
            spaces: viewer
                .spaces()
                .iter()
                .map(kernel::tenancy::SpaceId::as_str)
                .collect::<Vec<_>>()
                .join(","),
            maintenance: MAINTENANCE_OFF,
        },
        Some(Scope::Maintenance) => Context {
            principal: String::new(),
            spaces: String::new(),
            maintenance: MAINTENANCE_ON,
        },
        Some(Scope::Closed) => Context {
            principal: String::new(),
            spaces: String::new(),
            maintenance: MAINTENANCE_OFF,
        },
        None => {
            tracing::debug!("database connection taken outside a tenancy scope; it sees no rows");
            Context {
                principal: String::new(),
                spaces: String::new(),
                maintenance: MAINTENANCE_OFF,
            }
        }
    }
}

pub(crate) async fn apply(conn: &mut PgConnection) -> Result<(), sqlx::Error> {
    let context = context(Scope::current());
    sqlx::query(
        "SELECT set_config('rh.principal', $1, false), set_config('rh.spaces', $2, false), \
                set_config('rh.maintenance', $3, false), set_config('jit', 'off', false)",
    )
    .bind(context.principal)
    .bind(context.spaces)
    .bind(context.maintenance)
    .execute(conn)
    .await
    .map(drop)
}

#[must_use]
pub fn scoped(options: PgPoolOptions) -> PgPoolOptions {
    options
        .after_connect(|conn, _| Box::pin(apply(conn)))
        .before_acquire(|conn, _| Box::pin(async move { apply(conn).await.map(|()| true) }))
}
