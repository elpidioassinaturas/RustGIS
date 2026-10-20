//! Secret-name and query filtering shared by connection addresses and saved sources.

/// Whether a key names a secret, wherever it is: an exact list (case aside), and any key
/// naming a password, secret or token. Context-dependent query keys are handled by [`redact_query`].
pub fn secret_key(k: &str) -> bool {
    secret_key_lower(&k.to_ascii_lowercase())
}

fn secret_key_lower(k: &str) -> bool {
    matches!(
        k,
        "token"
            | "access_token"
            | "id_token"
            | "refresh_token"
            | "api_key"
            | "apikey"
            | "api-key"
            | "x-api-key"
            | "key"
            | "access_key"
            | "accesskey"
            | "secret"
            | "client_secret"
            | "password"
            | "passwd"
            | "pwd"
            | "sig"
            | "signature"
            | "sas"
            | "authorization"
            | "proxy-authorization"
            | "cookie"
            | "set-cookie"
            | "bearer"
    ) || k.starts_with("x-amz-")
        || k.starts_with("x-goog-")
        || k.contains("password")
        || k.contains("secret")
        || k.contains("token")
}

/// The query keys of an Azure shared access signature, secret only with both `sig` and `sv`.
const SAS_KEYS: &[&str] =
    &["sv", "se", "sp", "sr", "st", "spr", "sip", "si", "ss", "srt", "skoid", "sktid", "skt", "ske", "sks", "skv", "sdd"];

fn secret_in(k: &str, keys: &[String]) -> bool {
    let has = |x: &str| keys.iter().any(|q| q == x);
    secret_key_lower(k) || (has("sig") && has("sv") && SAS_KEYS.contains(&k)) || (k == "code" && has("state"))
}

/// Removes secret query pairs without changing the address's user-info or connection options.
/// Callers must remove passwords from user-info and fragments before calling this function.
pub fn redact_query(address: &mut String) {
    let Some(at) = address.find('?') else { return };
    let query = address.get(at + 1..).unwrap_or("");
    let keys: Vec<String> = query.split('&').map(|kv| kv.split_once('=').map_or(kv, |(k, _)| k).to_ascii_lowercase()).collect();
    if query.split('&').zip(&keys).all(|(kv, key)| !kv.is_empty() && !secret_in(key, &keys)) {
        return;
    }
    let mut kept = String::with_capacity(query.len());
    for (kv, key) in query.split('&').zip(&keys) {
        if kv.is_empty() || secret_in(key, &keys) {
            continue;
        }
        if !kept.is_empty() {
            kept.push('&');
        }
        kept.push_str(kv);
    }
    address.truncate(at);
    if !kept.is_empty() {
        address.push('?');
        address.push_str(&kept);
    }
}
