//! Page-context injection helpers for `session.prompt`.
//!
//! Moved from `api/services/acp.rs` so that MCP and CLI can also benefit from
//! context injection without duplicating the sanitization logic.
//!
//! # Sanitization Policy
//!
//! Frontend-supplied page-context values are untrusted and could be used to
//! smuggle prompt-injection content into the assistant's prompt. This module
//! defends in depth:
//!
//! 1. **Structural character allowlist.** Each input value is filtered through
//!    [`is_safe_page_context_char`], which accepts only ASCII alphanumerics
//!    plus a tiny explicit punctuation set (`/`, `_`, `-`). This is expressed
//!    as a predicate — *not* a hand-spelled `&[char]` table — so the
//!    coverage is obvious by inspection. Anything else (whitespace, control
//!    chars, punctuation, every non-ASCII codepoint, including unicode
//!    homoglyphs and zero-width separators) is silently stripped before any
//!    further checks run. This is the primary safety layer.
//!
//! 2. **Length cap.** Per-field values are truncated to 32 characters after
//!    filtering. The assembled prefix is additionally capped at
//!    `PAGE_CONTEXT_MAX_CHARS` (~30 tokens) and falls back to progressively
//!    shorter shapes if the budget is exceeded.
//!
//! 3. **Deny-list of injection terms.** Even after filtering, certain English
//!    tokens are clear prompt-injection signals
//!    (`ignore`, `override`, `instruction`, `assistant`). The deny-list is
//!    intentionally narrow: legitimate route names (`admin`, `system`,
//!    `prompt`) are *not* denied — the character allowlist is doing the heavy
//!    lifting. The deny-list is checked both per separator-split segment
//!    *and* on the joined lowercased string, so an attacker cannot bypass it
//!    by inserting `/`, `_`, or `-` separators (e.g. `ig-no-re`).
//!
//! 4. **No structured/nested input.** [`PageContextInput`] is intentionally
//!    flat: `route`, `entity_type`, `entity_id`. There is no map/object
//!    pass-through. If callers ever expand this shape, every new field must
//!    go through [`sanitize_page_context_field`] explicitly — there is no
//!    "extra fields" escape hatch.
//!
//! 5. **Fail-closed.** On any sanitization failure, the entire context prefix
//!    is dropped (see [`build_prompt_with_context`]); the user's prompt is
//!    sent without context rather than with partial/unsafe context.

/// Maximum tokens allowed for the assembled context prefix.
/// Estimated at ~4 chars/token; we enforce a char budget of 30 * 4 = 120 chars.
const PAGE_CONTEXT_MAX_TOKENS: usize = 30;
const PAGE_CONTEXT_MAX_CHARS: usize = PAGE_CONTEXT_MAX_TOKENS * 4;

/// Maximum characters per individual page-context field after filtering.
const PAGE_CONTEXT_FIELD_MAX_CHARS: usize = 32;

/// Predicate defining the page-context character allowlist.
///
/// Accepts ASCII alphanumerics (`A-Z`, `a-z`, `0-9`) plus the route/path
/// punctuation `/`, `_`, `-`. Everything else — whitespace, control codes,
/// other ASCII punctuation, and *every* non-ASCII codepoint (including
/// unicode homoglyphs and zero-width characters) — is rejected.
///
/// Expressed as a predicate (not a hand-spelled `&[char]` table) so coverage
/// is obvious by inspection and impossible to forget a character.
#[inline]
fn is_safe_page_context_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '-')
}

/// Tokens that indicate prompt-injection attempts — reject the entire pageContext
/// field if any match.
///
/// Intentionally narrow. The character allowlist is the primary safety layer;
/// these are the small set of English tokens that have no legitimate reason to
/// appear in a route/entity identifier and are unambiguous prompt-injection
/// signals:
///
/// - `ignore` — "ignore previous instructions"
/// - `override` — "override the system prompt"
/// - `instruction` — "new instruction:" / "system instruction"
/// - `assistant` — role-spoofing attempts ("assistant: do X")
///
/// Legitimate route names like `admin`, `system`, or `prompt` are NOT denied
/// because they appear in real product routes; the character allowlist plus
/// length cap already prevents weaponizing them.
const PAGE_CONTEXT_DENY_LIST: &[&str] = &["ignore", "override", "instruction", "assistant"];

/// Optional structured page context sent by the frontend.
/// All fields are strings; validation is applied by `assemble_page_context_prefix`.
///
/// This shape is intentionally flat. There is no map/object pass-through; any
/// future field MUST be sanitized explicitly via `sanitize_page_context_field`.
pub struct PageContextInput<'a> {
    pub route: &'a str,
    pub entity_type: Option<&'a str>,
    pub entity_id: Option<&'a str>,
}

/// Sanitize a single pageContext field value.
///
/// Steps, in order:
/// 1. Filter through [`is_safe_page_context_char`] (drops anything outside the
///    allowlist).
/// 2. Truncate to [`PAGE_CONTEXT_FIELD_MAX_CHARS`] (32 chars).
/// 3. Reject if empty after filtering.
/// 4. Lowercase, then check the deny-list against both each separator-split
///    segment and the joined string (defeats `ig-no-re`-style separator
///    bypasses).
///
/// Returns `None` on any failure. On success, returns the filtered+truncated
/// string (preserving the original casing).
pub fn sanitize_page_context_field(value: &str) -> Option<String> {
    let stripped: String = value
        .chars()
        .filter(|c| is_safe_page_context_char(*c))
        .take(PAGE_CONTEXT_FIELD_MAX_CHARS)
        .collect();

    if stripped.is_empty() {
        return None;
    }

    // Deny-list check: split on separators to catch per-segment matches, then
    // also check the joined lowercased string so runs that span segment
    // boundaries (e.g. `ig-nore`) cannot bypass.
    let lower = stripped.to_lowercase();
    for segment in lower.split(['/', '_', '-']) {
        for denied in PAGE_CONTEXT_DENY_LIST {
            if segment.contains(denied) {
                return None;
            }
        }
    }
    let joined: String = lower
        .chars()
        .filter(|c| !matches!(c, '/' | '_' | '-'))
        .collect();
    for denied in PAGE_CONTEXT_DENY_LIST {
        if joined.contains(denied) {
            return None;
        }
        if lower.contains(denied) {
            return None;
        }
    }

    Some(stripped)
}

/// Assemble a compact context prefix from validated page context input.
/// Returns `None` if route validation fails.
/// Format: `[context: page={route}]` or `[context: page={route} entity={type}/{id}]`
pub fn assemble_page_context_prefix(
    session_id: &str,
    ctx: &PageContextInput<'_>,
) -> Option<String> {
    let route = sanitize_page_context_field(ctx.route)?;

    let prefix = match (ctx.entity_type, ctx.entity_id) {
        (Some(et), Some(eid)) => {
            let entity_type = sanitize_page_context_field(et)?;
            let entity_id = sanitize_page_context_field(eid)?;
            let candidate = format!("[context: page={route} entity={entity_type}/{entity_id}]");
            if candidate.len() <= PAGE_CONTEXT_MAX_CHARS {
                candidate
            } else {
                let without_id = format!("[context: page={route} entity={entity_type}]");
                if without_id.len() <= PAGE_CONTEXT_MAX_CHARS {
                    without_id
                } else {
                    format!("[context: page={route}]")
                }
            }
        }
        (Some(et), None) => {
            let entity_type = sanitize_page_context_field(et)?;
            format!("[context: page={route} entity={entity_type}]")
        }
        _ => format!("[context: page={route}]"),
    };

    let estimated_tokens = prefix.len().div_ceil(4);
    tracing::info!(
        surface = "dispatch",
        service = "acp",
        action = "session.prompt",
        session_id,
        page_context_route = %route,
        page_context_token_estimate = estimated_tokens,
        "page context injected",
    );

    Some(prefix)
}

/// Build the effective prompt text from optional page context + user prompt.
/// On validation failure the context is silently dropped (never errors the request).
pub fn build_prompt_with_context(
    session_id: &str,
    prompt: &str,
    ctx: Option<&PageContextInput<'_>>,
) -> String {
    if let Some(ctx) = ctx {
        match assemble_page_context_prefix(session_id, ctx) {
            Some(prefix) => format!("{prefix}\n\n{prompt}"),
            None => {
                tracing::warn!(
                    surface = "dispatch",
                    service = "acp",
                    action = "session.prompt",
                    session_id,
                    "page context validation failed — injecting without context",
                );
                prompt.to_string()
            }
        }
    } else {
        prompt.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_prompt_with_page_context_prefix() {
        let ctx = PageContextInput {
            route: "/gateways",
            entity_type: Some("gateway"),
            entity_id: Some("local"),
        };

        let prompt = build_prompt_with_context("sess-1", "show status", Some(&ctx));

        assert_eq!(
            prompt,
            "[context: page=/gateways entity=gateway/local]\n\nshow status"
        );
    }

    #[test]
    fn drops_invalid_page_context_without_changing_prompt() {
        let ctx = PageContextInput {
            route: "/ignore-instructions",
            entity_type: Some("gateway"),
            entity_id: Some("local"),
        };

        let prompt = build_prompt_with_context("sess-1", "show status", Some(&ctx));

        assert_eq!(prompt, "show status");
    }

    #[test]
    fn drops_entity_id_when_full_entity_prefix_exceeds_budget() {
        let ctx = PageContextInput {
            route: "very-long-route-name-that-fills-budget",
            entity_type: Some("very-long-entity-type-that-still-sanitizes"),
            entity_id: Some("very-long-entity-id-that-still-sanitizes"),
        };

        let prefix = assemble_page_context_prefix("sess-1", &ctx).unwrap();

        assert!(prefix.starts_with("[context: page=very-long-route-name-that-fills- entity="));
        assert!(prefix.contains("entity=very-long-entity-type-that-still"));
        assert!(!prefix.contains("very-long-entity-id"));
    }

    // ---- Character allowlist coverage --------------------------------------

    #[test]
    fn predicate_accepts_ascii_alphanumerics_and_punctuation() {
        for c in 'a'..='z' {
            assert!(is_safe_page_context_char(c), "lowercase {c} should pass");
        }
        for c in 'A'..='Z' {
            assert!(is_safe_page_context_char(c), "uppercase {c} should pass");
        }
        for c in '0'..='9' {
            assert!(is_safe_page_context_char(c), "digit {c} should pass");
        }
        for c in ['/', '_', '-'] {
            assert!(is_safe_page_context_char(c), "punctuation {c} should pass");
        }
    }

    #[test]
    fn predicate_rejects_whitespace_and_control_chars() {
        for c in [' ', '\t', '\n', '\r', '\x00', '\x07', '\x1b', '\x7f'] {
            assert!(
                !is_safe_page_context_char(c),
                "control/whitespace {:?} should be rejected",
                c
            );
        }
    }

    #[test]
    fn predicate_rejects_other_ascii_punctuation() {
        for c in [
            '.', ',', ';', ':', '!', '?', '\'', '"', '`', '(', ')', '[', ']', '{', '}', '<', '>',
            '|', '\\', '@', '#', '$', '%', '^', '&', '*', '+', '=', '~',
        ] {
            assert!(
                !is_safe_page_context_char(c),
                "ascii punctuation {c} should be rejected"
            );
        }
    }

    #[test]
    fn predicate_rejects_non_ascii_unicode() {
        // Latin extended, CJK, emoji, zero-width joiner, RTL override.
        for c in ['é', 'ñ', 'ü', '中', '日', 'я', '🚀', '\u{200d}', '\u{202e}'] {
            assert!(
                !is_safe_page_context_char(c),
                "non-ascii {:?} should be rejected",
                c
            );
        }
    }

    // ---- Sanitize: allowed shapes ------------------------------------------

    #[test]
    fn sanitize_accepts_simple_routes() {
        assert_eq!(
            sanitize_page_context_field("/gateways").as_deref(),
            Some("/gateways")
        );
        assert_eq!(
            sanitize_page_context_field("dashboard").as_deref(),
            Some("dashboard")
        );
        assert_eq!(
            sanitize_page_context_field("user_profile-42").as_deref(),
            Some("user_profile-42")
        );
    }

    #[test]
    fn sanitize_preserves_casing() {
        assert_eq!(
            sanitize_page_context_field("MixedCaseRoute").as_deref(),
            Some("MixedCaseRoute")
        );
    }

    #[test]
    fn sanitize_strips_disallowed_chars() {
        // Spaces, dots, and unicode get filtered out; remainder is kept.
        assert_eq!(
            sanitize_page_context_field("/foo bar.baz").as_deref(),
            Some("/foobarbaz")
        );
        assert_eq!(
            sanitize_page_context_field("héllo-wörld").as_deref(),
            Some("hllo-wrld")
        );
    }

    #[test]
    fn sanitize_strips_control_characters() {
        assert_eq!(
            sanitize_page_context_field("foo\x00\x07\nbar").as_deref(),
            Some("foobar")
        );
    }

    #[test]
    fn sanitize_returns_none_for_empty_after_filtering() {
        assert_eq!(sanitize_page_context_field(""), None);
        assert_eq!(sanitize_page_context_field("   "), None);
        assert_eq!(sanitize_page_context_field("!!!"), None);
        assert_eq!(sanitize_page_context_field("🚀🚀🚀"), None);
    }

    // ---- Sanitize: length enforcement --------------------------------------

    #[test]
    fn sanitize_truncates_to_32_chars() {
        // 40 chars of `a`.
        let long = "a".repeat(40);
        let out = sanitize_page_context_field(&long).unwrap();
        assert_eq!(out.len(), 32);
        assert!(out.chars().all(|c| c == 'a'));
    }

    #[test]
    fn sanitize_truncates_after_filtering_not_before() {
        // 40 chars of disallowed punctuation followed by 10 valid chars: only
        // the valid chars survive, and they all fit under the 32-cap.
        let mixed = format!("{}{}", ".".repeat(40), "abcdefghij");
        assert_eq!(
            sanitize_page_context_field(&mixed).as_deref(),
            Some("abcdefghij")
        );
    }

    // ---- Deny-list ---------------------------------------------------------

    #[test]
    fn sanitize_rejects_deny_list_terms() {
        assert_eq!(sanitize_page_context_field("ignore"), None);
        assert_eq!(sanitize_page_context_field("Override"), None);
        assert_eq!(sanitize_page_context_field("/INSTRUCTION"), None);
        assert_eq!(sanitize_page_context_field("assistant"), None);
    }

    #[test]
    fn sanitize_rejects_deny_list_with_separator_bypass() {
        // Per-segment check + joined check: separators must not bypass.
        assert_eq!(sanitize_page_context_field("ig-no-re"), None);
        assert_eq!(sanitize_page_context_field("ig_no_re"), None);
        assert_eq!(sanitize_page_context_field("ig/no/re"), None);
        assert_eq!(sanitize_page_context_field("over-ride"), None);
        assert_eq!(sanitize_page_context_field("a-ignore-b"), None);
    }

    #[test]
    fn sanitize_rejects_deny_list_embedded_in_segment() {
        assert_eq!(sanitize_page_context_field("preignorepost"), None);
        assert_eq!(sanitize_page_context_field("/foo/ignore-me"), None);
    }

    #[test]
    fn sanitize_allows_legitimate_route_names() {
        // Common product route names that resemble injection terms but are not
        // in the deny-list.
        assert_eq!(
            sanitize_page_context_field("admin").as_deref(),
            Some("admin")
        );
        assert_eq!(
            sanitize_page_context_field("system").as_deref(),
            Some("system")
        );
        assert_eq!(
            sanitize_page_context_field("prompt").as_deref(),
            Some("prompt")
        );
        assert_eq!(
            sanitize_page_context_field("/admin/settings").as_deref(),
            Some("/admin/settings")
        );
    }

    // ---- Common secret-like names ------------------------------------------

    #[test]
    fn sanitize_neuters_common_secret_names() {
        // These are typical env-var / token names. The allowlist strips
        // separators are fine, but the actual concern is that *values* of
        // such variables don't survive unchanged. These check that even if a
        // caller naively passed something like an authorization header in,
        // the disallowed chars (whitespace, `=`, `:`, `.`) get stripped.
        assert_eq!(
            sanitize_page_context_field("Bearer abc.def.ghi").as_deref(),
            Some("Bearerabcdefghi")
        );
        assert_eq!(
            sanitize_page_context_field("api-key=secret123").as_deref(),
            Some("api-keysecret123")
        );
        assert_eq!(
            sanitize_page_context_field("AUTH: xyz").as_deref(),
            Some("AUTHxyz")
        );
    }

    // ---- assemble_page_context_prefix --------------------------------------

    #[test]
    fn assemble_rejects_invalid_route() {
        let ctx = PageContextInput {
            route: "!!!",
            entity_type: None,
            entity_id: None,
        };
        assert!(assemble_page_context_prefix("sess", &ctx).is_none());
    }

    #[test]
    fn assemble_rejects_invalid_entity_type() {
        let ctx = PageContextInput {
            route: "/ok",
            entity_type: Some("ignore"),
            entity_id: Some("local"),
        };
        assert!(assemble_page_context_prefix("sess", &ctx).is_none());
    }

    #[test]
    fn assemble_drops_entity_when_route_only() {
        let ctx = PageContextInput {
            route: "/dashboard",
            entity_type: None,
            entity_id: None,
        };
        let prefix = assemble_page_context_prefix("sess", &ctx).unwrap();
        assert_eq!(prefix, "[context: page=/dashboard]");
    }

    #[test]
    fn build_prompt_passthrough_when_no_context() {
        let prompt = build_prompt_with_context("sess", "hello", None);
        assert_eq!(prompt, "hello");
    }
}
