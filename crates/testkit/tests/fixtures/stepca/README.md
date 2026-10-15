# step-ca API recordings

Real exchanges with step-ca 0.30.2 (smallstep/certificates, Apache-2.0), recorded 2026-10-15 from an isolated test CA
(not the mesh CA; documentation addresses 192.0.2.0/24 and .example.test names only). `api.json` holds one case per exchange: request method, path, whether a client
certificate was presented (`none`, `current`, `revoked`) and the JSON body; response status, content type and body.
`crlPem` in api.json is `GET /1.0/crl?pem=true` after one revocation (DER is served without `?pem=true`, as application/pkix-crl).

Certificates in these bodies come from the throwaway test CA; no private key is recorded. One-time tokens in request
bodies are spent and expired. Configuration that produced them: provisioners `nodes` (JWK, 168h, SPIFFE node URI + the
SANs pinned in the token) and `devices` (JWK, 720h, SPIFFE device URI, clientAuth only); CRL enabled. See
/rivet-shared/plans/rust-runtime/notes/step-ca-validation-2026-10-15.md and PKI-DESIGN.md addendum 2026-10-15b.
