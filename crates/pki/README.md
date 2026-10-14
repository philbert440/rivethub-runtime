# RivetHub PKI adapter

`Pki` implements every method of `kernel::ca::CertificateAuthority`. The composition root injects it into the gateway and CLI. Its only runtime workspace dependency is `kernel`; cryptography uses rcgen, AWS-LC, rustls-webpki, rustls-pki-types and x509-parser.

Holders generate ECDSA P-256 keys and send a signed CSR. The adapter verifies the CSR signature and permits exactly one SPIFFE URI SAN matching the enrollment principal or the caller's already verified renewal principal. DNS names, extra identities, foreign trust domains, CA privileges and unrelated key usages are refused. Principal components are 1–128 ASCII unreserved URI bytes, excluding `.` and `..`. Nodes and agents receive seven days; devices receive thirty. A leaf never outlives either issuer. CNs confer no identity. Certificates contain the holder's public key and an empty subject.

Enrollment tokens contain 256 OS-random bits, expire after fifteen minutes and are stored only as SHA-256 digests. Digest comparisons use `subtle`. A SQLite immediate transaction commits token consumption with issuance and serial allocation, so concurrent requests cannot reuse a token. Invalid CSRs do not consume tokens. Tokens and revocations survive restart. Token issuance and signing each admit sixty attempts per minute per CA; failed signing attempts count. The authenticated operator interface must control access to `issue_token` and `revoke`. `SignAuthority::Renewal` must come from the gateway's TLS verifier, never a request body.

The signing material is immutable behind `Arc`. Each database operation owns its connection. Disk I/O and certificate work run in `spawn_blocking`; independent chain reads do not share a process-wide mutex. Cancellation refuses queued work before it starts. Once a blocking transaction starts it completes atomically, including when its caller disconnects. Shutdown callers must drain their outstanding port calls before releasing the service. Time is anchored to UTC at open and advanced with Tokio's monotonic clock; tests advance that clock without sleeping.

## Import and state

`Pki::import(source, state, layout, cancellation)` reads an existing CA exactly once. `Pki::open(state, cancellation)` subsequently needs only state. Import never reads, generates or stores the root private key. `Pki::initialize(state, root_key_output, cancellation)` generates a fresh root and intermediate, exports the root key exclusively with mode 0600 outside state, and retains only the intermediate key. The CLI requires an explicit export path; the operator moves that export offline. `ImportLayout` supplies relative file locations, so installations can name their existing files explicitly. The generated fixture uses this conventional OpenSSL/rivet-ca layout:

```text
root/certs/ca.cert.pem
intermediate/certs/intermediate.cert.pem
intermediate/private/intermediate.key.pem
intermediate/index.txt
intermediate/serial
```

The index has six tab-separated OpenSSL columns; valid, expired and revoked records are imported, including revoked serials. The next hexadecimal serial must exceed every imported serial. Legacy subjects are retained only in the source: they never become SPIFFE identities through a CN guess. `revoke_serial` handles imported leaves that have no typed principal mapping. Principal revocation blocks that principal, invalidates its tokens and revokes all of its newly issued serials. Re-enrollment after a principal-wide revocation requires a new principal; there is no implicit unrevocation.

State is an absolute, user-owned directory with mode 0700, ordinarily `~/.rivethub/ca`. A process lock prevents two owners. The intermediate key is created exclusively as `intermediate.key.pem`, mode 0600, with no key backups or holder key files. Existing key hard links, symlink paths, traversal and paths outside the import root are refused. An incomplete import cannot be opened or silently retried over existing state. The caller must inspect and remove an incomplete destination before retrying.

`crl` produces a signed standard X.509 CRL with a monotonic CRL number and one-day next-update bound, capped by issuer expiry. It also atomically writes `intermediate.crl.pem` in state. Both revocation methods refresh that file after durably recording revocations. `with_crl_publication(path)` configures the existing compatibility CRL path before the adapter is shared. CRL reads and revocations atomically replace that public file as well as the state copy. The destination must be outside CA state, with no traversal or symlinks; no key goes to the share. Deployment must refresh the CRL before next-update. CRLs preserve the imported intermediate's subject-key identifier, including a nondefault identifier.

## Recordings and acceptance tests

There are **no historical PKI recordings**: this is the approved new B14 design. No test reads the live shared CA, and no CA key or certificate fixture is checked in. `tests/support` generates a root and intermediate with rcgen and writes a disposable import layout under the repository's target directory. The root key and every holder key remain in memory; the disposable source contains only the intermediate key required to exercise import.

`tests/fixtures/san-policy.json` contains typed design scenarios, not captured network traffic. `policy` replays that table through the kernel port using testkit's fixture path helper and compares typed SANs, decisions and lifetimes. Other acceptance tests prove token atomicity, expiry, hashed persistence, renewal binding, signature and privilege refusals, rate limits, cancellation, CRL verification and webpki revocation rejection, import continuity, restart behavior, source immutability, key containment and compatibility CRL publication. `tests/fixtures/skipped.json` names later-slice scenarios and explains each exclusion; `scope` checks that list.

Run `cargo test -p pki`, then the workspace gate and nextest suite with the lane database URLs.

The approved design named rustls-pemfile. Its maintained underlying parser is now in rustls-pki-types; this adapter uses that successor directly because RUSTSEC-2025-0134 makes the deprecated wrapper fail the workspace dependency gate. No dependency rule was relaxed.

SPIFFE formatting and parsing now live on `kernel::ca::Principal`; the issuer and CLI share this policy. The strict single-certificate PEM decoder and contained-file helpers are also shared with the CLI composition crate.
