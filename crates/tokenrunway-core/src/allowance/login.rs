//! The in-app "Log in" method (SPEC.md §8.1, method "login"): which page the login window opens
//! for a provider and which cookie(s) it waits for. The window itself is in the app
//! (src-tauri/src/login.rs); this half has no Tauri in it so it can be tested.
//!
//! Only the named cookies are kept, as one Cookie header value ("name=value; name2=value2"), in
//! the OS keychain under `secrets::session_name`. Nothing else from the session is read.

/// What the login window does for one provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoginSpec {
    /// The page the window opens; it sends the user on to the provider's own sign-in.
    pub start_url: &'static str,
    /// The site whose cookies are read ("cursor.com" also matches ".cursor.com" and
    /// "www.cursor.com").
    pub host: &'static str,
    /// The cookie that means "signed in"; the window closes once it's there.
    pub required: &'static str,
    /// Cookies kept with it when present.
    pub optional: &'static [&'static str],
}

/// The login window for a provider, if it has one.
pub fn spec(provider: &str) -> Option<LoginSpec> {
    match provider {
        "cursor" => Some(LoginSpec {
            start_url: "https://cursor.com/dashboard",
            host: "cursor.com",
            required: "WorkosCursorSessionToken",
            optional: &[],
        }),
        // Unverified: grok.com's session cookies as seen in the browser (`sso`, `sso-rw`).
        "xai" => Some(LoginSpec { start_url: "https://grok.com/", host: "grok.com", required: "sso", optional: &["sso-rw"] }),
        _ => None,
    }
}

/// True if a cookie set for `domain` is sent to `host` ("" and leading dots allowed).
pub fn domain_matches(domain: &str, host: &str) -> bool {
    let d = domain.trim_start_matches('.').to_ascii_lowercase();
    !d.is_empty() && (d == host || host.ends_with(&format!(".{d}")) || d.ends_with(&format!(".{host}")))
}

/// The Cookie header value to keep, once the required cookie is there. `cookies` are
/// (domain, name, value). `previous` is the value already in the keychain: a re-login waits for
/// a new session rather than taking the old (refused) one again.
pub fn session_from(spec: &LoginSpec, cookies: &[(String, String, String)], previous: Option<&str>) -> Option<String> {
    let find = |name: &str| {
        cookies.iter().find(|(d, n, v)| n == name && !v.is_empty() && domain_matches(d, spec.host)).map(|(_, n, v)| format!("{n}={v}"))
    };
    let mut parts = vec![find(spec.required)?];
    parts.extend(spec.optional.iter().filter_map(|n| find(n)));
    let header = parts.join("; ");
    (previous != Some(header.as_str())).then_some(header)
}

/// A stable 16-byte id for an account's own web data store (macOS 14+ `WKWebsiteDataStore`
/// identifiers), so each account's login session is kept apart from the others and from the
/// app's own windows. Shaped as a version-4 UUID.
pub fn store_id(account_id: &str) -> [u8; 16] {
    fn fnv(seed: u64, s: &str) -> u64 {
        s.bytes().fold(seed, |h, b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3))
    }
    let a = fnv(0xcbf2_9ce4_8422_2325, account_id);
    let b = fnv(a ^ 0x9e37_79b9_7f4a_7c15, &format!("tokenrunway-login:{account_id}"));
    let mut out = [0u8; 16];
    out[..8].copy_from_slice(&a.to_be_bytes());
    out[8..].copy_from_slice(&b.to_be_bytes());
    out[6] = (out[6] & 0x0f) | 0x40;
    out[8] = (out[8] & 0x3f) | 0x80;
    out
}

/// The value of one cookie in a Cookie header value ("a=1; b=2").
pub fn cookie_value<'a>(header: &'a str, name: &str) -> Option<&'a str> {
    header.split(';').filter_map(|p| p.trim().split_once('=')).find(|(n, _)| n.trim() == name).map(|(_, v)| v.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(d: &str, n: &str, v: &str) -> (String, String, String) {
        (d.into(), n.into(), v.into())
    }

    #[test]
    fn waits_for_the_session_cookie() {
        let s = spec("cursor").unwrap();
        assert_eq!(session_from(&s, &[c(".cursor.com", "other", "1")], None), None);
        let jar = [c("cursor.com", "WorkosCursorSessionToken", "user_1%3A%3Aeyx"), c("example.com", "WorkosCursorSessionToken", "nope")];
        assert_eq!(session_from(&s, &jar, None).as_deref(), Some("WorkosCursorSessionToken=user_1%3A%3Aeyx"));
        assert_eq!(session_from(&s, &jar, Some("WorkosCursorSessionToken=user_1%3A%3Aeyx")), None, "not the old session again");
        let g = spec("xai").unwrap();
        let jar = [c(".grok.com", "sso-rw", "b"), c(".grok.com", "sso", "a")];
        assert_eq!(session_from(&g, &jar, None).as_deref(), Some("sso=a; sso-rw=b"));
        assert_eq!(session_from(&g, &[c(".grok.com", "sso-rw", "b")], None), None, "sso is required");
        assert!(spec("anthropic").is_none());
    }

    #[test]
    fn domains() {
        assert!(domain_matches(".cursor.com", "cursor.com"));
        assert!(domain_matches("www.cursor.com", "cursor.com"));
        assert!(!domain_matches("notcursor.com", "cursor.com"));
        assert!(!domain_matches("", "cursor.com"));
    }

    #[test]
    fn store_ids_are_stable_and_distinct() {
        assert_eq!(store_id("acct-1"), store_id("acct-1"));
        assert_ne!(store_id("acct-1"), store_id("acct-2"));
        assert_eq!(store_id("acct-1")[6] >> 4, 4);
    }

    #[test]
    fn cookie_values() {
        assert_eq!(cookie_value("sso=a; sso-rw=b", "sso-rw"), Some("b"));
        assert_eq!(cookie_value("sso=a", "x"), None);
    }
}
