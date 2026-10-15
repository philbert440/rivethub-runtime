use std::path::Path;

use kernel::terminal::TermSize;
use kernel::terminal::TerminalId;

use crate::error::Error;
use crate::herdr::HerdrSession;
use crate::herdr::namespace;
use crate::herdr::namespace::OwnedSession;
use crate::herdr::namespace::Scan;
use crate::herdr::namespace::SkipRule;
use crate::manager::Backing;
use crate::manager::TerminalManager;
use crate::manager::registry::RecordFields;
use crate::manager::registry::Runtime;
use crate::mux::MuxPlan;
use crate::session::SessionKey;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdoptedSession {
    pub id: TerminalId,
    pub session: SessionKey,
    pub roster_key: crate::session::RosterKey,
    pub argv: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Adoption {
    pub adopted: Vec<AdoptedSession>,
    pub inactive: Vec<SessionKey>,
    pub skipped: std::collections::BTreeSet<SkipRule>,
}

impl TerminalManager {
    pub async fn adopt_namespace(&self, state_dir: &Path, owner: &str) -> Result<Adoption, Error> {
        if owner.is_empty() {
            return Err(Error::OwnerUnset);
        }
        *self.inner.owner.lock() = Some(owner.to_owned());
        let setup = match &self.inner.plan {
            MuxPlan::Direct => return Ok(Adoption::default()),
            MuxPlan::Unavailable(unavailable) => return Err(unavailable.reason.to_error()),
            MuxPlan::Herdr(setup) => setup.clone(),
        };
        let state_dir = state_dir.to_path_buf();
        let owner = owner.to_owned();
        let uid = nix::unistd::Uid::current().as_raw();
        let config_home = setup.config_home.clone();
        let scanned = tokio::task::spawn_blocking(move || {
            namespace::scan(
                &state_dir,
                &config_home,
                &owner,
                &namespace::HostRoots {
                    runtime: Path::new("/run/user"),
                    temp: Path::new("/tmp"),
                    uid,
                },
            )
        })
        .await
        .map_err(Error::Worker)??;
        let manager = self.clone();
        self.inner
            .tasks
            .spawn_blocking(move || {
                manager.install_owned(&setup.binary, &setup.config_home, scanned)
            })
            .await?
    }

    pub fn release(&self, id: &TerminalId) {
        self.inner.state.lock().remove(id);
        self.unbind_session(id);
    }

    fn install_owned(
        &self,
        binary: &Path,
        config_home: &Path,
        scanned: Scan,
    ) -> Result<Adoption, Error> {
        let mut adopted = Vec::with_capacity(scanned.owned.len());
        for owned in scanned.owned {
            adopted.push(self.insert_owned(binary, config_home, owned)?);
        }
        Ok(Adoption {
            adopted,
            inactive: scanned.inactive,
            skipped: scanned.skipped,
        })
    }

    fn insert_owned(
        &self,
        binary: &Path,
        config_home: &Path,
        owned: OwnedSession,
    ) -> Result<AdoptedSession, Error> {
        let setup = crate::mux::HerdrSetup {
            binary: binary.to_path_buf(),
            config_home: config_home.to_path_buf(),
        };
        let session = HerdrSession::attach_existing(&setup, &owned.session)?;
        if !namespace::session_socket_is_owned(config_home, session.client.socket())? {
            return Err(Error::NamespaceEscape {
                path: Box::from(session.client.socket()),
                root: Box::from(config_home),
            });
        }
        let id = self.fresh_id();
        let fields = RecordFields {
            id: id.clone(),
            session: Some(owned.session.clone()),
            roster_key: owned.roster_key.clone(),
            argv: owned.argv.clone(),
            size: TermSize { cols: 80, rows: 24 },
            backing: Backing::Herdr,
        };
        self.inner
            .state
            .lock()
            .insert(id.clone(), Runtime::Herdr { fields, session });
        self.bind_session(&id, Some(&owned.session));
        Ok(AdoptedSession {
            id,
            session: owned.session,
            roster_key: owned.roster_key,
            argv: owned.argv,
        })
    }
}
