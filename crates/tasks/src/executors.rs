use wire::task::ExecutorInteractionCapabilities;
use wire::task::ExecutorResultCapabilities;
use wire::task::ExecutorTurnCapabilities;
use wire::task::HarnessExecutorCapabilities;
use wire::task::McpInjection;
use wire::task::TaskExecutorKind;

pub(crate) fn offered(mesh: bool) -> Vec<TaskExecutorKind> {
    let mut kinds = vec![TaskExecutorKind::ChatLoop, TaskExecutorKind::HarnessSession];
    if mesh {
        kinds.push(TaskExecutorKind::Mesh);
    }
    kinds
}

pub(crate) fn capabilities(kind: TaskExecutorKind) -> HarnessExecutorCapabilities {
    match kind {
        TaskExecutorKind::ChatLoop | TaskExecutorKind::HarnessSession | TaskExecutorKind::Mesh => {
            session_post()
        }
    }
}

fn session_post() -> HarnessExecutorCapabilities {
    HarnessExecutorCapabilities {
        turn: ExecutorTurnCapabilities {
            steerable: true,
            multi_turn: true,
            structured_stream: false,
        },
        result: ExecutorResultCapabilities {
            usage_in_result: true,
            session_id_capture: false,
        },
        interaction: ExecutorInteractionCapabilities {
            slash_commands: false,
            effort_selection: false,
        },
        mcp_injection: McpInjection::None,
    }
}
