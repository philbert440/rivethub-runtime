use std::fmt;
use std::fs::File;
use std::fs::OpenOptions;
use std::fs::TryLockError;
use std::net::TcpListener;
use std::path::Path;

const FIRST: u16 = 20_000;
const LAST: u16 = 31_999;

#[derive(Debug)]
pub struct ClaimedPort {
    number: u16,
    _claim: File,
}

impl ClaimedPort {
    pub const fn number(&self) -> u16 {
        self.number
    }
}

impl fmt::Display for ClaimedPort {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.number.fmt(formatter)
    }
}

pub fn claim() -> std::io::Result<ClaimedPort> {
    let claims = Path::new("/tmp/rivethub-test-ports");
    std::fs::create_dir_all(claims)?;
    for number in FIRST..=LAST {
        let claim = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(claims.join(number.to_string()))?;
        match claim.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => continue,
            Err(TryLockError::Error(error)) => return Err(error),
        }
        if TcpListener::bind(("127.0.0.1", number)).is_ok() {
            return Ok(ClaimedPort {
                number,
                _claim: claim,
            });
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AddrInUse,
        format!("every port from {FIRST} to {LAST} is claimed or in use"),
    ))
}
