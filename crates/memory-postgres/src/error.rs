use kernel::memory::MemoryStoreError;
use kernel::queue::JobQueueError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("connection to the memory database failed")]
    Connection(#[source] sqlx::Error),
    #[error(
        "the memory schema is behind: the database reports version {found}, this build requires {required} — run rivethub-migrate postgres"
    )]
    SchemaBehind { found: i64, required: i64 },
    #[error(
        "this database has no RivetHub migration history — run rivethub-migrate postgres once to adopt it"
    )]
    NotAdopted,
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
            Error::SchemaBehind { found, required } => {
                Self::Unavailable(Box::new(Error::SchemaBehind { found, required }))
            }
            Error::NotAdopted => Self::Unavailable(Box::new(Error::NotAdopted)),
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
            Error::SchemaBehind { found, required } => {
                Self::Unavailable(Box::new(Error::SchemaBehind { found, required }))
            }
            Error::NotAdopted => Self::Unavailable(Box::new(Error::NotAdopted)),
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

#[cfg(test)]
mod tests {
    use super::Error;
    use kernel::memory::MemoryStoreError;
    use kernel::queue::JobQueueError;

    #[test]
    fn schema_refusals_box_the_typed_error() {
        assert_store(Error::SchemaBehind {
            found: 3,
            required: 21,
        });
        assert_queue(Error::SchemaBehind {
            found: 3,
            required: 21,
        });
        assert_store(Error::NotAdopted);
        assert_queue(Error::NotAdopted);
    }

    fn assert_store(error: Error) {
        let text = error.to_string();
        let MemoryStoreError::Unavailable(source) = MemoryStoreError::from(error) else {
            panic!("expected Unavailable");
        };
        let typed = source.downcast::<Error>().expect("boxed typed error");
        assert_eq!(typed.to_string(), text);
    }

    fn assert_queue(error: Error) {
        let text = error.to_string();
        let JobQueueError::Unavailable(source) = JobQueueError::from(error) else {
            panic!("expected Unavailable");
        };
        let typed = source.downcast::<Error>().expect("boxed typed error");
        assert_eq!(typed.to_string(), text);
    }
}
