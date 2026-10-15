use std::error::Error as _;

use testkit::http::Response;
use testkit::json::Masks;

use super::Error;
use super::recording::{Case, ExpectedFailure, Skip};

pub(super) fn check(
    id: &str,
    case: &Case,
    actual: Result<Response, Error>,
    masks: &Masks,
    skip: Option<&Skip>,
) -> Result<(), Error> {
    let Some(skip) = skip else {
        return super::compare::compare(id, case, actual?, masks);
    };
    match (&skip.expected, actual) {
        (ExpectedFailure::Http { status }, Ok(actual)) => {
            if *status != actual.status {
                return Err(Error::Mismatch(format!(
                    "{id}: skip expected status {status}, received {}",
                    actual.status
                )));
            }
            match super::compare::compare(id, case, actual, masks) {
                Ok(()) => Err(Error::Mismatch(format!(
                    "{id}: stale skip: {}",
                    skip.reason
                ))),
                Err(Error::Mismatch(_)) => Ok(()),
                Err(error) => Err(error),
            }
        }
        (ExpectedFailure::Tls { alert }, Err(Error::Request(error))) => {
            let mut source = error.source();
            while let Some(cause) = source {
                let tls = cause.downcast_ref::<rustls::Error>().or_else(|| {
                    cause
                        .downcast_ref::<std::io::Error>()?
                        .get_ref()?
                        .downcast_ref::<rustls::Error>()
                });
                if let Some(rustls::Error::AlertReceived(actual)) = tls {
                    if *actual == alert.alert() {
                        return Ok(());
                    }
                    return Err(Error::Mismatch(format!(
                        "{id}: unexpected TLS alert {actual:?}"
                    )));
                }
                source = cause.source();
            }
            Err(Error::Mismatch(format!(
                "{id}: expected certificate alert, received {error:?}"
            )))
        }
        (ExpectedFailure::Tls { .. }, Ok(actual)) => Err(Error::Mismatch(format!(
            "{id}: expected TLS refusal, received HTTP {}",
            actual.status
        ))),
        (ExpectedFailure::Http { .. } | ExpectedFailure::Tls { .. }, Err(error)) => Err(error),
    }
}
