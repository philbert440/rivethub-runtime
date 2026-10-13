use kernel::memory::MemoryStoreError;
use kernel::queue::JobQueueError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("connection to the memory database failed")]
    Connection(#[source] sqlx::Error),
    #[error("schema migration failed")]
    Migrate(#[source] sqlx::migrate::MigrateError),
    #[error("the memory schema is not current: {message}")]
    Schema { message: String },
    #[error("the graphile_worker schema is absent on this database")]
    QueueAbsent,
    #[error("{operation} failed: {message}")]
    Operation {
        operation: &'static str,
        message: String,
        #[source]
        source: sqlx::Error,
    },
}

impl Error {
    pub(crate) fn operation(operation: &'static str, source: sqlx::Error) -> Self {
        Self::Operation {
            operation,
            message: source.to_string(),
            source,
        }
    }

    pub(crate) fn protocol(message: impl Into<String>) -> Self {
        Self::Connection(sqlx::Error::Protocol(message.into()))
    }

    pub(crate) fn relation_missing(source: &sqlx::Error) -> bool {
        source
            .as_database_error()
            .is_some_and(|error| error.code().as_deref() == Some("42P01"))
    }
}

impl From<Error> for MemoryStoreError {
    fn from(value: Error) -> Self {
        match value {
            Error::Connection(source) => Self::Unavailable(Box::new(source)),
            Error::Migrate(source) => Self::Unavailable(Box::new(source)),
            Error::Schema { message } => Self::Unavailable(message.into()),
            Error::QueueAbsent => Self::Unavailable(Box::new(Error::QueueAbsent)),
            Error::Operation {
                message, source, ..
            } => Self::Query {
                message,
                source: Box::new(source),
            },
        }
    }
}

impl From<Error> for JobQueueError {
    fn from(value: Error) -> Self {
        match value {
            Error::Connection(source) => Self::Unavailable(Box::new(source)),
            Error::Migrate(source) => Self::Unavailable(Box::new(source)),
            Error::Schema { message } => Self::Unavailable(message.into()),
            Error::QueueAbsent => Self::Unavailable(Box::new(Error::QueueAbsent)),
            Error::Operation {
                message, source, ..
            } => Self::Claim {
                message,
                source: Box::new(source),
            },
        }
    }
}
