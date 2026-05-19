//! Cookie wire-format helpers. Jars are owned per-account by `accounts::cookie_jar`.
//!
//! These helpers convert the on-disk/in-memory `SerializedCookieJar` shape
//! into outbound `Cookie:` request headers, and merge inbound `Set-Cookie`
//! response headers back into the jar. We deliberately avoid pulling a
//! full cookie crate — the Codex flow only needs path/secure scoping and a
//! handful of attributes.

use chrono::{DateTime, TimeZone, Utc};
use url::Url;

use crate::core::{SerializedCookie, SerializedCookieJar};

/// Build a `Cookie:` request header value containing every cookie in the
/// jar whose `path` is a prefix of `url.path()` and whose `secure` flag is
/// compatible with `url`'s scheme.
///
/// Returns `None` if no cookies apply (callers should then skip emitting
/// the header entirely rather than sending an empty one).
pub fn cookie_header_value(jar: &SerializedCookieJar, url: &Url) -> Option<String> {
    let is_https = url.scheme().eq_ignore_ascii_case("https");
    let target_path = url.path();
    let now = Utc::now();

    let mut parts: Vec<String> = Vec::new();
    for cookies in jar.jar.values() {
        for c in cookies {
            if c.secure && !is_https {
                continue;
            }
            if !path_matches(&c.path, target_path) {
                continue;
            }
            if let Some(exp) = c.expires_at {
                if exp <= now {
                    continue;
                }
            }
            parts.push(format!("{}={}", c.name, c.value));
        }
    }

    if parts.is_empty() {
        None
    } else {
        Some(parts.join("; "))
    }
}

/// RFC 6265 §5.1.4 path-match (simplified): cookie path is a prefix of the
/// request path, with the usual `/` boundary handling.
fn path_matches(cookie_path: &str, request_path: &str) -> bool {
    if cookie_path.is_empty() || cookie_path == "/" {
        return true;
    }
    if request_path == cookie_path {
        return true;
    }
    if request_path.starts_with(cookie_path) {
        let next = request_path.as_bytes().get(cookie_path.len()).copied();
        if cookie_path.ends_with('/') || next == Some(b'/') {
            return true;
        }
    }
    false
}

/// Merge a batch of `Set-Cookie` header lines into the jar.
///
/// Each line is parsed independently. Cookies with the same name+path
/// overwrite the previous entry in their bucket; new cookies are appended.
/// Cookies whose `Max-Age=0` or whose `Expires` is in the past are removed
/// from the bucket (this is how upstream signals deletion).
pub fn merge_set_cookie(
    jar: &mut SerializedCookieJar,
    set_cookie_values: impl IntoIterator<Item = String>,
) {
    for raw in set_cookie_values {
        let Some(parsed) = parse_set_cookie(&raw) else {
            continue;
        };
        let bucket = jar.jar.entry(parsed.name.clone()).or_default();

        let expired = parsed.expires_at.map(|e| e <= Utc::now()).unwrap_or(false);

        if expired {
            bucket.retain(|c| c.path != parsed.path);
            if bucket.is_empty() {
                jar.jar.remove(&parsed.name);
            }
            continue;
        }

        if let Some(existing) = bucket.iter_mut().find(|c| c.path == parsed.path) {
            existing.value = parsed.value;
            existing.secure = parsed.secure;
            existing.http_only = parsed.http_only;
            existing.expires_at = parsed.expires_at;
        } else {
            bucket.push(parsed);
        }
    }
}

#[derive(Debug)]
struct ParsedCookie {
    name: String,
    value: String,
    path: String,
    secure: bool,
    http_only: bool,
    expires_at: Option<DateTime<Utc>>,
}

impl From<ParsedCookie> for SerializedCookie {
    fn from(p: ParsedCookie) -> Self {
        SerializedCookie {
            name: p.name,
            value: p.value,
            path: p.path,
            secure: p.secure,
            http_only: p.http_only,
            expires_at: p.expires_at,
        }
    }
}

fn parse_set_cookie(raw: &str) -> Option<SerializedCookie> {
    let mut parts = raw.split(';');
    let first = parts.next()?.trim();
    let (name, value) = first.split_once('=')?;
    let name = name.trim().to_owned();
    if name.is_empty() {
        return None;
    }
    let value = value.trim().to_owned();

    let mut path = "/".to_owned();
    let mut secure = false;
    let mut http_only = false;
    let mut expires_at: Option<DateTime<Utc>> = None;
    let mut max_age: Option<i64> = None;

    for attr in parts {
        let attr = attr.trim();
        if attr.is_empty() {
            continue;
        }
        let (k, v) = match attr.split_once('=') {
            Some((k, v)) => (k.trim(), Some(v.trim())),
            None => (attr, None),
        };
        if k.eq_ignore_ascii_case("path") {
            if let Some(v) = v {
                if !v.is_empty() {
                    path = v.to_owned();
                }
            }
        } else if k.eq_ignore_ascii_case("secure") {
            secure = true;
        } else if k.eq_ignore_ascii_case("httponly") {
            http_only = true;
        } else if k.eq_ignore_ascii_case("expires") {
            if let Some(v) = v {
                expires_at = parse_http_date(v);
            }
        } else if k.eq_ignore_ascii_case("max-age") {
            if let Some(v) = v {
                if let Ok(n) = v.parse::<i64>() {
                    max_age = Some(n);
                }
            }
        }
    }

    if let Some(ma) = max_age {
        expires_at = Some(Utc::now() + chrono::Duration::seconds(ma));
    }

    Some(SerializedCookie {
        name,
        value,
        path,
        secure,
        http_only,
        expires_at,
    })
}

/// Parse an HTTP-date in RFC 1123 form (`Sun, 06 Nov 1994 08:49:37 GMT`).
/// Falls back to RFC 2822 if the strict form fails. Returns `None` on
/// unparseable input rather than erroring.
fn parse_http_date(s: &str) -> Option<DateTime<Utc>> {
    if let Ok(dt) = DateTime::parse_from_rfc2822(s) {
        return Some(dt.with_timezone(&Utc));
    }
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, "%a, %d-%b-%Y %H:%M:%S GMT") {
        return Some(Utc.from_utc_datetime(&dt));
    }
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, "%a, %d %b %Y %H:%M:%S GMT") {
        return Some(Utc.from_utc_datetime(&dt));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn jar_with(cookies: Vec<SerializedCookie>) -> SerializedCookieJar {
        let mut jar: HashMap<String, Vec<SerializedCookie>> = HashMap::new();
        for c in cookies {
            jar.entry(c.name.clone()).or_default().push(c);
        }
        SerializedCookieJar { jar }
    }

    #[test]
    fn header_value_joins_applicable_cookies() {
        let jar = jar_with(vec![
            SerializedCookie {
                name: "__Secure-next-auth.session-token".into(),
                value: "abc".into(),
                path: "/".into(),
                secure: true,
                http_only: true,
                expires_at: None,
            },
            SerializedCookie {
                name: "oai-locale".into(),
                value: "en-US".into(),
                path: "/".into(),
                secure: false,
                http_only: false,
                expires_at: None,
            },
        ]);
        let url: Url = "https://chatgpt.com/backend-api/codex/responses"
            .parse()
            .unwrap();
        let header = cookie_header_value(&jar, &url).expect("header");
        assert!(header.contains("__Secure-next-auth.session-token=abc"));
        assert!(header.contains("oai-locale=en-US"));
    }

    #[test]
    fn header_value_skips_secure_on_http() {
        let jar = jar_with(vec![SerializedCookie {
            name: "s".into(),
            value: "v".into(),
            path: "/".into(),
            secure: true,
            http_only: false,
            expires_at: None,
        }]);
        let url: Url = "http://chatgpt.com/x".parse().unwrap();
        assert!(cookie_header_value(&jar, &url).is_none());
    }

    #[test]
    fn header_value_respects_path() {
        let jar = jar_with(vec![SerializedCookie {
            name: "admin".into(),
            value: "1".into(),
            path: "/backend-api".into(),
            secure: true,
            http_only: false,
            expires_at: None,
        }]);
        let ok: Url = "https://chatgpt.com/backend-api/codex/x".parse().unwrap();
        let bad: Url = "https://chatgpt.com/public".parse().unwrap();
        assert!(cookie_header_value(&jar, &ok).is_some());
        assert!(cookie_header_value(&jar, &bad).is_none());
    }

    #[test]
    fn header_value_returns_none_for_empty_jar() {
        let jar = SerializedCookieJar::default();
        let url: Url = "https://chatgpt.com/".parse().unwrap();
        assert!(cookie_header_value(&jar, &url).is_none());
    }

    #[test]
    fn merge_set_cookie_adds_new_cookie() {
        let mut jar = SerializedCookieJar::default();
        merge_set_cookie(
            &mut jar,
            vec!["session=xyz; Path=/; Secure; HttpOnly".to_owned()],
        );
        let bucket = jar.jar.get("session").expect("bucket");
        assert_eq!(bucket.len(), 1);
        assert_eq!(bucket[0].value, "xyz");
        assert!(bucket[0].secure);
        assert!(bucket[0].http_only);
        assert_eq!(bucket[0].path, "/");
    }

    #[test]
    fn merge_set_cookie_overwrites_same_path() {
        let mut jar = SerializedCookieJar::default();
        merge_set_cookie(&mut jar, vec!["s=v1; Path=/".to_owned()]);
        merge_set_cookie(&mut jar, vec!["s=v2; Path=/".to_owned()]);
        let bucket = jar.jar.get("s").unwrap();
        assert_eq!(bucket.len(), 1);
        assert_eq!(bucket[0].value, "v2");
    }

    #[test]
    fn merge_set_cookie_deletes_expired() {
        let mut jar = SerializedCookieJar::default();
        merge_set_cookie(&mut jar, vec!["s=v; Path=/".to_owned()]);
        merge_set_cookie(&mut jar, vec!["s=v; Path=/; Max-Age=0".to_owned()]);
        assert!(!jar.jar.contains_key("s"));
    }

    #[test]
    fn merge_set_cookie_ignores_garbage() {
        let mut jar = SerializedCookieJar::default();
        merge_set_cookie(&mut jar, vec!["no-equals-sign".to_owned()]);
        merge_set_cookie(&mut jar, vec!["=empty-name; Path=/".to_owned()]);
        assert!(jar.jar.is_empty());
    }
}
