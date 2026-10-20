//! MySQL and MariaDB connection addresses (`mysql://user:password@host:port/database?…`, and
//! `mariadb://`): taken apart so a connection can be named without its password. Pure text, so
//! the browser build has it too.

/// Whether `s` is a MySQL or MariaDB address (`mysql://…`, `mariadb://…`).
pub fn is_address(s: &str) -> bool {
    let lower = s.trim().to_ascii_lowercase();
    lower.starts_with("mysql://") || lower.starts_with("mariadb://")
}

/// A connection address without its password (`mysql://user@host:port/db?…`, the scheme as
/// written), and the password, decoded. `None` for anything but a MySQL or MariaDB address.
pub fn split_password(s: &str) -> Option<(String, Option<String>)> {
    let t = s.trim();
    if !is_address(t) {
        return None;
    }
    split_userinfo_password(t)
}

/// Removes user-info and query passwords without restricting the address's scheme.
pub(super) fn split_userinfo_password(t: &str) -> Option<(String, Option<String>)> {
    let (scheme, rest) = t.split_once("://")?;
    // The user and password end at the `@` before the host, found before anything else splits
    // the address: a password may hold an unencoded `/`, `?`, `#` or `@`.
    // A password of digits before an unencoded `/`, `?` or `#` reads like a port (`u:123/w@h`):
    // a database name has no `@`, so one before the query ends the user and password then.
    let at = super::pgconn::userinfo_end(rest)
        .or_else(|| {
            let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
            let query = rest.find('?').unwrap_or(rest.len());
            rest.get(..query)?.rfind('@').filter(|&i| i >= end)
        })
        // … or before an unencoded `?` (`u:12?x@h`): an `@` in the query before any `=`, `&` or `/`
        // isn't a parameter's.
        .or_else(|| {
            let q = rest.find('?')?;
            let tail = rest.get(q + 1..)?;
            // The last `@` before them: the password may hold another.
            let head = tail.get(..tail.find(['=', '&', '/']).unwrap_or(tail.len()))?;
            head.rfind('@').map(|i| q + 1 + i)
        });
    let (user, password, after) = match at {
        Some(at) => {
            let (userinfo, after) = (rest.get(..at).unwrap_or(""), rest.get(at + 1..).unwrap_or(""));
            match userinfo.split_once(':') {
                Some((user, pw)) => (user, Some(super::pgconn::percent_decode(pw)), after),
                None => (userinfo, None, after),
            }
        }
        None => ("", None, rest),
    };
    // A fragment is never part of a connection (and may hold anything).
    let after = after.split('#').next().unwrap_or(after);
    let (after, in_query) = query_without_password(after);
    let password = password.or(in_query);
    Some((if user.is_empty() { format!("{scheme}://{after}") } else { format!("{scheme}://{user}@{after}") }, password))
}

/// An address's text without a `password` in its query, and that password.
fn query_without_password(s: &str) -> (String, Option<String>) {
    let Some((base, query)) = s.split_once('?') else { return (s.to_string(), None) };
    let mut password = None;
    let mut kept = Vec::new();
    for kv in query.split('&').filter(|kv| !kv.is_empty()) {
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        if k.eq_ignore_ascii_case("password") || k.eq_ignore_ascii_case("pass") {
            password = Some(super::pgconn::percent_decode(v));
        } else {
            kept.push(kv);
        }
    }
    (if kept.is_empty() { base.to_string() } else { format!("{base}?{}", kept.join("&")) }, password)
}

/// The database an address names (its path), if any.
pub fn database_name(address: &str) -> Option<String> {
    let (_, rest) = address.split_once("://")?;
    let rest = rest.split(['?', '#']).next().unwrap_or(rest);
    let after = match super::pgconn::userinfo_end(rest) {
        Some(at) => rest.get(at + 1..)?,
        None => rest,
    };
    let (_, path) = after.split_once('/')?;
    let name = super::pgconn::percent_decode(path.trim_matches('/'));
    (!name.is_empty()).then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_lose_their_passwords() {
        let s = |a: &str| split_password(a).unwrap_or_else(|| panic!("{a}"));
        assert_eq!(s("mysql://me:p%40ss@db.example:3307/gis"), ("mysql://me@db.example:3307/gis".into(), Some("p@ss".into())));
        assert_eq!(s("mariadb://me@h/gis?ssl-mode=REQUIRED&password=x"), ("mariadb://me@h/gis?ssl-mode=REQUIRED".into(), Some("x".into())));
        assert_eq!(s("mysql://h/gis#frag"), ("mysql://h/gis".into(), None));
        // A password holding an unencoded `/` or `@` still goes.
        assert_eq!(s("mysql://me:a/b@c@h/gis").0, "mysql://me@h/gis");
        assert_eq!(s("mysql://:pw@h/gis"), ("mysql://h/gis".into(), Some("pw".into())));
        // A password of digits before an unencoded `/` or `#` (it reads like a port).
        assert_eq!(s("mysql://u:123/w@db.example/gis"), ("mysql://u@db.example/gis".into(), Some("123/w".into())));
        assert_eq!(s("mysql://u:12#x@h:3306/gis"), ("mysql://u@h:3306/gis".into(), Some("12#x".into())));
        assert_eq!(s("mysql://h:3306/gis"), ("mysql://h:3306/gis".into(), None));
        assert_eq!(s("mysql://u:12?x@h/gis"), ("mysql://u@h/gis".into(), Some("12?x".into())));
        assert_eq!(s("mysql://u:12?part@secret@h/gis"), ("mysql://u@h/gis".into(), Some("12?part@secret".into())));
        assert_eq!(s("mysql://h/gis?ssl-ca=/a@b.pem"), ("mysql://h/gis?ssl-ca=/a@b.pem".into(), None));
        assert_eq!(s("mysql://me:p#w?x@h:3306/gis#f"), ("mysql://me@h:3306/gis".into(), Some("p#w?x".into())));
        assert_eq!(s("mysql://me@h/gis?pass=a%26b&ssl-mode=DISABLED"), ("mysql://me@h/gis?ssl-mode=DISABLED".into(), Some("a&b".into())));
        assert!(split_password("postgres:host=h").is_none());
        assert_eq!(database_name("mysql://me@h:3306/my%20db?x=1").as_deref(), Some("my db"));
        assert_eq!(database_name("mysql://h"), None);
        assert_eq!(database_name("mysql://h/"), None);
    }
}
