# RivetHub certificate commands

The PKI CLI implements the approved design in `docs/PKI.md`. There are no historical recordings for this new B14 boundary. The typed principal scenarios in `tests/fixtures/pki/principals.json` are design fixtures, not captured traffic. Acceptance tests exercise real generated certificates and a disposable HTTPS server backed by the kernel CA port. No production CA material is read or checked in.

## Operator commands

- `rh ca init --root-key-out /offline/export/root.key.pem` creates a P-256 root and intermediate. The root key is written once, exclusively, with mode 0600 at the operator-selected location outside CA state. It is never written in CA state. Move that export offline after initialization; only the intermediate key remains in CA state. Existing output files and existing CA databases are refused.
- `rh ca import <legacy-directory>` imports the conventional OpenSSL layout described in `crates/pki/README.md`, including the existing chain, index and next serial. It does not read a legacy root key or modify the source. Import is one-time.
- `rh ca token spiffe://rivet.mesh/node/node-a` prints one 256-bit enrollment token followed by one newline, exactly once. The CA stores only its digest. Redirect stdout to a protected file for delivery to the holder.
- `rh ca revoke <SPIFFE-principal|hexadecimal-serial>` revokes the principal or one issued/imported serial and updates the CRL.
- `rh ca crl` prints the signed X.509 CRL as PEM.
- `rh ca status` prints the issuer subjects and expiration dates, without key material or tokens.

CA state defaults to `~/.rivethub/ca`. `--state <directory>` selects another state directory, and `--crl-out <path>` atomically publishes the compatibility CRL outside it. The directory lock prevents concurrent owners. Commands use the complete `kernel::ca::CertificateAuthority` port; initialization/import/open and legacy serial revocation use the adapter's existing or added inherent administrative methods. State/database logic remains in `pki`.

## Holder commands

```text
rh cert renew --principal spiffe://rivet.mesh/node/node-a \
  --ca-url https://datahub.example:7443 \
  --trust-root /trusted/root.cert.pem \
  --token-file /protected/enrollment-token
```

The first run generates a P-256 private key on the holder, writes it exclusively with mode 0600, and signs a CSR with an empty subject and exactly the principal's SPIFFE URI SAN. The token is read from a file, never accepted as a command-line secret. The supplied root is the sole HTTPS trust anchor. An HTTPS origin is required; credentials, query strings and fragments are refused. Same-origin HTTPS redirects work, while origin changes are refused before forwarding credentials.

`POST /ca/v1/sign` carries `wire::ca::SignRequest` (`csr`); enrollment uses an `Authorization: Bearer` token. Renewal sends the current certificate through mTLS and no bearer token. `wire::ca::SignResponse` (`leaf`, `chain`) is also the kernel's certificate-chain result, so there is one response type. These types define the new wire contract for the later gateway route slice.

Holder state defaults to `~/.rivethub/cert`, overridable with `--state`. It contains `holder.key.pem`, `holder.cert.pem` (leaf followed by issuer chain), and the process lock. The certificate is verified against the pinned root, its local public key, client-auth usage, current validity, and exact SPIFFE SAN before atomic replacement. Renewal retains the local private key and generates a fresh signed CSR. Failed requests preserve the current certificate. Symlinks, traversal and linked or permissive private keys are refused. Paths with spaces and relative CLI paths are supported.

Without `--once`, the process renews at two thirds of the certificate's actual lifetime. Restarts read the existing certificate and resume that schedule; omit `--token-file` on restarts. `--once` enrolls or immediately renews and exits. Cancellation interrupts network requests and timers; an already started atomic disk operation is drained before exit. Network and validation failures return the existing failure exit code without printing response bodies or credentials. There is no silent retry or re-enrollment fallback. A supervisor can restart the process after transient failure. Holder commands produce no successful stdout or secret logs.

## Coverage and exclusions

`ca` tests cover initialization and root export, one-time import and chain continuity, token output and consumption, principal/serial revocation, CRL publication, status, and unsafe paths. `cert` tests use real TLS to prove enrollment, mTLS renewal, key locality, same-origin redirects, malicious response refusal, cancellation and restart scheduling. Paused-clock tests exercise the renewal timer without wall-clock sleeps. `pki_scope` verifies the contract reference and empty skip list in `tests/fixtures/pki/skipped.json`: every scenario in this CLI slice is exercised, and other slices' cases are not listed as skipped recordings.

The production gateway routes and TLS-to-principal policy, legacy device registry mapping, and desktop/Android enrollment remain separate slices. The test server is only an acceptance fixture. No CLI scenarios are skipped.

# RivetHub systemd commands

`rh service init|start|stop|restart|status|logs` implements B9's service contract in
`crates/testkit/tests/fixtures/cli/README.md`. Effective UID selects system mode
(`/etc/systemd/system/rivethub.service`) or user mode
(`~/.config/systemd/user/rivethub.service`). Init generates the unit from `service::Unit`
using this executable's absolute path and the configured state directory's `.env`.
It publishes the file exclusively, reports user lingering, and refuses existing
files, traversal and directory links escaping the home directory. Systemd quoting
preserves spaces, quotes, Unicode, dollar signs and percent signs literally.

Start and restart reload the manager, execute the action and show status. A failed
reload or action stops the sequence and returns failure with stderr. Status retains
the child exit code. Logs inherits the terminal streams and follows the journal until
interrupted; cancellation kills and reaps the child before returning success. All
commands use `tokio::process`; blocking account lookup and unit publication run on
`spawn_blocking`.

The fixture directory currently contains the written contract only, with no captured
CLI transcripts. The `service` acceptance tests exercise every service row using
fake `systemctl`, `journalctl` and `loginctl` executables in a temporary PATH. They
check both scopes, unit serialization, overwrite refusal, path containment, quoting,
status codes, subprocess errors and event-driven cancellation. No service scenario is
skipped; the existing checked deferred list covers the remaining CLI commands in
that same fixture README. `rh start` supervises `rh gateway`, `rh agent` when
Postgres memory would start, `rh worker --role`, and `rh mcp` when its network
transport is configured. It holds the state-directory pid lock, restarts children
with backoff, and forwards SIGTERM. `rh local` is deferred until its own slice
lands; the SQLite store implements the kernel memory ports.
