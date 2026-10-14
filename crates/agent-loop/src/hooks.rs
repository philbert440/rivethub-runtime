use std::path::PathBuf;

use thiserror::Error;
use wire::hooks::{HookContext, HookContextKind, HookPipelineError, HookPipelineResult};

use crate::audit::AuditLog;
use crate::fence::WorkspaceFence;
use crate::shell::ShellDanger;

#[derive(Debug, Error)]
pub enum HookError {
    #[error("invalid hook arguments: {0}")]
    Arguments(#[from] serde_json::Error),
    #[error("invalid danger pattern: {0}")]
    Pattern(#[from] regex::Error),
    #[error("shell danger policy cannot be empty")]
    EmptyDangerPolicy,
    #[error("dangerous shell command refused")]
    DangerousShell,
    #[error("path outside workspace: {}", .0.display())]
    OutsideWorkspace(PathBuf),
    #[error("workspace root is not a directory: {}", .0.display())]
    NotDirectory(PathBuf),
    #[error("hook IO failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("hook task failed: {0}")]
    Task(#[from] tokio::task::JoinError),
    #[error("audit writer unavailable")]
    AuditClosed,
}

#[derive(Debug)]
pub struct HookPipeline {
    shell: ShellDanger,
    fence: WorkspaceFence,
    audit: AuditLog,
}

impl HookPipeline {
    pub const fn new(shell: ShellDanger, fence: WorkspaceFence, audit: AuditLog) -> Self {
        Self {
            shell,
            fence,
            audit,
        }
    }

    pub async fn run(&self, context: HookContext) -> Result<HookPipelineResult, HookError> {
        let mut result = HookPipelineResult {
            context,
            aborted: false,
            skipped: false,
            errors: Vec::new(),
            ran: Vec::new(),
        };
        if let HookContextKind::ToolBefore {
            tool_name, args, ..
        } = &result.context.kind
        {
            result.ran.push("shell-danger".to_owned());
            let checked = self.shell.check(tool_name, args);
            if let Err(error) = checked {
                refuse(&mut result, "shell-danger", &error);
            } else {
                result.ran.push("workspace-fence".to_owned());
                if let Err(error) = self.fence.check(tool_name, args).await {
                    refuse(&mut result, "workspace-fence", &error);
                }
            }
        }
        result.skipped = matches!(
            &result.context.kind,
            HookContextKind::ProviderBefore {
                skip: Some(true),
                ..
            } | HookContextKind::TurnBefore {
                skip: Some(true),
                ..
            } | HookContextKind::SkillBefore {
                skip: Some(true),
                ..
            }
        );
        result.aborted |= matches!(
            &result.context.kind,
            HookContextKind::ToolBefore {
                blocked: Some(true),
                ..
            } | HookContextKind::DelegationBefore {
                blocked: Some(true),
                ..
            }
        );
        result.ran.push("audit-log".to_owned());
        self.audit.record(&result).await?;
        Ok(result)
    }
}

fn refuse(result: &mut HookPipelineResult, hook: &str, error: &HookError) {
    result.aborted = true;
    result.errors.push(HookPipelineError {
        hook_id: hook.to_owned(),
        error: error.to_string(),
    });
    if let HookContextKind::ToolBefore {
        blocked,
        block_reason,
        ..
    } = &mut result.context.kind
    {
        *blocked = Some(true);
        *block_reason = Some(error.to_string());
    }
}
