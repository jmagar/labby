//! Target resolution and forwarding helpers for the local OAuth relay.

use std::collections::{BTreeMap, BTreeSet};

use axum::http::{HeaderMap, HeaderName, HeaderValue};
use url::Url;

use crate::config::OauthMachineConfig;
use crate::oauth::error::OauthRelayError;

/// A resolved forwarding target.
#[derive(Debug, Clone)]
pub struct ResolvedTarget {
    pub machine_id: Option<String>,
    pub target_url: Url,
    pub default_port: Option<u16>,
}

/// Resolve a named machine target from config.
pub fn resolve_machine_target(
    machines: &BTreeMap<String, OauthMachineConfig>,
    machine_id: &str,
) -> Result<ResolvedTarget, OauthRelayError> {
    let config = machines
        .get(machine_id)
        .ok_or_else(|| OauthRelayError::UnknownMachine {
            machine_id: machine_id.to_string(),
            available: machines.keys().cloned().collect::<Vec<_>>().join(", "),
        })?;
    let target_url =
        Url::parse(&config.target_url).map_err(|source| OauthRelayError::InvalidTargetUrl {
            value: config.target_url.clone(),
            source,
        })?;

    Ok(ResolvedTarget {
        machine_id: Some(machine_id.to_string()),
        target_url,
        default_port: config.default_port,
    })
}

/// Resolve an ad hoc forwarding target from a CLI-supplied base URL.
pub fn resolve_explicit_target(
    target_url: &str,
    default_port: Option<u16>,
) -> Result<ResolvedTarget, OauthRelayError> {
    let target_url =
        Url::parse(target_url).map_err(|source| OauthRelayError::InvalidTargetUrl {
            value: target_url.to_string(),
            source,
        })?;

    Ok(ResolvedTarget {
        machine_id: None,
        target_url,
        default_port,
    })
}

/// Construct a forwarding URL by appending the incoming suffix path and query.
pub fn build_forward_url(
    target_base: &Url,
    suffix_path: &str,
    query_items: &[(&str, &str)],
) -> Result<Url, OauthRelayError> {
    let mut url = target_base.clone();
    let suffix_path = suffix_path.trim_matches('/');
    if !suffix_path.is_empty() {
        let mut path = url.path().trim_end_matches('/').to_string();
        if path.is_empty() {
            path.push('/');
        }
        if !path.ends_with('/') {
            path.push('/');
        }
        path.push_str(suffix_path);
        url.set_path(&path);
    }

    let mut merged_query = url
        .query_pairs()
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect::<Vec<_>>();
    merged_query.extend(
        query_items
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string())),
    );

    {
        let mut serializer = url.query_pairs_mut();
        serializer.clear();
        for (key, value) in merged_query {
            serializer.append_pair(&key, &value);
        }
    }

    Ok(url)
}

/// Filter hop-by-hop headers from an inbound request before forwarding.
pub fn filter_hop_by_hop_request_headers(headers: &HeaderMap) -> HeaderMap {
    filter_headers(headers, REQUEST_HEADER_ALLOWLIST)
}

/// Filter hop-by-hop headers from an upstream response before returning it.
pub fn filter_hop_by_hop_response_headers(headers: &HeaderMap) -> HeaderMap {
    filter_headers(headers, RESPONSE_HEADER_ALLOWLIST)
}

const REQUEST_HEADER_ALLOWLIST: &[&str] = &[
    "accept",
    "accept-language",
    "content-type",
    "origin",
    "referer",
    "user-agent",
];

const RESPONSE_HEADER_ALLOWLIST: &[&str] = &[
    "cache-control",
    "content-language",
    "content-type",
    "expires",
    "location",
    "pragma",
];

fn filter_headers(headers: &HeaderMap, allowlist: &[&str]) -> HeaderMap {
    let connection_header_names = connection_header_names(headers);
    headers
        .iter()
        .filter(|(name, _)| {
            allowlist.contains(&name.as_str())
                && !is_hop_by_hop_header(name)
                && !connection_header_names.contains(name.as_str())
        })
        .fold(HeaderMap::new(), |mut filtered, (name, value)| {
            filtered.append(name.clone(), copy_header_value(value));
            filtered
        })
}

fn copy_header_value(value: &HeaderValue) -> HeaderValue {
    value.clone()
}

fn connection_header_names(headers: &HeaderMap) -> BTreeSet<String> {
    headers
        .get_all("connection")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase())
        .collect()
}

fn is_hop_by_hop_header(name: &HeaderName) -> bool {
    matches!(
        name.as_str(),
        "connection"
            | "content-length"
            | "host"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::header;
    use axum::http::header::{
        AUTHORIZATION, CONNECTION, CONTENT_LENGTH, CONTENT_TYPE, COOKIE, HOST, ORIGIN, REFERER,
        SET_COOKIE, TRANSFER_ENCODING, USER_AGENT,
    };

    #[test]
    fn resolve_machine_target_returns_configured_machine() {
        let machines = BTreeMap::from([(
            "dookie".to_string(),
            OauthMachineConfig {
                target_url: "http://100.88.16.79:38935/callback/dookie".to_string(),
                description: None,
                default_port: Some(38935),
            },
        )]);

        let resolved = resolve_machine_target(&machines, "dookie").expect("machine should resolve");
        assert_eq!(resolved.machine_id.as_deref(), Some("dookie"));
        assert_eq!(
            resolved.target_url.as_str(),
            "http://100.88.16.79:38935/callback/dookie"
        );
        assert_eq!(resolved.default_port, Some(38935));
    }

    #[test]
    fn resolve_machine_target_lists_available_machine_ids() {
        let machines = BTreeMap::from([
            (
                "dookie".to_string(),
                OauthMachineConfig {
                    target_url: "http://100.88.16.79:38935/callback/dookie".to_string(),
                    description: None,
                    default_port: Some(38935),
                },
            ),
            (
                "squirts".to_string(),
                OauthMachineConfig {
                    target_url: "http://127.0.0.1:38935/callback/squirts".to_string(),
                    description: None,
                    default_port: Some(38935),
                },
            ),
        ]);

        let error = resolve_machine_target(&machines, "missing").expect_err("lookup should fail");
        let message = error.to_string();
        assert!(message.contains("missing"));
        assert!(message.contains("dookie"));
        assert!(message.contains("squirts"));
    }

    #[test]
    fn build_forward_url_appends_suffix_path_and_query() {
        let url = build_forward_url(
            &Url::parse("http://100.88.16.79:38935/callback/dookie").unwrap(),
            "foo/bar",
            &[("code", "abc"), ("state", "xyz")],
        )
        .expect("url should build");

        assert_eq!(
            url.as_str(),
            "http://100.88.16.79:38935/callback/dookie/foo/bar?code=abc&state=xyz"
        );
    }

    #[test]
    fn build_forward_url_preserves_existing_query_values() {
        let url = build_forward_url(
            &Url::parse("http://target/callback?existing=1").unwrap(),
            "",
            &[("code", "abc")],
        )
        .expect("url should build");

        assert_eq!(url.as_str(), "http://target/callback?existing=1&code=abc");
    }

    #[test]
    fn hop_by_hop_request_headers_are_filtered() {
        let mut headers = HeaderMap::new();
        headers.insert(CONNECTION, HeaderValue::from_static("keep-alive"));
        headers.insert(HOST, HeaderValue::from_static("localhost"));
        headers.insert(TRANSFER_ENCODING, HeaderValue::from_static("chunked"));
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(AUTHORIZATION, HeaderValue::from_static("Bearer secret"));
        headers.insert(COOKIE, HeaderValue::from_static("lab_session=secret"));
        headers.insert(ORIGIN, HeaderValue::from_static("https://lab.example.com"));
        headers.insert(
            REFERER,
            HeaderValue::from_static("https://lab.example.com/auth"),
        );
        headers.insert(USER_AGENT, HeaderValue::from_static("lab-test"));

        let filtered = filter_hop_by_hop_request_headers(&headers);

        assert!(!filtered.contains_key(CONNECTION));
        assert!(!filtered.contains_key(HOST));
        assert!(!filtered.contains_key(TRANSFER_ENCODING));
        assert!(filtered.contains_key(CONTENT_TYPE));
        assert!(filtered.contains_key(ORIGIN));
        assert!(filtered.contains_key(REFERER));
        assert!(filtered.contains_key(USER_AGENT));
        assert!(!filtered.contains_key(AUTHORIZATION));
        assert!(!filtered.contains_key(COOKIE));
    }

    #[test]
    fn hop_by_hop_response_headers_are_filtered() {
        let mut headers = HeaderMap::new();
        headers.insert(CONNECTION, HeaderValue::from_static("close"));
        headers.insert(CONTENT_LENGTH, HeaderValue::from_static("123"));
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("text/plain"));
        headers.insert(SET_COOKIE, HeaderValue::from_static("oauth=secret"));
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        headers.insert(header::EXPIRES, HeaderValue::from_static("0"));
        headers.insert(
            header::LOCATION,
            HeaderValue::from_static("https://example.com"),
        );
        headers.insert(header::PRAGMA, HeaderValue::from_static("no-cache"));

        let filtered = filter_hop_by_hop_response_headers(&headers);

        assert!(!filtered.contains_key(CONNECTION));
        assert!(!filtered.contains_key(CONTENT_LENGTH));
        assert!(filtered.contains_key(CONTENT_TYPE));
        assert!(filtered.contains_key(header::CACHE_CONTROL));
        assert!(filtered.contains_key(header::EXPIRES));
        assert!(filtered.contains_key(header::LOCATION));
        assert!(filtered.contains_key(header::PRAGMA));
        assert!(!filtered.contains_key(SET_COOKIE));
    }

    #[test]
    fn headers_nominated_by_connection_are_filtered() {
        let mut headers = HeaderMap::new();
        headers.insert(
            CONNECTION,
            HeaderValue::from_static("x-lab-session, keep-alive"),
        );
        headers.insert("x-lab-session", HeaderValue::from_static("secret"));
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("text/plain"));

        let filtered = filter_hop_by_hop_request_headers(&headers);

        assert!(!filtered.contains_key(CONNECTION));
        assert!(!filtered.contains_key("x-lab-session"));
        assert!(filtered.contains_key(CONTENT_TYPE));
    }
}
