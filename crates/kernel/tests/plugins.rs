use std::time::Duration;

use kernel::plugins::{
    ApprovalError, PluginAccess, access, approve, check_manifest, narrowed, pending, restart_delay,
    split_tool_name, tool_name,
};
use wire::plugin::manifest::{Permission, PluginManifest};

const RUN: &str = r#""run": {"command": ["files"]}"#;

fn manifest(body: &str) -> Result<PluginManifest, serde_json::Error> {
    let text = format!(
        r#"{{"manifest": 1, "id": "files", "name": "Files", "version": "0.1.0",
            "description": "Files.", "permissions": ["sessions.read"], {body}}}"#
    );
    serde_json::from_str(&text)
}

#[test]
fn routes_need_the_permission_their_family_names() {
    assert_eq!(access(true, "/api/plugin/self"), PluginAccess::Own);
    assert_eq!(
        access(false, "/api/plugin/views/changed"),
        PluginAccess::Own
    );
    assert_eq!(
        access(false, "/api/plugin/notify"),
        PluginAccess::Needs(Permission::NotificationsSend)
    );
    assert_eq!(
        access(true, "/api/memory/search"),
        PluginAccess::Needs(Permission::MemoryRead)
    );
    assert_eq!(
        access(false, "/api/memory/tool/memory_search"),
        PluginAccess::Needs(Permission::MemoryRead)
    );
    assert_eq!(
        access(false, "/api/memory/tool/memory_append"),
        PluginAccess::Needs(Permission::MemoryWrite)
    );
    assert_eq!(
        access(false, "/api/capture"),
        PluginAccess::Needs(Permission::MemoryWrite)
    );
    assert_eq!(
        access(true, "/api/harness-sessions/abc/transcript"),
        PluginAccess::Needs(Permission::SessionsRead)
    );
    assert_eq!(
        access(false, "/api/harness-sessions"),
        PluginAccess::Needs(Permission::SessionsControl)
    );
    assert_eq!(
        access(false, "/api/topics/abc/close"),
        PluginAccess::Needs(Permission::TopicsWrite)
    );
    assert_eq!(
        access(false, "/api/tasks"),
        PluginAccess::Needs(Permission::TasksDelegate)
    );
    assert_eq!(
        access(true, "/api/tasks/abc"),
        PluginAccess::Needs(Permission::TasksRead)
    );
    for refused in [
        (true, "/api/plugins"),
        (false, "/api/plugins/files/enable"),
        (false, "/api/memory/tags"),
        (false, "/api/tasks/abc/kill"),
        (true, "/api/files/list"),
        (true, "/mesh.json"),
    ] {
        assert_eq!(
            access(refused.0, refused.1),
            PluginAccess::Refused,
            "{refused:?}"
        );
    }
}

#[test]
fn approval_names_exactly_the_pending_permissions() {
    let requested = [Permission::SessionsRead, Permission::TasksRead];
    assert_eq!(
        pending(&requested, &[Permission::SessionsRead]),
        [Permission::TasksRead]
    );
    assert_eq!(
        approve(&requested, &[], &[Permission::SessionsRead]),
        Err(ApprovalError::Unapproved(vec![Permission::TasksRead]))
    );
    assert_eq!(
        approve(
            &requested,
            &[],
            &[
                Permission::SessionsRead,
                Permission::TasksRead,
                Permission::HostExec
            ]
        ),
        Err(ApprovalError::NotPending(vec![Permission::HostExec]))
    );
    assert_eq!(
        approve(
            &requested,
            &[Permission::SessionsRead],
            &[Permission::TasksRead]
        ),
        Ok(requested.to_vec())
    );
    assert_eq!(
        narrowed(&[Permission::TasksRead], &requested),
        [Permission::TasksRead]
    );
}

#[test]
fn manifest_problems_name_the_field() {
    assert!(check_manifest(&manifest(RUN).unwrap()).is_ok());
    let problems = [
        (format!(r#"{RUN}, "events": ["task.done"]"#), "events[0]"),
        (r#""run": {"command": []}"#.to_owned(), "run.command"),
        (
            r#""run": {"command": ["x"], "env": {"RIVETHUB_PLUGIN_TOKEN": "x"}}"#.to_owned(),
            "run.env.RIVETHUB_PLUGIN_TOKEN",
        ),
        (
            format!(
                r#"{RUN}, "settings": [{{"key": "a", "label": "A", "input": {{"type": "file"}}}}]"#
            ),
            "settings[0].input",
        ),
        (
            format!(r#"{RUN}, "jobs": [{{"id": "a", "title": "A"}}, {{"id": "a", "title": "B"}}]"#),
            "jobs",
        ),
    ];
    for (body, path) in problems {
        assert_eq!(
            check_manifest(&manifest(&body).unwrap()).unwrap_err().path,
            path,
            "{body}"
        );
    }
}

#[test]
fn tool_names_join_and_split_on_a_double_underscore() {
    let (plugin, tool) = split_tool_name("workflows__start_run").unwrap();
    assert_eq!(tool_name(&plugin, &tool), "workflows__start_run");
    assert!(split_tool_name("memory_search").is_err());
    assert!(split_tool_name("Bad__x").is_err());
}

#[test]
fn restarts_back_off_from_one_second_to_a_minute() {
    assert_eq!(restart_delay(1), Duration::from_secs(1));
    assert_eq!(restart_delay(2), Duration::from_secs(2));
    assert_eq!(restart_delay(6), Duration::from_secs(32));
    assert_eq!(restart_delay(7), Duration::from_secs(60));
    assert_eq!(restart_delay(40), Duration::from_secs(60));
}

#[test]
fn settings_are_checked_against_their_fields() -> Result<(), serde_json::Error> {
    use kernel::plugins::settings::{apply, effective, shown};
    use std::collections::BTreeMap;
    use wire::plugin::view::{ViewField, ViewValue};
    let fields: Vec<ViewField> = serde_json::from_str(
        r#"[{"key": "runs", "label": "Runs", "input": {"type": "number", "min": 1, "max": 8}, "value": 2, "required": true},
            {"key": "mode", "label": "Mode", "input": {"type": "select", "options": [{"value": "fast", "label": "Fast"}]}},
            {"key": "token", "label": "Token", "input": {"type": "secret"}}]"#,
    )?;
    let none = BTreeMap::new();
    let update = |pairs: &[(&str, Option<ViewValue>)]| -> BTreeMap<String, Option<ViewValue>> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), value.clone()))
            .collect()
    };
    let refused = |pairs: &[(&str, Option<ViewValue>)]| {
        apply(&fields, &none, &update(pairs)).map_err(|problem| problem.key)
    };
    assert_eq!(
        refused(&[("runs", Some(ViewValue::Number(9.0)))]),
        Err("runs".to_owned())
    );
    assert_eq!(
        refused(&[("runs", Some(ViewValue::Bool(true)))]),
        Err("runs".to_owned())
    );
    assert_eq!(
        refused(&[("mode", Some(ViewValue::Text("slow".into())))]),
        Err("mode".to_owned())
    );
    assert_eq!(
        refused(&[("colour", Some(ViewValue::Text("red".into())))]),
        Err("colour".to_owned())
    );
    let kept = apply(
        &fields,
        &none,
        &update(&[
            ("runs", None),
            ("token", Some(ViewValue::Text("s3cret".into()))),
        ]),
    )
    .map_err(|problem| serde::de::Error::custom(problem.to_string()))?;
    assert_eq!(
        effective(&fields, &kept).get("runs"),
        Some(&ViewValue::Number(2.0))
    );
    let visible = shown(&fields, &kept);
    assert_eq!(visible.secrets_set, ["token"]);
    assert!(!visible.values.contains_key("token"));
    Ok(())
}
