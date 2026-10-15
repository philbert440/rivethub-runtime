# RivetHub certificate commands

The PKI CLI implements the approved design in `docs/PKI.md` and the step-ca addendum. `rh ca` still uses the kernel CA port and the typed scenarios in `tests/fixtures/pki/principals.json`. `rh cert renew` talks to step-ca. Its acceptance tests replay `crates/testkit/tests/fixtures/stepca/api.json` through a local HTTPS signer. No production CA material is read or checked in.

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
  --ca-url https://ca.example:9000 \
  --trust-root /trusted/root.cert.pem \
  --token-file /protected/enrollment-token
```

The first run, and any later run that passes `--token-file`, generates a P-256 private key on the holder when `tls/holder.key.pem` is absent. The key never leaves the holder. The command reads the `sans` claim from the step-ca one-time token, builds a CSR containing exactly those names, and calls `stepca::Authority::sign`. step-ca verifies the token. The holder does not put the principal into the CSR. The issued certificate must contain that SPIFFE URI and every name the token pinned, and no other name. Renewal with no token calls `stepca::Authority::renew` over mTLS using the current certificate and key.

The supplied root is the only HTTPS trust anchor and the only chain anchor. The CA URL must be an HTTPS origin; credentials, query strings and fragments are refused. Same-origin 307 and 308 redirects are followed. A redirect to another origin is refused before the request is sent there.

Holder state defaults to `~/.rivethub`, and `--state` selects another directory. The files live in `{state}/tls/`: `holder.key.pem`, `holder.cert.pem` (leaf, then intermediate), and `renew.lock`. The directory is mode 0700. The key and certificate are mode 0600 and have one hard link. Publication writes a temporary file in that directory, sets mode 0600, and renames it into place. Before replacement, the certificate is checked against the pinned root, the local public key, client-auth use, the validity window, the principal URI, and the requested names. A certificate that has already reached two thirds of its lifetime is refused. Renewal keeps the existing key. A failed request leaves the current files in place. Symlinks, `..`, a shared-prefix sibling, and a linked or permissive key are refused. Paths with spaces and relative paths are accepted when they stay inside the state directory.

Without `--once`, the process waits until two thirds of the certificate's actual lifetime, then renews. `--once` enrolls or renews once and exits. Cancellation before the command starts is a failure. Cancellation during the wait or an in-flight request returns success and does not publish a certificate. An atomic publish that has already started is finished. The command does not re-enroll silently when renewal fails. A later process that passes `--token-file` enrolls again; a spent token is a CA refusal and the stored files stay. Successful holder commands print no stdout and do not log the token.

## Coverage and exclusions

`ca` tests are unchanged. `cert` tests use a local TLS server that signs the submitted CSR and returns the recorded `/1.0/sign` and `/1.0/renew` document shape, including the recorded refusal text. They cover token-SAN enrollment, mTLS renewal, key locality, same-origin and foreign redirects, a wrong key, a wrong principal, an extra name, a reused token, a revoked renewal, cancellation, and unsafe paths. Paused-clock tests cover the renewal timer. `stepca_scope` requires every case in the step-ca recording, plus the recorded CRL, to be exercised or skipped with a reason. This command skips `sign-san-not-granted`, `renew-without-certificate`, `revoke-own-mtls`, `roots`, and `crl`: it never builds an ungranted CSR, never renews without a certificate, and it does not revoke, fetch roots, or read the CRL. `pki_scope` still requires an empty `tests/fixtures/pki/skipped.json`.

The production gateway routes, `rh ca`, and the pki adapter remain separate slices. The test server is an acceptance fixture.

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
