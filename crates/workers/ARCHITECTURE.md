# Worker queue ownership

Workers receive stores, a model provider and a kernel `JobQueue`. Each claimed job
runs under the runner's `JoinSet`, semaphore and child `CancellationToken`.
Shutdown or a queue failure cancels active handlers and awaits their settlement.
A requested role must have its dependencies before the first subscription or
claim; `all` includes wiki building and cannot silently omit it.

The runner owns one `JobNotifications` subscription for its lifetime. Subscription
completes before claiming, so an insertion between an empty claim and waiting is
observable. PostgreSQL owns a dedicated LISTEN connection; SQLite clones share a
watch channel. The poll deadline covers scheduled retries, missed notifications
and writes made through separately opened SQLite connections. Notification errors
propagate rather than turning into polling-only operation.

## Why two queue-facing kernel contracts remain

`queue::JobQueue` serves background memory workers: typed job names, role-based
claims, job keys and revival of dead jobs. `tasks::TaskJobs` serves the task
engine: selector-based claims, task job submission, job reads and notification
waiting. These are separate consumer contracts over the same PostgreSQL backend,
not separate definitions of the Graphile wire format. Both ports live in kernel;
services depend on neither another service nor its adapter.

Keeping these contracts separate means the SQLite background-worker queue need
not claim support for task-engine job reads or selectors it does not implement.
A union would require implementing those capabilities on every adapter, or adding
partial methods that fail as unsupported. Neither is a safe provider/worker fix.
The adapters can share database operations without requiring every consumer and
backend to implement the union of both interfaces. This round therefore extends
the existing worker contract only with an owned notification subscription.
