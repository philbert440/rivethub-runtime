# Archived task rows

`ts-rows.json` holds task rows recorded from the TypeScript runtime. Those rows now live in
`rh_tasks_archive`. The agent loop's exclusion test reads the `production-exclude-tools` row's
`spec.excludeTools`, the tool list a delegated agent was given.
