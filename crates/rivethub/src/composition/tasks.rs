use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use config::{Config, Environment};
use gateway::Gateway;
use gateway::tasks::LocalAgent;
use gateway::tasks::Tasks;
use kernel::queue::JobQueue;
use kernel::tasks::TaskStore;
use kernel::tasks::TaskWatch;
use memory_postgres::PostgresTasks;
use tokio::task::JoinHandle;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use super::ComposeError;

#[derive(Clone)]
pub(super) struct TaskPorts {
    store: Arc<dyn TaskStore>,
    jobs: Arc<dyn JobQueue>,
    watch: Arc<dyn TaskWatch>,
}

impl TaskPorts {
    pub(super) fn of<T>(tasks: &T) -> Self
    where
        T: TaskStore + JobQueue + TaskWatch + Clone + 'static,
    {
        Self {
            store: Arc::new(tasks.clone()),
            jobs: Arc::new(tasks.clone()),
            watch: Arc::new(tasks.clone()),
        }
    }
}

pub(super) async fn postgres(
    url: &str,
    shutdown: &CancellationToken,
) -> Result<(TaskPorts, Watching), ComposeError> {
    let tasks = PostgresTasks::connect_lazy(url)?;
    let watching = tasks.spawn_watch(shutdown.child_token()).await?;
    Ok((TaskPorts::of(&tasks), watching))
}

pub(super) type Watching = JoinHandle<Result<(), kernel::tasks::TaskError>>;

pub(super) async fn attach(
    gateway: Gateway,
    ports: TaskPorts,
    config: &Config,
    env: &Environment,
    files_root: Option<PathBuf>,
) -> Result<Gateway, ComposeError> {
    if config
        .tasks
        .as_ref()
        .is_some_and(|tasks| tasks.enabled == Some(false))
    {
        return Ok(gateway);
    }
    let node = gateway.settings().node.clone();
    let root = files_root.or_else(|| {
        config
            .runtime
            .as_ref()
            .and_then(|runtime| runtime.workspace.clone())
    });
    let mut tasks = Tasks::new(ports.store, ports.jobs, ports.watch, node)
        .with_agents(local_agents(config))
        .with_files_root(root);
    if config
        .mesh
        .as_ref()
        .is_some_and(|mesh| mesh.enabled == Some(true))
    {
        let registry =
            gateway::mesh::Registry::open(config::shared_dir(env), PathBuf::from("mesh.json"))
                .await?;
        tasks = tasks.with_mesh(registry);
    }
    Ok(gateway.with_tasks(Arc::new(tasks)))
}

pub(super) fn serve(
    gateway: &Arc<Gateway>,
    shutdown: &CancellationToken,
    background: &mut JoinSet<Result<(), ComposeError>>,
) {
    let gateway = Arc::clone(gateway);
    let token = shutdown.child_token();
    background.spawn(async move {
        gateway::tasks::serve(gateway, token).await;
        Ok(())
    });
}

fn local_agents(config: &Config) -> BTreeMap<String, LocalAgent> {
    config
        .agents
        .iter()
        .flatten()
        .map(|(name, agent)| {
            (
                name.clone(),
                LocalAgent {
                    provider: agent.provider.clone(),
                    model: agent.model.clone(),
                },
            )
        })
        .collect()
}
