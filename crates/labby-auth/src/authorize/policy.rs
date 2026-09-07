use tracing::{debug, warn};

use crate::error::AuthError;
use crate::state::AuthState;
use crate::util::fingerprint;

/// Enforces the configured email allowlist using verified provider identity claims.
pub(crate) fn check_email_allowlist(
    email: Option<&str>,
    email_verified: Option<bool>,
    hosted_domain: Option<&str>,
    allowed_emails: &[String],
    allowed_domains: &[String],
) -> Result<(), AuthError> {
    if allowed_emails.is_empty() && allowed_domains.is_empty() {
        return Ok(());
    }
    if email_verified != Some(true) {
        warn!("oauth callback rejected: identity provider did not return a verified email address");
        return Err(AuthError::AuthFailed(
            "identity provider did not return a verified email address".to_string(),
        ));
    }
    let Some(email) = email else {
        warn!("oauth callback rejected: identity provider did not return an email address");
        return Err(AuthError::AuthFailed(
            "identity provider did not return an email address".to_string(),
        ));
    };
    let email = email.trim();
    if allowed_emails
        .iter()
        .any(|allowed| allowed.eq_ignore_ascii_case(email))
    {
        return Ok(());
    }
    if let Some(domain) = hosted_domain
        .map(str::trim)
        .filter(|domain| !domain.is_empty())
        && allowed_domains
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(domain))
    {
        return Ok(());
    }
    warn!(
        email_id = %fingerprint(email),
        "oauth callback rejected: email not in allowed list"
    );
    Err(AuthError::AuthFailed(
        "identity is not permitted to access this gateway".to_string(),
    ))
}

pub(crate) fn validate_response_type(response_type: &str) -> Result<(), AuthError> {
    if response_type == "code" {
        return Ok(());
    }
    warn!(
        response_type = %response_type,
        "oauth authorize rejected: unsupported response_type"
    );
    Err(AuthError::Validation(
        "response_type must be `code`".to_string(),
    ))
}

pub(crate) fn validate_scope(
    state: &AuthState,
    resource: &str,
    scope: &str,
) -> Result<String, AuthError> {
    let canonical = crate::metadata::canonical_resource_url(state);
    let supported = if resource.trim_end_matches('/') == canonical {
        state.config.scopes_supported.clone()
    } else {
        state
            .allowed_resource_scopes(resource)
            .filter(|scopes| !scopes.is_empty())
            .ok_or_else(|| {
                AuthError::Validation(format!(
                    "resource must be `{canonical}` or a configured protected MCP route"
                ))
            })?
    };
    let normalized = scope.trim();
    if normalized.is_empty() {
        let scope = if resource.trim_end_matches('/') == canonical {
            state.config.default_scope.clone()
        } else {
            supported.join(" ")
        };
        debug!(resource_id = %fingerprint(resource), scope_id = %fingerprint(&scope), "oauth authorize defaulted scope");
        return Ok(scope);
    }
    let requested = normalized.split_whitespace().collect::<Vec<_>>();
    if requested
        .iter()
        .all(|scope| supported.iter().any(|allowed| allowed == scope))
    {
        let scope = requested.join(" ");
        debug!(resource_id = %fingerprint(resource), requested_scope_id = %fingerprint(normalized), normalized_scope_id = %fingerprint(&scope), "oauth authorize scope accepted");
        return Ok(scope);
    }
    warn!(scope_id = %fingerprint(normalized), resource_id = %fingerprint(resource), "oauth authorize rejected: unsupported scope");
    Err(AuthError::Validation(format!(
        "scope must be one of: {}",
        supported.join(", ")
    )))
}

pub(crate) fn validate_resource(
    state: &AuthState,
    requested: Option<&str>,
) -> Result<String, AuthError> {
    let canonical = crate::metadata::canonical_resource_url(state);
    let Some(requested) = requested.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(canonical);
    };
    let requested = requested.trim_end_matches('/');
    if requested == canonical || state.is_allowed_resource_url(requested) {
        debug!(requested_resource_id = %fingerprint(requested), protected_resource = requested != canonical, "oauth resource accepted");
        return Ok(requested.to_string());
    }
    warn!(requested_resource_id = %fingerprint(requested), "oauth request rejected: resource does not match an allowed MCP endpoint");
    Err(AuthError::Validation(format!(
        "resource must be `{canonical}` or a configured protected MCP route"
    )))
}
