use chrono::DateTime;
use chrono::TimeZone;
use chrono::Utc;
use kernel::topics::Alias;
use kernel::topics::Change;
use kernel::topics::FirstLeaf;
use kernel::topics::Lineage;
use kernel::topics::MAX_DEPTH;
use kernel::topics::MergePlan;
use kernel::topics::NewTopic;
use kernel::topics::Placement;
use kernel::topics::PlacementOutcome;
use kernel::topics::PlacementSource;
use kernel::topics::Redirect;
use kernel::topics::Root;
use kernel::topics::RootKind;
use kernel::topics::Slug;
use kernel::topics::Topic;
use kernel::topics::TopicChanges;
use kernel::topics::TopicError;
use kernel::topics::TopicId;
use kernel::topics::TopicKey;
use kernel::topics::TopicListing;
use kernel::topics::TopicName;
use kernel::topics::TopicQuery;
use kernel::topics::TopicTree;
use kernel::topics::Verdict;
use kernel::topics::check_parent;
use kernel::topics::rank;
use std::str::FromStr;

use wire::session_id::SessionId;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn at(minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 1, 12, minute, 0)
        .single()
        .unwrap_or_default()
}

fn topic(name: &str, aliases: &[&str]) -> Result<Topic, TopicError> {
    Topic::create(
        TopicId::random(),
        NewTopic {
            name: TopicName::new(name)?,
            aliases: aliases
                .iter()
                .map(|alias| Alias::new(alias))
                .collect::<Result<_, _>>()?,
            parent: None,
            directory: None,
        },
        at(0),
    )
}

fn placement(
    by: PlacementSource,
    topic: TopicId,
    minute: u32,
) -> Result<Placement, Box<dyn std::error::Error>> {
    Ok(Placement {
        session: SessionId::from_str("claude-code:00000000-0000-4000-8000-000000000001")?,
        topic,
        by,
        at: at(minute),
    })
}

#[test]
fn slugs_fold_names_to_lowercase_dashed_text() -> TestResult {
    assert_eq!(Slug::fold("RivetHub Desktop")?.as_str(), "rivethub-desktop");
    assert_eq!(
        Slug::fold("  --Rust / Runtime!!  ")?.as_str(),
        "rust-runtime"
    );
    assert_eq!(Slug::fold("Café 2")?.as_str(), "café-2");
    assert!(matches!(
        Slug::fold("!!!"),
        Err(TopicError::EmptySlug { .. })
    ));
    Ok(())
}

#[test]
fn names_and_aliases_are_trimmed_and_never_empty() -> TestResult {
    assert_eq!(TopicName::new("  Deploy ")?.as_str(), "Deploy");
    assert!(matches!(TopicName::new("   "), Err(TopicError::EmptyName)));
    assert!(matches!(Alias::new(""), Err(TopicError::EmptyAlias)));
    let created = topic("Deploy", &["Ship", "ship", "CI"])?;
    let aliases: Vec<&str> = created.aliases.iter().map(Alias::as_str).collect();
    assert_eq!(aliases, ["CI", "Ship"]);
    assert_eq!(created.slug.as_str(), "deploy");
    Ok(())
}

#[test]
fn renaming_refolds_the_slug_and_changes_clear_or_keep_fields() -> TestResult {
    let created = topic("Deploy", &[])?;
    let parent = TopicId::random();
    let directory = Root::Local {
        node: "node-a".into(),
        path: "/home/user/acmekit".into(),
    };
    let renamed = created.clone().apply(
        TopicChanges {
            name: Some(TopicName::new("Release Train")?),
            aliases: None,
            parent: Change::Set(parent),
            directory: Change::Set(directory.clone()),
        },
        at(5),
    )?;
    assert_eq!(renamed.slug.as_str(), "release-train");
    assert_eq!(renamed.parent, Some(parent));
    assert_eq!(renamed.directory, Some(directory));
    assert_eq!(renamed.updated_at, at(5));
    assert_eq!(renamed.created_at, created.created_at);
    let cleared = renamed.apply(
        TopicChanges {
            parent: Change::Clear,
            ..TopicChanges::default()
        },
        at(6),
    )?;
    assert_eq!(cleared.parent, None);
    assert!(cleared.directory.is_some());
    Ok(())
}

#[test]
fn roots_round_trip_through_their_stored_parts() {
    let local = Root::Local {
        node: "node-a".into(),
        path: "/home/user/acmekit".into(),
    };
    assert_eq!(
        Root::from_parts(
            local.kind(),
            local.node().map(str::to_owned),
            local.path().into()
        ),
        Some(local)
    );
    assert_eq!(RootKind::Shared.to_string(), "shared");
    assert_eq!(Root::from_parts(RootKind::Local, None, "/srv".into()), None);
    assert_eq!(
        Root::from_parts(RootKind::Shared, Some("node-a".into()), "/srv".into()),
        None
    );
}

#[test]
fn the_categorizer_never_overrides_your_placement() -> TestResult {
    let mine = TopicId::random();
    let suggested = TopicId::random();
    let yours = placement(PlacementSource::You, mine, 1)?;
    let outcome = placement(PlacementSource::Categorizer, suggested, 2)?.admit(Some(yours.clone()));
    assert_eq!(outcome, PlacementOutcome::Kept(yours));
    Ok(())
}

#[test]
fn placements_replace_one_another_otherwise() -> TestResult {
    let first = TopicId::random();
    let second = TopicId::random();
    let fresh = placement(PlacementSource::Categorizer, first, 1)?;
    assert_eq!(
        fresh.clone().admit(None),
        PlacementOutcome::Placed(fresh.clone())
    );
    let recategorized = placement(PlacementSource::Categorizer, second, 2)?;
    assert_eq!(
        recategorized.clone().admit(Some(fresh)),
        PlacementOutcome::Placed(recategorized)
    );
    let moved = placement(PlacementSource::You, second, 3)?;
    let over_yours = placement(PlacementSource::You, first, 4)?;
    assert_eq!(
        over_yours.clone().admit(Some(moved)),
        PlacementOutcome::Placed(over_yours.clone())
    );
    assert_eq!(
        PlacementOutcome::Placed(over_yours.clone()).into_placement(),
        over_yours
    );
    assert_eq!(PlacementSource::Categorizer.to_string(), "categorizer");
    assert_eq!("you".parse::<PlacementSource>()?, PlacementSource::You);
    Ok(())
}

fn refusal(result: &Result<(), TopicError>) -> &'static str {
    if result.is_ok() {
        "ok"
    } else if matches!(result, Err(TopicError::TooDeep { max: MAX_DEPTH })) {
        "too deep"
    } else if matches!(result, Err(TopicError::IntoOwnSubtree)) {
        "own subtree"
    } else if matches!(result, Err(TopicError::OwnParent)) {
        "own parent"
    } else {
        "other"
    }
}

fn under(name: &str, parent: Option<&Topic>) -> Result<Topic, Box<dyn std::error::Error>> {
    let mut made = topic(name, &[])?;
    made.parent = parent.map(|parent| parent.id);
    Ok(made)
}

#[test]
fn topics_nest_four_levels_deep_and_never_under_themselves() -> TestResult {
    let runtime = under("Runtime", None)?;
    let topics = under("Topics", Some(&runtime))?;
    let merge = under("Merge", Some(&topics))?;
    let desktop = under("Desktop", None)?;
    let memory = under("Memory", Some(&desktop))?;
    let tree = TopicTree::of([&runtime, &topics, &merge, &desktop, &memory]);
    for (child, parent, want) in [
        (TopicId::random(), &merge, "ok"),
        (desktop.id, &topics, "ok"),
        (desktop.id, &merge, "too deep"),
        (runtime.id, &merge, "own subtree"),
        (runtime.id, &runtime, "own parent"),
    ] {
        assert_eq!(
            refusal(&check_parent(child, parent, &tree)),
            want,
            "under {}",
            parent.name.as_str()
        );
    }
    assert_eq!((tree.depth(merge.id), tree.height(runtime.id)), (3, 3));
    assert_eq!(tree.root(merge.id), runtime.id);
    Ok(())
}

#[test]
fn merging_redirects_the_slug_and_moves_aliases_that_do_not_clash() -> TestResult {
    let source = topic("Desktop App", &["electron", "Tauri"])?;
    let into = topic("RivetHub Desktop", &["tauri"])?;
    let tree = TopicTree::of([&source, &into]);
    let plan = MergePlan::new(&source, &into, &tree)?;
    assert_eq!(
        plan.redirect,
        Redirect {
            slug: Slug::fold("desktop-app")?,
            topic: into.id,
        }
    );
    let moved: Vec<&str> = plan.aliases.iter().map(Alias::as_str).collect();
    assert_eq!(moved, ["electron"]);
    let merged = plan.merged(into, at(9));
    let aliases: Vec<&str> = merged.aliases.iter().map(Alias::as_str).collect();
    assert_eq!(aliases, ["electron", "tauri"]);
    assert_eq!(merged.updated_at, at(9));
    assert!(matches!(
        MergePlan::new(&source, &source, &tree),
        Err(TopicError::SelfMerge)
    ));
    Ok(())
}

#[test]
fn merging_a_parent_into_its_child_lifts_the_child_and_adopts_its_siblings() -> TestResult {
    let parent = under("Runtime", None)?;
    let child = under("Topics", Some(&parent))?;
    let sibling = under("Gateway", Some(&parent))?;
    let plan = MergePlan::new(&parent, &child, &TopicTree::of([&parent, &child, &sibling]))?;
    assert_eq!(plan.into_parent, None);
    assert_eq!(plan.subtopics, [sibling.id]);
    Ok(())
}

#[test]
fn merging_keeps_to_four_levels_and_never_into_a_deeper_descendant() -> TestResult {
    let runtime = under("Runtime", None)?;
    let topics = under("Topics", Some(&runtime))?;
    let merge = under("Merge", Some(&topics))?;
    let shallow = under("Shallow", None)?;
    let leaf = under("Leaf", Some(&shallow))?;
    let deep = under("Deep", None)?;
    let middle = under("Middle", Some(&deep))?;
    let bottom = under("Bottom", Some(&middle))?;
    let tree = TopicTree::of([
        &runtime, &topics, &merge, &shallow, &leaf, &deep, &middle, &bottom,
    ]);
    assert!(MergePlan::new(&shallow, &merge, &tree).is_ok());
    assert!(matches!(
        MergePlan::new(&deep, &merge, &tree),
        Err(TopicError::TooDeep { max: MAX_DEPTH })
    ));
    assert!(matches!(
        MergePlan::new(&runtime, &merge, &tree),
        Err(TopicError::IntoOwnSubtree)
    ));
    Ok(())
}

#[test]
fn keys_are_ids_or_folded_slugs() -> TestResult {
    let id = TopicId::random();
    assert_eq!(TopicKey::parse(&id.to_string())?, TopicKey::Id(id));
    assert_eq!(
        TopicKey::parse("RivetHub Desktop")?,
        TopicKey::Slug(Slug::fold("rivethub-desktop")?)
    );
    assert!(TopicKey::parse("--").is_err());
    Ok(())
}

fn listed(topic: Topic, last_active: Option<u32>) -> TopicListing {
    TopicListing {
        topic,
        session_count: 0,
        last_active: last_active.map(at),
    }
}

#[test]
fn search_matches_prefixes_folded_slugs_aliases_and_redirects_by_recent_activity() -> TestResult {
    let desktop = topic("RivetHub Desktop", &["Tauri"])?;
    let runtime = topic("Rust Runtime", &["rh"])?;
    let deploy = topic("Deploy", &[])?;
    let redirects = vec![Redirect {
        slug: Slug::fold("desktop-app")?,
        topic: desktop.id,
    }];
    let listings = vec![
        listed(desktop, Some(1)),
        listed(runtime, Some(5)),
        listed(deploy, None),
    ];
    let names = |query: &str| -> Vec<String> {
        rank(listings.clone(), &redirects, &TopicQuery::new(query), 10)
            .into_iter()
            .map(|listing| listing.topic.name.as_str().to_owned())
            .collect()
    };
    assert_eq!(names("riv"), ["RivetHub Desktop"]);
    assert_eq!(names("desk"), ["RivetHub Desktop"]);
    assert_eq!(names("runtime"), ["Rust Runtime"]);
    assert_eq!(names("RH"), ["Rust Runtime"]);
    assert_eq!(names("de"), ["RivetHub Desktop", "Deploy"]);
    assert_eq!(names("tau"), ["RivetHub Desktop"]);
    assert_eq!(names("Desktop App"), ["RivetHub Desktop"]);
    assert_eq!(names(""), ["Rust Runtime", "RivetHub Desktop", "Deploy"]);
    assert_eq!(names("zzz"), Vec::<String>::new());
    assert_eq!(
        rank(listings.clone(), &redirects, &TopicQuery::new(""), 1).len(),
        1
    );
    Ok(())
}

fn catalog(topics: &[&Topic]) -> Vec<TopicListing> {
    topics
        .iter()
        .map(|topic| TopicListing {
            topic: (*topic).clone(),
            session_count: 0,
            last_active: None,
        })
        .collect()
}

fn first_leaf<'a>(title: Option<&'a str>, summary: &'a str) -> FirstLeaf<'a> {
    FirstLeaf {
        placement: None,
        lineage: Lineage::Root,
        title,
        summary,
    }
}

#[test]
fn the_categorizer_leaves_placed_and_child_sessions_alone() -> TestResult {
    let desktop = topic("RivetHub Desktop", &[])?;
    let topics = catalog(&[&desktop]);
    let summary = "Fixed the RivetHub desktop window.";
    let yours = placement(PlacementSource::You, desktop.id, 1)?;
    let placed = FirstLeaf {
        placement: Some(&yours),
        ..first_leaf(None, summary)
    };
    assert_eq!(placed.categorize(&topics, &[])?, Verdict::AlreadyPlaced);
    let child = FirstLeaf {
        lineage: Lineage::Child,
        ..first_leaf(None, summary)
    };
    assert_eq!(child.categorize(&topics, &[])?, Verdict::InheritsParent);
    Ok(())
}

#[test]
fn one_named_topic_is_a_clear_match() -> TestResult {
    let runtime = topic("RivetHub", &[])?;
    let mut desktop = topic("RivetHub Desktop", &["Tauri"])?;
    desktop.parent = Some(runtime.id);
    let gateway = topic("Gateway", &[])?;
    let renamed = Redirect {
        slug: Slug::fold("Mesh Gateway")?,
        topic: gateway.id,
    };
    let topics = catalog(&[&runtime, &desktop, &gateway]);
    let by_alias = first_leaf(None, "Moved the tauri shell to the new window API.");
    assert_eq!(
        by_alias.categorize(&topics, &[])?,
        Verdict::Place(desktop.id)
    );
    let sub_topic = first_leaf(Some("RivetHub desktop menus"), "Reworked the menus.");
    assert_eq!(
        sub_topic.categorize(&topics, &[])?,
        Verdict::Place(desktop.id)
    );
    let unrelated = catalog(&[&runtime, &desktop]);
    let by_redirect = first_leaf(None, "Retried the mesh gateway handshake.");
    assert_eq!(
        by_redirect.categorize(&unrelated, &[])?,
        Verdict::Suggest {
            name: TopicName::new("Retried the mesh gateway handshake")?,
            topic: None,
        }
    );
    let redirected = Redirect {
        topic: desktop.id,
        ..renamed
    };
    assert_eq!(
        by_redirect.categorize(&unrelated, &[redirected])?,
        Verdict::Place(desktop.id)
    );
    Ok(())
}

#[test]
fn unclear_sessions_get_a_suggestion_from_their_title() -> TestResult {
    let desktop = topic("RivetHub Desktop", &[])?;
    let gateway = topic("Gateway", &[])?;
    let short = topic("UI", &[])?;
    let topics = catalog(&[&desktop, &gateway, &short]);
    let both = first_leaf(
        None,
        "Wired the gateway into RivetHub Desktop. The gateway now retries.",
    );
    assert_eq!(
        both.categorize(&topics, &[])?,
        Verdict::Suggest {
            name: TopicName::new("Wired the gateway into RivetHub Desktop")?,
            topic: None,
        }
    );
    let titled = first_leaf(
        Some("Gateway retries"),
        "Tuned the RivetHub desktop and the gateway backoff.",
    );
    assert_eq!(
        titled.categorize(&topics, &[])?,
        Verdict::Suggest {
            name: TopicName::new("Gateway retries")?,
            topic: Some(gateway.id),
        }
    );
    let unknown = first_leaf(None, "Cleaned up the ui and gatewayish code.");
    assert_eq!(
        unknown.categorize(&topics, &[])?,
        Verdict::Suggest {
            name: TopicName::new("Cleaned up the ui and gatewayish code")?,
            topic: None,
        }
    );
    assert_eq!(
        first_leaf(None, "  ").categorize(&topics, &[])?,
        Verdict::Untitled
    );
    Ok(())
}
