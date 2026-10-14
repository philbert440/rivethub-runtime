pub(crate) fn assert_deprecated_aliases(env: &config::Environment) {
    for (current, legacy) in config::deprecated_alias_names() {
        let legacy_pinned = env.text(legacy).is_some();
        let current_set = env
            .text(current)
            .is_some_and(|value| !value.trim().is_empty());
        let notice = env.legacy_notice(current, legacy);
        if legacy_pinned && !current_set {
            assert!(
                notice
                    .as_ref()
                    .is_some_and(|item| { item.contains(current) && item.contains(legacy) }),
                "deprecated alias {legacy} must warn and name {current}: {notice:?}"
            );
            assert_eq!(
                env.legacy_notices()
                    .iter()
                    .filter(|item| item.contains(legacy))
                    .count(),
                1,
                "one warning for {legacy}"
            );
        } else {
            assert!(
                notice.is_none(),
                "no deprecation warning expected for {legacy}: {notice:?}"
            );
        }
    }
}
