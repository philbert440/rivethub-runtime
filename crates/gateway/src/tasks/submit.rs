use chrono::Utc;
use kernel::queue::Job;
use kernel::queue::JobKeyMode;
use kernel::queue::JobName;
use kernel::tasks::Budget;
use kernel::tasks::Criterion;
use kernel::tasks::DepthError;
use kernel::tasks::Finish;
use kernel::tasks::Parent;
use kernel::tasks::ParentSource;
use kernel::tasks::Target;
use kernel::tasks::Task;
use kernel::tasks::TaskError;
use kernel::tasks::TaskJobPayload;
use kernel::tasks::TaskStatus;
use uuid::Uuid;
use wire::JsonObject;

use super::Refusal;
use super::Tasks;
use crate::Gateway;
use crate::tenancy::UserId;

#[derive(Debug)]
pub(crate) struct Submission {
    pub(crate) goal: String,
    pub(crate) agent: String,
    pub(crate) pin: Option<String>,
    pub(crate) criteria: Vec<Criterion>,
    pub(crate) budget: Budget,
    pub(crate) parent: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) effort: Option<String>,
    pub(crate) cwd: Option<String>,
    pub(crate) requested_by: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum SubmitError {
    #[error(transparent)]
    Refused(#[from] Refusal),
    #[error(transparent)]
    Depth(#[from] DepthError),
    #[error("{0}")]
    Unavailable(String),
    #[error(transparent)]
    Store(#[from] TaskError),
}

impl Tasks {
    pub(crate) async fn submit(
        &self,
        gateway: &Gateway,
        user: &UserId,
        submission: Submission,
    ) -> Result<Task, SubmitError> {
        let resolved = self
            .sources(gateway)
            .await?
            .resolve(&submission.agent, submission.pin.as_deref())?;
        let (parent, inherited) = self.parent(gateway, submission.parent).await?;
        let task = Task {
            id: Uuid::now_v7(),
            goal: submission.goal,
            criteria: submission.criteria,
            budget: submission.budget,
            status: TaskStatus::Queued,
            target: Target {
                agent: resolved.agent,
                preset: resolved.preset,
                node: resolved.node,
                harness: resolved.harness.map(super::harness_in),
                model: submission.model.or(resolved.model),
                effort: submission.effort.or(resolved.effort),
                cwd: submission.cwd,
                permission_mode: inherited,
            },
            parent,
            child: None,
            result: None,
            error: None,
            usage: None,
            requested_by: submission.requested_by,
            owner: user.as_str().to_owned(),
            created_at: Utc::now(),
            started_at: None,
            completed_at: None,
            alive_at: None,
        };
        self.store.insert(&task).await?;
        self.waits.created(task.id);
        if let Err(error) = self.jobs.enqueue(job(&task)?).await {
            self.waits.forget(task.id);
            let error = TaskError::from(error);
            self.store
                .finish(task.id, &Finish::failed(error.to_string(), Utc::now()))
                .await?;
            return Err(error.into());
        }
        Ok(task)
    }

    async fn parent(
        &self,
        gateway: &Gateway,
        given: Option<String>,
    ) -> Result<(Parent, Option<String>), SubmitError> {
        let Some(given) = given.filter(|session| !session.is_empty()) else {
            return Ok((Parent::default(), None));
        };
        let terminal = gateway.terminal();
        let session = terminal.map_or_else(|| given.clone(), |terminal| terminal.canonical(&given));
        let source = match gateway.sessions().owner(&session) {
            Ok(Some(_)) => ParentSource::Launch,
            Ok(None) => ParentSource::Request,
            Err(error) => {
                tracing::debug!(%error, "the parent session is not one this node knows");
                ParentSource::Request
            }
        };
        let above = self.store.by_child(&session).await?;
        let parent = Parent::of(Some(session.clone()), source, above.as_ref())?;
        let mode = terminal
            .and_then(|terminal| terminal.permission_mode(&session))
            .or_else(|| above.and_then(|task| task.target.permission_mode));
        Ok((parent, mode))
    }
}

fn job(task: &Task) -> Result<Job, TaskError> {
    Ok(Job {
        name: JobName::RunTaskOn {
            node: task.target.node.clone(),
        },
        payload: JsonObject::from_serialize(&TaskJobPayload {
            task_id: task.id.to_string(),
        })
        .map_err(|source| TaskError::Payload(Box::new(source)))?,
        key: Some(format!("task:{}", task.id)),
        key_mode: JobKeyMode::Replace,
        priority: 0,
        run_at: None,
        max_attempts: Some(1),
    })
}
