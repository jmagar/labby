//! Short-lived, exact Labby-to-Depot delegated operation assertions.

use std::collections::BTreeMap;

use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde::{Deserialize, Serialize};

pub const ASSERTION_TYPE: &str = "labby+depot-delegation+jwt";
pub const ASSERTION_ISSUER: &str = "labby";
pub const ASSERTION_AUDIENCE: &str = "depot";
pub const MAX_TTL_SECONDS: u64 = 60;
pub const MAX_VALUES: usize = 64;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DelegatedAuthorityEpochs {
    pub authority_schema: u64,
    pub organization_policy: u64,
    pub team_membership: Option<u64>,
    pub team_policy: Option<u64>,
    pub project_membership: Option<u64>,
    pub project_policy: Option<u64>,
    pub global_revision: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DepotDelegationClaims {
    pub iss: String,
    pub sub: String,
    pub aud: String,
    pub iat: u64,
    pub nbf: u64,
    pub exp: u64,
    pub jti: String,
    pub deployment_id: String,
    pub account_id: String,
    pub organization_id: String,
    pub team_id: Option<String>,
    pub project_id: Option<String>,
    pub principal_id: String,
    pub method: String,
    pub resource: String,
    pub operation: String,
    pub intent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_length: Option<u64>,
    pub scopes: Vec<String>,
    pub capabilities: Vec<String>,
    pub epochs: DelegatedAuthorityEpochs,
    #[serde(default)]
    pub delegation_chain: Vec<String>,
}

#[derive(Debug, thiserror::Error, Eq, PartialEq)]
pub enum DelegationError {
    #[error("delegated assertion input is invalid")]
    Invalid,
    #[error("delegated assertion signing failed")]
    Signing,
}

pub struct DepotDelegationSigner {
    active_key_id: String,
    keys: BTreeMap<String, EncodingKey>,
}
impl std::fmt::Debug for DepotDelegationSigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DepotDelegationSigner")
            .field("active_key_id", &self.active_key_id)
            .field("key_count", &self.keys.len())
            .finish()
    }
}

impl DepotDelegationSigner {
    pub fn new(
        active_key_id: String,
        keys: impl IntoIterator<Item = (String, Vec<u8>)>,
    ) -> Result<Self, DelegationError> {
        let keys = keys
            .into_iter()
            .map(|(id, der)| (id, EncodingKey::from_ed_der(&der)))
            .collect::<BTreeMap<_, _>>();
        if !valid(&active_key_id)
            || keys.is_empty()
            || keys.len() > 8
            || !keys.contains_key(&active_key_id)
            || keys.keys().any(|id| !valid(id))
        {
            return Err(DelegationError::Invalid);
        }
        Ok(Self {
            active_key_id,
            keys,
        })
    }
    pub fn issue(&self, claims: DepotDelegationClaims) -> Result<String, DelegationError> {
        validate(&claims)?;
        let mut header = Header::new(Algorithm::EdDSA);
        header.typ = Some(ASSERTION_TYPE.into());
        header.kid = Some(self.active_key_id.clone());
        encode(
            &header,
            &claims,
            self.keys
                .get(&self.active_key_id)
                .ok_or(DelegationError::Invalid)?,
        )
        .map_err(|_| DelegationError::Signing)
    }
}

fn valid(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value.trim() == value
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':' | '/'))
}
fn validate(c: &DepotDelegationClaims) -> Result<(), DelegationError> {
    if c.iss != ASSERTION_ISSUER
        || c.aud != ASSERTION_AUDIENCE
        || c.sub != c.principal_id
        || c.exp <= c.iat
        || c.nbf > c.iat
        || c.exp - c.iat > MAX_TTL_SECONDS
        || !matches!(
            c.method.as_str(),
            "GET" | "POST" | "PUT" | "PATCH" | "DELETE"
        )
        || !c.resource.starts_with('/')
        || ![
            &c.jti,
            &c.deployment_id,
            &c.account_id,
            &c.organization_id,
            &c.principal_id,
            &c.operation,
            &c.intent_id,
        ]
        .into_iter()
        .all(|v| valid(v))
        || c.scopes.len() > MAX_VALUES
        || c.capabilities.len() > MAX_VALUES
        || c.delegation_chain.len() > 8
        || c.content_digest.as_ref().is_some_and(|digest| {
            digest.len() != 71
                || !digest.starts_with("sha256:")
                || !digest[7..]
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
        || c.team_id.as_deref().is_some_and(|v| !valid(v))
        || c.project_id.as_deref().is_some_and(|v| !valid(v))
    {
        return Err(DelegationError::Invalid);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::pkcs8::EncodePrivateKey;
    fn claims() -> DepotDelegationClaims {
        DepotDelegationClaims {
            iss: ASSERTION_ISSUER.into(),
            sub: "p1".into(),
            aud: ASSERTION_AUDIENCE.into(),
            iat: 10,
            nbf: 10,
            exp: 40,
            jti: "j1".into(),
            deployment_id: "d1".into(),
            account_id: "a1".into(),
            organization_id: "o1".into(),
            team_id: Some("t1".into()),
            project_id: Some("pr1".into()),
            principal_id: "p1".into(),
            method: "POST".into(),
            resource: "/api/artifacts".into(),
            operation: "artifact.create".into(),
            intent_id: "i1".into(),
            content_digest: None,
            content_length: None,
            scopes: vec!["skills:write".into()],
            capabilities: vec!["scope.create".into()],
            epochs: DelegatedAuthorityEpochs {
                authority_schema: 1,
                organization_policy: 1,
                team_membership: Some(1),
                team_policy: Some(1),
                project_membership: Some(1),
                project_policy: Some(1),
                global_revision: 1,
            },
            delegation_chain: vec!["labby".into()],
        }
    }
    #[test]
    fn exact_claims_are_signed_and_ttl_is_bounded() {
        let key = ed25519_dalek::SigningKey::from_bytes(&[9; 32]);
        let der = key.to_pkcs8_der().unwrap();
        let signer = DepotDelegationSigner::new(
            "current".into(),
            [("current".into(), der.as_bytes().to_vec())],
        )
        .unwrap();
        let token = signer.issue(claims()).unwrap();
        assert_eq!(
            jsonwebtoken::decode_header(&token).unwrap().kid.as_deref(),
            Some("current")
        );
        let mut invalid = claims();
        invalid.exp = invalid.iat + 61;
        assert_eq!(signer.issue(invalid).unwrap_err(), DelegationError::Invalid);
    }
}
