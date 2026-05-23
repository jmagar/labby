//! Authentication primitives.
//!
//! Every service uses one of `ApiKey`, `Token`, `Bearer`, `Basic`, or `Session`
//! auth. `HttpClient` injects the right header (or cookie) from the [`Auth`]
//! enum at request time.

use std::fmt;

/// How a service authenticates outbound requests.
#[derive(Clone)]
pub enum Auth {
    /// No authentication.
    None,
    /// `<header>: <key>` style. Header name is configurable because Servarr
    /// uses `X-Api-Key` while Tautulli passes the key as a query param and
    /// Linkding uses `Authorization`.
    ApiKey {
        /// Header name (e.g. `"X-Api-Key"`).
        header: String,
        /// Secret value.
        key: String,
    },
    /// `Authorization: Token <token>` (Linkding, ...).
    Token {
        /// Token value.
        token: String,
    },
    /// `Authorization: Bearer <token>` (`Memos`, `ByteStash`, ...).
    Bearer {
        /// Bearer token value.
        token: String,
    },
    /// HTTP Basic auth (qBittorrent pre-session, etc.).
    Basic {
        /// Username.
        username: String,
        /// Password.
        password: String,
    },
    /// Cookie-based session (qBittorrent post-login).
    Session {
        /// Cookie header value to attach to every request.
        cookie: String,
    },
}

impl fmt::Debug for Auth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => f.write_str("Auth::None"),
            Self::ApiKey { header, .. } => {
                write!(f, "Auth::ApiKey {{ header: {header}, key: <redacted> }}")
            }
            Self::Token { .. } => f.write_str("Auth::Token { token: <redacted> }"),
            Self::Bearer { .. } => f.write_str("Auth::Bearer { token: <redacted> }"),
            Self::Basic { username, .. } => {
                write!(
                    f,
                    "Auth::Basic {{ username: {username}, password: <redacted> }}"
                )
            }
            Self::Session { .. } => f.write_str("Auth::Session { cookie: <redacted> }"),
        }
    }
}
