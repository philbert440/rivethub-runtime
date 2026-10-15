# Gateway CA routes

`routes::ca::router()` serves the certificate authority on the gateway listener:

- `POST /ca/v1/sign` accepts a JSON `SignRequest` (`csr`). A bearer enrollment token signs when the connection has no verified SPIFFE principal. A verified client certificate renews as that principal and the token is ignored. The response is `SignResponse`.
- `GET /ca/v1/chain` returns the intermediate and root as PEM.
- `GET /ca/v1/crl` returns the current X.509 CRL as PEM.

The router reads `Gateway`'s injected `CertificateAuthority`. It does not mount itself. The composition root merges `router()` onto the listener. Sign requests share one tower middleware budget of sixty per minute and receive `429` past that. Chain and CRL reads are not limited. Token issuance stays on the port.

The listener asks for a client certificate and still accepts a handshake with none, so a token can enroll. A presented certificate is verified, including the configured CRL, before `PeerCertificate::from_verified_leaf` parses one SPIFFE URI with `kernel::ca::Principal`. Device principals feed the existing users-registry identity. Node and agent principals are recognized and refused for user routes. Legacy `device:` / OU `client` certificates still map through that registry and are not renewal principals.

There are no historical PKI recordings. The cli crate's `gateway_ca` test injects a test-local `CertificateAuthority`, serves `router()` through `server::serve_router_with` and `tls::TlsPaths`, and compares typed certificates, SAN URIs, CRL serials and status codes with the port. `tests/ca.rs` checks the sign budget. `tests/fixtures/ca/skipped.json` names the scenarios this slice does not cover, and a test checks that list. The production authority is `pki`'s step-ca client, wired by the composition root when gateway config names one.
