//! What the side browser does with what's typed in its address bar.

use regex::Regex;

pub const HOME_URL: &str = "https://www.bing.com";

fn encode_component(text: &str) -> String {
    let mut out = String::new();
    for b in text.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// A full address stays as it is, a host name gets a scheme, anything else is a search.
pub fn normalize_url(text: &str) -> Option<String> {
    let s = text.trim();
    if s.is_empty() {
        return None;
    }
    let is = |pattern: &str| Regex::new(pattern).is_ok_and(|r| r.is_match(s));
    if is(r"(?i)^[a-z][a-z0-9+.-]*://") || is(r"(?i)^(about|data|file):") {
        return Some(s.to_string());
    }
    if is(r"(?i)^(localhost|127\.0\.0\.1|\[::1\])(:\d+)?(/|$)") {
        return Some(format!("http://{s}"));
    }
    if !s.chars().any(char::is_whitespace) && is(r"(?i)^[^/]+\.[a-z]{2,}(:\d+)?(/.*)?$") {
        return Some(format!("https://{s}"));
    }
    Some(format!("https://www.bing.com/search?q={}", encode_component(s)))
}

/// The name a tab shows until the page has a title.
pub fn host_of(url: &str) -> String {
    let Some((_, rest)) = url.split_once("://") else { return url.to_string() };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let host = authority.rsplit('@').next().unwrap_or(authority);
    let host = if host.starts_with('[') { host } else { host.split(':').next().unwrap_or(host) };
    if host.is_empty() {
        return url.to_string();
    }
    host.strip_prefix("www.").unwrap_or(host).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_and_searches() {
        assert_eq!(normalize_url("  "), None);
        assert_eq!(normalize_url("https://a.io/x").unwrap(), "https://a.io/x");
        assert_eq!(normalize_url("about:blank").unwrap(), "about:blank");
        assert_eq!(normalize_url("localhost:3000/a").unwrap(), "http://localhost:3000/a");
        assert_eq!(normalize_url("127.0.0.1").unwrap(), "http://127.0.0.1");
        assert_eq!(normalize_url("example.com/path").unwrap(), "https://example.com/path");
        assert_eq!(normalize_url("rust gpui").unwrap(), "https://www.bing.com/search?q=rust%20gpui");
        assert_eq!(normalize_url("中文").unwrap(), "https://www.bing.com/search?q=%E4%B8%AD%E6%96%87");
    }

    #[test]
    fn tab_names() {
        assert_eq!(host_of("https://www.bing.com/search?q=1"), "bing.com");
        assert_eq!(host_of("http://u:p@localhost:3000/"), "localhost");
        assert_eq!(host_of("about:blank"), "about:blank");
    }
}
