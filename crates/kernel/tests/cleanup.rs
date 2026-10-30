use chrono::Utc;
use kernel::cleanup::DraftTopic;
use kernel::cleanup::JunkReason;
use kernel::cleanup::NameGuard;
use kernel::cleanup::Project;
use kernel::cleanup::ProposedTree;
use kernel::cleanup::Snapshot;
use kernel::cleanup::SnapshotTopic;
use kernel::topics::TopicId;
use kernel::topics::TopicKind;

fn draft(name: &str, parent: Option<&str>, estimate: u32, merges: &[&str]) -> DraftTopic {
    DraftTopic {
        name: name.to_owned(),
        parent: parent.map(str::to_owned),
        about: None,
        estimate,
        merges: merges.iter().map(|merge| (*merge).to_owned()).collect(),
    }
}

#[test]
fn seeds_projects_and_guards_the_proposed_names() {
    let now = Utc::now();
    let existing = SnapshotTopic {
        id: TopicId::random(),
        kind: TopicKind::Work,
        name: "AcmeKit Rewrite".to_owned(),
        parent: None,
        aliases: Vec::new(),
        directory: None,
        created_at: now,
        updated_at: now,
    };
    let snapshot = Snapshot {
        taken_at: now,
        conversations: Vec::new(),
        topics: vec![existing.clone()],
        redirects: Vec::new(),
        placements: Vec::new(),
        corrections: Vec::new(),
        projects: vec![
            Project {
                name: "AcmeKit".to_owned(),
                aliases: vec!["acmekit-main".to_owned()],
            },
            Project {
                name: "General".to_owned(),
                aliases: Vec::new(),
            },
        ],
        refused: Vec::new(),
        pages: Vec::new(),
        sections: Vec::new(),
    };
    let guard = NameGuard::new(["Fix the nightly build"], []);
    let drafts = vec![
        draft("Release Pipeline", Some("AcmeKit"), 9, &[]),
        draft("Release pipelines", Some("AcmeKit"), 2, &[]),
        draft("Fix the nightly build", Some("AcmeKit"), 5, &[]),
        draft("Nightly Signing", Some("Release Pipeline"), 4, &[]),
        draft("Docs Site", None, 1, &[]),
        draft("acmekit-main", None, 6, &["AcmeKit Rewrite"]),
    ];
    let tree = ProposedTree::propose(&snapshot, drafts, &guard).unwrap();
    let paths: Vec<String> = tree
        .topics
        .iter()
        .map(|topic| tree.path(topic.id))
        .collect();
    assert_eq!(
        paths,
        [
            "AcmeKit",
            "AcmeKit › Release Pipeline",
            "AcmeKit › Release Pipeline › Nightly Signing"
        ]
    );
    assert_eq!(tree.merges.len(), 1);
    assert_eq!(
        tree.resolve(existing.id),
        tree.topics.first().map(|topic| topic.id)
    );
    let reasons: Vec<(&str, &JunkReason)> = tree
        .junk
        .iter()
        .map(|junk| (junk.name.as_str(), &junk.reason))
        .collect();
    assert_eq!(
        reasons,
        [
            ("General", &JunkReason::Generic),
            (
                "Release pipelines",
                &JunkReason::Duplicate {
                    of: "AcmeKit › Release Pipeline".to_owned()
                }
            ),
            ("Fix the nightly build", &JunkReason::Title),
            ("Docs Site", &JunkReason::TooFew { sessions: 1 }),
        ]
    );
}
