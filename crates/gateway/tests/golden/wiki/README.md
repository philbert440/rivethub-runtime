# Wiki gateway coverage

The vendored gateway HTTP and WebSocket traffic has no wiki request exchanges.

The files `users-tenancy.json` and `users-invalid.json` are auth configuration
fixtures, not traffic recordings; the latter intentionally contains invalid JSON.
They belong to the existing auth layer and are not wiki test inputs.

`wiki` exercises typed API responses, HTML escaping, request validation, local
redirects, failures, and the existing gateway guard. The composition crate's
`rivethub::wiki` tests exercise the gateway over HTTP against real wiki tables
in isolated SQLx test databases.
These are newly written tests, not recordings of the previous runtime. Historical
boundary parity cannot be claimed without the missing wiki recordings or an
accessible boundary specification.
