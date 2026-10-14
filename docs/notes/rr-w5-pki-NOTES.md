# PKI slice notes

Implemented the approved CertificateAuthority port in kernel and all five methods in a new PKI adapter. Holder keys stay with holders; the service signs verified P-256 CSRs, enforces exact SPIFFE SAN binding and lifetimes, persists hashed fifteen-minute one-use tokens and serial/revocation state, rate-limits issuance/signing, and emits signed X.509 CRLs. Import reads a caller-specified legacy layout once and never reads a root key. Intermediate keys are created only in private CA state with mode 0600. The complete port is ready for composition-root injection.

The adapter uses immutable shared signing material and per-operation SQLite connections on blocking tasks. SQLite transactions make token consumption and serial issuance atomic. Cancellation refuses work before it starts; started transactions finish. Independent reads are not behind a global lock. Revocation refreshes the CRL in state and at the optional configured compatibility path.

Updated B14 and the architecture map/table/CA section. Upgraded rcgen from 0.13 to 0.14.10 because CSR verification must use AWS-LC without introducing the forbidden ring dependency. Adapted the two existing gateway test signing calls to the new Issuer API. No gateway production routes or verifier were changed. The dependency gate rejected rustls-pemfile under RUSTSEC-2025-0134; PEM parsing uses its maintained rustls-pki-types successor directly, without relaxing deny.toml. A B14-scoped Dylint expectation preserves the required redacted token Debug implementation; deriving Debug would expose the secret.

Tests generate disposable CA material inside the repository target directory. The README explains that no historical recordings exist and identifies the typed policy scenarios. Coverage includes all principal kinds, SAN mismatches, bad signatures and CSR privileges, matching renewal, token atomic use/expiry/persistence, rate limits and cancellation, webpki leaf verification, CRL signature and revocation verification, nondefault imported key identifiers, serial continuation, import-once behavior, source immutability, private-key modes and path containment. Later gateway/CLI/app scenarios are named with reasons in a checked skip list.

The requested shared notes destination conflicts with the hard rule allowing writes only inside this repository. These notes remain here to respect that rule. No live shared CA or prohibited checkout was read. The lane Postgres endpoint responded to its protocol probe; no service was installed or reconfigured.

Validation completed with the lane DATABASE_URL and RIVETHUB_PG_URL, with SQLX_OFFLINE=true:

- `cargo xtask check --locked` exited 0. Last line: `GATE OK: 0 finding(s), cargo xtask check`.
- `cargo nextest run --workspace --all-features` exited 0. Last line: `Summary [  18.203s] 691 tests run: 691 passed, 9 skipped`.
- All eighteen PKI integration tests passed, including fifteen typed SAN-policy scenarios and the checked four-entry later-slice list.

The nine nextest skips are existing workspace exclusions. No PKI acceptance test is ignored. The source, import fixture and publication tests use no live shared CA. No push is performed.
