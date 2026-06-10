use super::{
    probe::{probe_manager_key, validate_probe_upstream_name, validate_probe_url},
    should_use_dynamic_registration,
};

#[test]
fn validate_probe_url_rejects_userinfo() {
    let result = validate_probe_url("https://user:pass@example.com/mcp");
    assert!(result.is_err(), "expected error for URL with userinfo");
}

#[test]
fn validate_probe_url_rejects_query_and_fragment() {
    let with_query = validate_probe_url("https://example.com/mcp?foo=bar");
    assert!(
        with_query.is_err(),
        "expected error for URL with query string"
    );
    let with_fragment = validate_probe_url("https://example.com/mcp#section");
    assert!(
        with_fragment.is_err(),
        "expected error for URL with fragment"
    );
}

#[test]
fn probe_manager_key_includes_port_and_path() {
    let url = url::Url::parse("https://example.com:9000/mcp").unwrap();
    let key = probe_manager_key(&url);
    assert!(
        key.contains("example.com"),
        "key should contain hostname: {key}"
    );
    assert!(key.contains("9000"), "key should contain port: {key}");
    assert!(
        key.contains("mcp"),
        "key should contain path segment: {key}"
    );
}

#[test]
fn probe_manager_key_distinguishes_colliding_paths() {
    let url_a = url::Url::parse("https://example.com/mcp/a").unwrap();
    let url_b = url::Url::parse("https://example.com/mcp/b").unwrap();
    let key_a = probe_manager_key(&url_a);
    let key_b = probe_manager_key(&url_b);
    assert_ne!(
        key_a, key_b,
        "different paths should produce different keys"
    );
}

#[test]
fn validate_probe_upstream_name_rejects_path_like_values() {
    let with_slash = validate_probe_upstream_name("my/server");
    assert!(
        with_slash.is_err(),
        "expected error for name containing '/'"
    );
    let with_backslash = validate_probe_upstream_name("my\\server");
    assert!(
        with_backslash.is_err(),
        "expected error for name containing '\\'"
    );
    let empty = validate_probe_upstream_name("  ");
    assert!(empty.is_err(), "expected error for whitespace-only name");
}

// ── should_use_dynamic_registration coverage ─────────────────────────────────

#[test]
fn swag_uses_client_metadata_document_even_when_dynamic_registration_is_advertised() {
    // Legacy default: "swag" always uses CIMD regardless of what the server supports.
    assert!(
        !should_use_dynamic_registration("swag", true, None),
        "swag + supports_dynamic + no override → should NOT use dynamic"
    );
    // Other upstreams that support dynamic registration should use it.
    assert!(
        should_use_dynamic_registration("github", true, None),
        "github + supports_dynamic + no override → should use dynamic"
    );
    // No supports_dynamic → always false regardless of upstream name.
    assert!(
        !should_use_dynamic_registration("github", false, None),
        "no dynamic support → should NOT use dynamic"
    );
}

#[test]
fn prefer_client_metadata_document_true_overrides_dynamic_registration() {
    // When the operator explicitly sets prefer_client_metadata_document = true,
    // dynamic registration is suppressed even when the server supports it.
    assert!(
        !should_use_dynamic_registration("github", true, Some(true)),
        "explicit prefer_cimd=true + supports_dynamic → should NOT use dynamic"
    );
    assert!(
        !should_use_dynamic_registration("github", false, Some(true)),
        "explicit prefer_cimd=true + no support → should NOT use dynamic"
    );
}

#[test]
fn prefer_client_metadata_document_false_opts_in_to_dynamic_registration() {
    // When the operator explicitly sets prefer_client_metadata_document = false,
    // dynamic registration is used even for "swag".
    assert!(
        should_use_dynamic_registration("swag", true, Some(false)),
        "explicit prefer_cimd=false + supports_dynamic → should use dynamic"
    );
    // No dynamic support → still false (hardware constraint, not a preference).
    assert!(
        !should_use_dynamic_registration("swag", false, Some(false)),
        "explicit prefer_cimd=false + no support → should NOT use dynamic"
    );
}
