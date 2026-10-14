use std::collections::BTreeMap;
use std::net::SocketAddr;

use axum::extract::ConnectInfo;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::error::ApiError;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct UserId(String);

impl UserId {
    pub fn new(id: String) -> Result<Self, ApiError> {
        if id.is_empty() || id.chars().any(char::is_control) {
            return Err(ApiError::BadRequest("invalid user id".into()));
        }
        Ok(Self(id))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone)]
pub struct PeerCertificate(pub [u8; 32]);

#[derive(Debug, Clone, Default)]
pub struct Tenancy {
    local: Option<UserId>,
    tokens: Vec<([u8; 32], UserId)>,
    certificates: BTreeMap<[u8; 32], UserId>,
}

impl Tenancy {
    pub fn local(user: UserId) -> Self {
        Self {
            local: Some(user),
            ..Self::default()
        }
    }

    pub fn with_token(mut self, token: &str, user: UserId) -> Result<Self, ApiError> {
        if token.is_empty() {
            return Err(ApiError::BadRequest("empty user token".into()));
        }
        self.tokens
            .push((Sha256::digest(token.as_bytes()).into(), user));
        Ok(self)
    }

    #[must_use]
    pub fn with_certificate(mut self, fingerprint: [u8; 32], user: UserId) -> Self {
        self.certificates.insert(fingerprint, user);
        self
    }

    pub(crate) fn resolve(&self, parts: &Parts) -> Result<UserId, ApiError> {
        if let Some(header) = parts.headers.get(axum::http::header::AUTHORIZATION) {
            let text = header.to_str().map_err(|_| ApiError::Unauthorized)?;
            let token = text.strip_prefix("Bearer ").ok_or(ApiError::Unauthorized)?;
            let digest: [u8; 32] = Sha256::digest(token.as_bytes()).into();
            let mut found = None;
            for (secret, user) in &self.tokens {
                if bool::from(secret.ct_eq(&digest)) {
                    found = Some(user.clone());
                }
            }
            return found.ok_or(ApiError::Unauthorized);
        }
        if let Some(peer) = parts.extensions.get::<PeerCertificate>() {
            return self
                .certificates
                .get(&peer.0)
                .cloned()
                .ok_or(ApiError::Unauthorized);
        }
        if let Some(ConnectInfo(remote)) = parts.extensions.get::<ConnectInfo<SocketAddr>>()
            && crate::auth::is_loopback_remote(*remote)
        {
            return self.local.clone().ok_or(ApiError::Unauthorized);
        }
        Err(ApiError::Unauthorized)
    }
}

impl FromRequestParts<std::sync::Arc<crate::Gateway>> for UserId {
    type Rejection = ApiError;

    fn from_request_parts(
        parts: &mut Parts,
        gateway: &std::sync::Arc<crate::Gateway>,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        std::future::ready(gateway.tenancy.resolve(parts))
    }
}
