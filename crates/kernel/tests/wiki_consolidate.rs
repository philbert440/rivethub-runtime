use kernel::wiki::consolidate::{ConsolidateWiki, WikiFold, consolidation_plan, is_slug_variant};

fn slugs(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_owned()).collect()
}

fn fold(cluster: &str, parent: &str, children: &[&str]) -> WikiFold {
    WikiFold {
        cluster: cluster.to_owned(),
        parent: parent.to_owned(),
        children: slugs(children),
    }
}

#[test]
fn session_variants_fold_into_the_stem_page() -> Result<(), serde_json::Error> {
    let plan = consolidation_plan(
        &slugs(&[
            "acmekit-deploys",
            "acmekit-deploys-friday",
            "acmekit-deploys-friday-rollback",
            "staging-database",
        ]),
        &defaults()?,
    );
    assert_eq!(
        plan,
        vec![fold(
            "acmekit-deploys",
            "acmekit-deploys",
            &["acmekit-deploys-friday", "acmekit-deploys-friday-rollback"],
        )]
    );
    Ok(())
}

#[test]
fn siblings_sharing_a_stem_without_a_parent_page_stay_apart() -> Result<(), serde_json::Error> {
    let plan = consolidation_plan(
        &slugs(&["billing-service-refunds", "billing-service-fraud"]),
        &defaults()?,
    );
    assert_eq!(plan, Vec::<WikiFold>::new());
    Ok(())
}

#[test]
fn the_shortest_variant_parents_a_cluster_without_its_stem_page() -> Result<(), serde_json::Error> {
    let plan = consolidation_plan(
        &slugs(&[
            "billing-service-refunds-march",
            "billing-service-refunds",
            "billing-service-fraud",
        ]),
        &defaults()?,
    );
    assert_eq!(plan, Vec::<WikiFold>::new());
    let plan = consolidation_plan(
        &slugs(&["billing-service-refunds-march", "billing-service-refunds"]),
        &defaults()?,
    );
    assert_eq!(
        plan,
        vec![fold(
            "billing-service",
            "billing-service-refunds",
            &["billing-service-refunds-march"],
        )]
    );
    Ok(())
}

#[test]
fn larger_clusters_come_first_and_the_limit_caps_them() -> Result<(), serde_json::Error> {
    let corpus = slugs(&[
        "acmekit-deploys",
        "acmekit-deploys-friday",
        "staging-database",
        "staging-database-restore",
        "staging-database-snapshot",
    ]);
    let all = consolidation_plan(&corpus, &defaults()?);
    assert_eq!(
        all,
        vec![
            fold(
                "staging-database",
                "staging-database",
                &["staging-database-restore", "staging-database-snapshot"],
            ),
            fold(
                "acmekit-deploys",
                "acmekit-deploys",
                &["acmekit-deploys-friday"]
            ),
        ]
    );
    let limited = consolidation_plan(
        &corpus,
        &ConsolidateWiki {
            limit_clusters: 1,
            ..defaults()?
        },
    );
    assert_eq!(limited, all.iter().take(1).cloned().collect::<Vec<_>>());
    let strict = consolidation_plan(
        &corpus,
        &ConsolidateWiki {
            min_cluster_size: 3,
            ..defaults()?
        },
    );
    assert_eq!(strict, all.iter().take(1).cloned().collect::<Vec<_>>());
    Ok(())
}

#[test]
fn slugs_that_are_not_normalized_are_left_alone() -> Result<(), serde_json::Error> {
    let plan = consolidation_plan(
        &slugs(&[
            "acmekit-deploys",
            "acmekit-deploys-Friday",
            "acmekit-deploys--x",
        ]),
        &defaults()?,
    );
    assert_eq!(plan, Vec::<WikiFold>::new());
    Ok(())
}

#[test]
fn a_variant_is_a_hyphen_boundary_prefix() {
    assert!(is_slug_variant("acmekit", "acmekit-deploys"));
    assert!(is_slug_variant("acmekit-deploys", "acmekit"));
    assert!(is_slug_variant("acmekit", "acmekit"));
    assert!(!is_slug_variant("acme", "acmekit"));
    assert!(!is_slug_variant("", ""));
}

#[test]
fn the_job_payload_takes_the_documented_defaults() -> Result<(), serde_json::Error> {
    let empty: ConsolidateWiki = serde_json::from_str("{}")?;
    assert_eq!(empty, defaults()?);
    assert_eq!(empty.limit_clusters, 50);
    assert_eq!(empty.min_cluster_size, 2);
    assert!(!empty.dry_run);
    let set: ConsolidateWiki =
        serde_json::from_str(r#"{"dryRun":true,"limitClusters":3,"minClusterSize":4}"#)?;
    assert_eq!(
        set,
        ConsolidateWiki {
            dry_run: true,
            limit_clusters: 3,
            min_cluster_size: 4,
        }
    );
    assert!(serde_json::from_str::<ConsolidateWiki>(r#"{"limitClusters":-1}"#).is_err());
    Ok(())
}

fn defaults() -> Result<ConsolidateWiki, serde_json::Error> {
    serde_json::from_str("{}")
}
