//! SQL Server connection addresses (`mssql://user:password@host:port/database?…`): taken apart
//! so a connection can be named without its password. Pure text, so the browser build has it too.

/// Whether `s` is a SQL Server address (`mssql://…`).
pub fn is_address(s: &str) -> bool {
    s.trim().get(..8).is_some_and(|p| p.eq_ignore_ascii_case("mssql://"))
}

/// The address without passwords or secret query values, and its decoded password.
/// `None` for other schemes.
pub fn split_password(s: &str) -> Option<(String, Option<String>)> {
    if !is_address(s) {
        return None;
    }
    let (mut public, password) = super::myconn::split_userinfo_password(s.trim())?;
    let password = password.or_else(|| {
        let (_, query) = public.split_once('?')?;
        query.split('&').find_map(|kv| {
            let (key, value) = kv.split_once('=')?;
            (key.eq_ignore_ascii_case("pwd") || key.eq_ignore_ascii_case("passwd")).then(|| super::pgconn::percent_decode(value))
        })
    });
    super::credentials::redact_query(&mut public);
    Some((public, password))
}

pub use super::myconn::database_name;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_lose_their_passwords() {
        let split = |a: &str| split_password(a).unwrap_or_else(|| panic!("{a}"));
        assert_eq!(split("mssql://sa:p%40ss@db:1433/gis"), ("mssql://sa@db:1433/gis".into(), Some("p@ss".into())));
        assert_eq!(split("mssql://CORP%5Cana:pw@h/gis"), ("mssql://CORP%5Cana@h/gis".into(), Some("pw".into())));
        assert_eq!(split("mssql://h/gis?password=x&encrypt=strict"), ("mssql://h/gis?encrypt=strict".into(), Some("x".into())));
        for key in ["pwd", "passwd", "token", "access_token", "client_secret", "X-Amz-Credential"] {
            let address = format!("mssql://CORP%5Cana:pw@h/gis?{key}=secret&encrypt=strict&instance=GIS&ca=/cert.pem");
            assert_eq!(
                split(&address),
                ("mssql://CORP%5Cana@h/gis?encrypt=strict&instance=GIS&ca=/cert.pem".into(), Some("pw".into())),
                "{key}"
            );
        }
        assert!(is_address(" MSSQL://h/gis "));
        assert!(split_password("mysql://h").is_none());
        assert_eq!(database_name("mssql://u@h:1433/my%20db?x=1").as_deref(), Some("my db"));
    }

    #[test]
    fn password_aliases_preserve_precedence_and_decoding() {
        for (query, expected) in [
            ("pwd=p%40ss", "p@ss"),
            ("PaSsWd=hello%20world", "hello world"),
            ("PWD=first&passwd=second", "first"),
            ("passwd=first&pwd=second", "first"),
            ("pwd=ignored&password=chosen", "chosen"),
            ("passwd=ignored&pass=chosen", "chosen"),
            ("pwd=&passwd=second", ""),
        ] {
            let (public, password) = split_password(&format!("mssql://sa@h/gis?{query}&encrypt=strict")).unwrap();
            assert_eq!(password.as_deref(), Some(expected), "{query}");
            assert_eq!(public, "mssql://sa@h/gis?encrypt=strict", "{query}");
        }
        let (_, password) = split_password("mssql://sa:userinfo@h/gis?password=query&pwd=alias").unwrap();
        assert_eq!(password.as_deref(), Some("userinfo"));
    }
}
