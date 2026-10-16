use wire::task::TaskPermissionDecision;

use rusqlite::OptionalExtension as _;

use super::{Connection, TaskError, Uuid, encode, ensure, immediate, params, sql_error};

pub(crate) fn append_permission(
    conn: &mut Connection,
    id: Uuid,
    decision: &TaskPermissionDecision,
) -> Result<(), TaskError> {
    let tx = immediate(conn, "append permission")?;
    ensure(&tx, "append permission")?;
    let shape: Option<(String, Option<String>)> = tx
        .query_row(
            "SELECT json_type(spec), json_type(spec, '$.permissionDecisions')
             FROM rh_tasks WHERE id = ?1",
            params![id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|source| sql_error("append permission", source))?;
    let Some((spec_type, decisions_type)) = shape else {
        return Err(TaskError::Missing { id });
    };
    if spec_type != "object" || !matches!(decisions_type.as_deref(), None | Some("array")) {
        return Err(TaskError::Stored(format!(
            "permissionDecisions cannot be appended (spec {spec_type}, decisions {decisions_type:?})"
        )));
    }
    tx.execute(
        "UPDATE rh_tasks SET spec = json_set(spec, '$.permissionDecisions',
            json_insert(COALESCE(json_extract(spec, '$.permissionDecisions'), '[]'),
                '$[#]', json(?2))) WHERE id = ?1",
        params![id.to_string(), encode(decision)?],
    )
    .map_err(|source| sql_error("append permission", source))?;
    tx.commit()
        .map_err(|source| sql_error("append permission", source))
}
