//! MCP bearer-token generation for first-run setup.
//!
//! Produces the same hex shape as `just mcp-token` (`openssl rand -hex 32`):
//! 32 random bytes → 64 lowercase hex chars. `doctor` validates length >= 32.

/// Generate a fresh 64-char hex MCP bearer token from 32 OS-random bytes.
///
/// Fail-closed by design: if the OS RNG is unavailable we `panic` rather than
/// emit a weak/predictable token for a security primitive.
#[must_use]
pub fn generate_mcp_token() -> String {
    let mut buf = [0_u8; 32];
    getrandom::fill(&mut buf).expect("OS RNG unavailable while generating MCP token");
    hex::encode(buf)
}

#[cfg(test)]
mod tests {
    use super::generate_mcp_token;

    #[test]
    fn token_is_64_hex_chars() {
        let t = generate_mcp_token();
        assert_eq!(t.len(), 64, "token must be 64 hex chars (32 bytes)");
        assert!(t.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn tokens_are_unique() {
        assert_ne!(generate_mcp_token(), generate_mcp_token());
    }
}
