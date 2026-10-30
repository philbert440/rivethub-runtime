use gateway::tenancy::{Tenancy, UserId};
use wire::gateway::users::UsersRegistry;

use super::ComposeError;

#[derive(Debug)]
pub(crate) struct Identity {
    pub(crate) tenancy: Tenancy,
    pub(super) owner: UserId,
}

pub(crate) async fn load(
    config: &config::Config,
    env: &config::Environment,
) -> Result<Identity, ComposeError> {
    let path = env.users_file().map(std::path::Path::new).or_else(|| {
        config
            .gateway
            .as_ref()
            .and_then(|gateway| gateway.users_file.as_deref())
    });
    let Some(path) = path else {
        return registry(UsersRegistry {
            unmapped_is_owner: true,
            ..UsersRegistry::default()
        });
    };
    let users: UsersRegistry = serde_json::from_slice(&tokio::fs::read(path).await?)?;
    registry(users)
}

fn registry(users: UsersRegistry) -> Result<Identity, ComposeError> {
    let owner = UserId::new(users.owner_user_id)?;
    let mut tenancy = Tenancy::local(owner.clone());
    if users.unmapped_is_owner {
        tenancy = tenancy.with_unmapped_device_owner(owner.clone());
    }
    for (id, record) in users.users {
        let user = UserId::new(id)?;
        if let Some(devices) = record.devices {
            for device in devices {
                tenancy = tenancy.with_device(&device, user.clone())?;
            }
        }
    }
    Ok(Identity { tenancy, owner })
}
