use crate::error::{Error, invalid, refused};
use kernel::ca::Principal;
use rcgen::{
    CertificateParams, CertificateSigningRequestParams, DistinguishedName, ExtendedKeyUsagePurpose,
    IsCa, KeyUsagePurpose, SanType,
};
use x509_parser::prelude::{FromDer, X509CertificationRequest};

pub(crate) fn uri(principal: &Principal) -> Result<String, Error> {
    let (kind, components) = match principal {
        Principal::Node { node } => ("node", vec![node]),
        Principal::Agent { node, agent } => ("agent", vec![node, agent]),
        Principal::Device { user, device } => ("device", vec![user, device]),
    };
    for component in &components {
        if component.is_empty()
            || component.len() > 128
            || component == &"."
            || component == &".."
            || !component.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~')
            })
        {
            return Err(refused(
                "principal components must be nonempty unreserved URI segments",
            ));
        }
    }
    Ok(format!(
        "spiffe://rivet.mesh/{kind}/{}",
        components
            .into_iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join("/")
    ))
}

pub(crate) fn lifetime(principal: &Principal) -> time::Duration {
    match principal {
        Principal::Node { .. } | Principal::Agent { .. } => time::Duration::days(7),
        Principal::Device { .. } => time::Duration::days(30),
    }
}

pub(crate) fn request(pem: &str, expected: &str) -> Result<CertificateSigningRequestParams, Error> {
    if pem.len() > 16_384 {
        return Err(invalid("CSR exceeds 16 KiB"));
    }
    if !pem
        .trim_start()
        .starts_with("-----BEGIN CERTIFICATE REQUEST-----")
    {
        return Err(invalid("unexpected data before CSR PEM"));
    }
    let mut reader = pem.as_bytes();
    let Some((rustls_pki_types::pem::SectionKind::Csr, der)) =
        rustls_pki_types::pem::from_buf(&mut reader)
            .map_err(|error| invalid(&error.to_string()))?
    else {
        return Err(invalid("expected one PEM certificate request"));
    };
    if !reader.iter().all(u8::is_ascii_whitespace) {
        return Err(invalid("trailing CSR data"));
    }
    let der = rustls_pki_types::CertificateSigningRequestDer::from(der);
    let (remaining, parsed) =
        X509CertificationRequest::from_der(&der).map_err(|error| invalid(&error.to_string()))?;
    if !remaining.is_empty() || parsed.certification_request_info.attributes().len() != 1 {
        return Err(invalid("unexpected CSR attributes or trailing DER"));
    }
    validate_structure(&parsed)?;
    let mut csr = CertificateSigningRequestParams::from_der(&der)
        .map_err(|error| invalid(&error.to_string()))?;
    if csr.public_key.algorithm() != &rcgen::PKCS_ECDSA_P256_SHA256 {
        return Err(invalid("holder key must be ECDSA P-256"));
    }
    if csr.params.subject_alt_names.len() != 1
        || !matches!(csr.params.subject_alt_names.first(), Some(SanType::URI(value)) if value.as_str() == expected)
    {
        return Err(refused(
            "CSR must contain exactly the authorized SPIFFE URI SAN",
        ));
    }
    if !matches!(csr.params.is_ca, IsCa::NoCa | IsCa::ExplicitNoCa)
        || csr
            .params
            .key_usages
            .iter()
            .any(|usage| !matches!(usage, KeyUsagePurpose::DigitalSignature))
        || csr.params.extended_key_usages.iter().any(|usage| {
            !matches!(
                usage,
                ExtendedKeyUsagePurpose::ClientAuth | ExtendedKeyUsagePurpose::ServerAuth
            )
        })
    {
        return Err(refused(
            "CSR requests certificate authority or unrelated key privileges",
        ));
    }
    let mut params = CertificateParams::default();
    params.subject_alt_names = csr.params.subject_alt_names;
    params.distinguished_name = DistinguishedName::new();
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![
        ExtendedKeyUsagePurpose::ClientAuth,
        ExtendedKeyUsagePurpose::ServerAuth,
    ];
    params.use_authority_key_identifier_extension = true;
    csr.params = params;
    Ok(csr)
}

fn validate_structure(csr: &X509CertificationRequest<'_>) -> Result<(), Error> {
    use x509_parser::cri_attributes::ParsedCriAttribute;
    let info = &csr.certification_request_info;
    if info.version.0 != 0
        || info.subject_pki.algorithm.algorithm.to_id_string() != "1.2.840.10045.2.1"
    {
        return Err(invalid(
            "expected PKCS#10 version zero and an EC public key",
        ));
    }
    let curve = info
        .subject_pki
        .algorithm
        .parameters
        .as_ref()
        .ok_or_else(|| invalid("EC curve is missing"))?
        .as_oid()
        .map_err(|error| invalid(&error.to_string()))?;
    if curve.to_id_string() != "1.2.840.10045.3.1.7" {
        return Err(invalid("holder public key curve must be P-256"));
    }
    let attribute = info
        .attributes()
        .first()
        .ok_or_else(|| invalid("missing CSR extensions"))?;
    let ParsedCriAttribute::ExtensionRequest(extensions) = attribute.parsed_attribute() else {
        return Err(invalid("only extensionRequest is accepted"));
    };
    let mut seen = std::collections::HashSet::new();
    for extension in &extensions.extensions {
        if !seen.insert(&extension.oid) {
            return Err(invalid("duplicate CSR extension"));
        }
    }
    Ok(())
}
