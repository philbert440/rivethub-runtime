use std::path::Path;
use std::time::Duration;

use harness_acp::{AcpCommand, AcpHarnessAdapter, AcpLaunch, LaunchTable};
use kernel::harness::{HarnessAdapter, SessionOption, SessionSpec};
use wire::harness::{HarnessId, HarnessModelsSource};
use wire::session_id::SessionId;

fn adapter(cwd: Option<&Path>, listing: bool) -> AcpHarnessAdapter {
    let mut args = vec![
        "--session-id".to_owned(),
        "ses_fake0123456789abcdefgh".to_owned(),
        "--on-load".to_owned(),
        "replay".to_owned(),
    ];
    if listing {
        args.push("--list-sessions".to_owned());
    }
    AcpHarnessAdapter::with_launch_table(LaunchTable::from_entries([(
        HarnessId::Opencode,
        AcpLaunch::Native(AcpCommand {
            program: env!("CARGO_BIN_EXE_fake-acp-agent").into(),
            args,
            working_dir: cwd.map(Path::to_path_buf),
            env: Vec::new(),
        }),
    )]))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn discovery_uses_advertised_labels_current_model_and_session_listing() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let cwd = tempfile::tempdir().expect("cwd");
        for listing in [true, false] {
            let adapter = adapter(Some(cwd.path()), listing);
            let offerings = adapter.offerings().await.expect("discovery");
            let offering = offerings.first().expect("offering");
            assert_eq!(offering.capabilities.observation.list_sessions, listing);
            assert_eq!(
                offering.capabilities.models_source,
                Some(HarnessModelsSource::Discovered)
            );
            let models = offering.capabilities.models.as_ref().expect("models");
            assert_eq!(models.len(), 3);
            let current = models
                .iter()
                .find(|model| model.is_default == Some(true))
                .expect("current model");
            assert_eq!(current.id, "grok-4.7");
            assert_eq!(current.label, "Grok 4.7 (recommended)");
            assert!(models.iter().all(|model| model.label != model.id));
        }
    })
    .await
    .expect("discovery timeout");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_selection_updates_the_advertised_default() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let cwd = tempfile::tempdir().expect("cwd");
        let adapter = adapter(None, true);
        let session = adapter
            .new_session(SessionSpec {
                session: SessionId::new(HarnessId::Opencode, "requested"),
                harness: HarnessId::Opencode,
                model: None,
                effort: None,
                working_dir: Some(cwd.path().to_path_buf()),
                startup_context: None,
            })
            .await
            .expect("new session");
        adapter
            .set_option(&session, SessionOption::Model("grok-5".to_owned()))
            .await
            .expect("select");
        let offerings = adapter.offerings().await.expect("offerings");
        let models = offerings
            .first()
            .expect("offering")
            .capabilities
            .models
            .as_ref()
            .expect("models");
        let selected = models
            .iter()
            .find(|model| model.is_default == Some(true))
            .expect("selected model");
        assert_eq!(selected.id, "grok-5");
        assert_eq!(selected.label, "Grok 5");
        assert_eq!(
            models
                .iter()
                .filter(|model| model.is_default == Some(true))
                .count(),
            1
        );
        adapter.release(&session).await.expect("release");
    })
    .await
    .expect("session timeout");
}
