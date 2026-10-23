//! Named, reusable endpoints kept with the app's preferences, not the project. Never a
//! secret: addresses are stripped of passwords, tokens and signatures before they are kept.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::sources::{self, Sources};

pub const MAX_CONNECTIONS: usize = 200;
const MAX_TEXT: usize = 2048;
const MAX_NAME: usize = 200;
pub const KINDS: &[&str] =
    &["postgres", "mysql", "sqlserver", "service", "wms", "wmts", "wfs", "wcs", "ogcapi", "arcgis", "csw", "stac", "s3"];
pub const DATABASE_KINDS: &[&str] = &["postgres", "mysql", "sqlserver"];
pub const S3_CREDENTIALS: &[&str] = &["keys", "profile", "environment", "none"];
pub const DESKTOP_ONLY: &str = "database connections are used by the desktop app";

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Connection {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub group: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub address: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub options: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Connections {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub next: u64,
    #[serde(default)]
    pub connections: Vec<Connection>,
    #[serde(skip)]
    pub generation: u64,
}

pub fn database_kind(address: &str) -> Option<&'static str> {
    if rustgis_data::source::myconn::split_password(address).is_some() {
        Some("mysql")
    } else if rustgis_data::source::msconn::split_password(address).is_some() {
        Some("sqlserver")
    } else {
        let lower = address.trim().to_ascii_lowercase();
        (lower.starts_with("postgres:") || lower.starts_with("postgresql:")).then_some("postgres")
    }
}

/// The database kind and session-password key, as the driver receives the password-free address.
/// None for other addresses, and always in the browser.
pub fn database_key(address: &str) -> Option<(&'static str, String)> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        use rustgis_data::source::duckdb::Database;
        match Database::parse(address) {
            Database::Postgres(d) => Some(("postgres", d)),
            Database::Mysql(d) => Some(("mysql", d)),
            Database::Sqlserver(d) => Some(("sqlserver", d)),
            Database::File(_) => None,
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = address;
        None
    }
}

/// The kind and database address a streamed layer reads.
pub fn layer_database(src: &rustgis_map::source::SourceLayer) -> Option<(&'static str, &str)> {
    if let Some(origin) = &src.postgis {
        Some(("postgres", origin.database.as_str()))
    } else if let Some(origin) = &src.mysql {
        Some(("mysql", origin.database.as_str()))
    } else {
        src.sqlserver.as_ref().map(|origin| ("sqlserver", origin.database.as_str()))
    }
}

fn same_name(a: &str, b: &str) -> bool {
    a.chars().flat_map(char::to_lowercase).eq(b.chars().flat_map(char::to_lowercase))
}

fn name_text(t: &str) -> String {
    t.trim().chars().filter(|c| !c.is_control()).take(MAX_NAME).collect()
}

pub fn clean(mut c: Connection) -> Result<(Connection, bool), String> {
    if !KINDS.contains(&c.kind.as_str()) {
        return Err(format!("unknown kind '{}' ({})", c.kind, KINDS.join(", ")));
    }
    let t = c.address.trim();
    if t.len() > MAX_TEXT || t.chars().any(char::is_control) {
        return Err("a connection address must be at most 2048 bytes and contain no control characters".into());
    }
    let stored = sources::redact(t);
    let stripped = stored != t;
    if DATABASE_KINDS.contains(&c.kind.as_str()) {
        if stored.is_empty() || database_kind(&stored) != Some(c.kind.as_str()) {
            return Err(format!(
                "'{stored}' isn't a {} address (postgres:host=… dbname=… user=…, mysql://user@host/db, mssql://user@host/db)",
                c.kind
            ));
        }
    } else {
        let lower = stored.to_ascii_lowercase();
        if !(lower.starts_with("http://") || lower.starts_with("https://") || c.kind == "s3" && stored.is_empty()) {
            return Err(format!("a {} connection needs an http:// or https:// address", c.kind));
        }
    }
    c.address = stored;
    c.name = name_text(&c.name);
    if c.name.is_empty() {
        c.name = if c.kind == "s3" && c.address.is_empty() { "AWS S3".into() } else { sources::default_name(&c.address) };
    }
    c.group = name_text(&c.group);
    if c.kind != "s3" && !c.options.is_empty() {
        return Err(format!("{} connections take no options", c.kind));
    }
    for (k, v) in &mut c.options {
        match k.as_str() {
            "region" | "profile" => {
                let max = if k == "region" { 64 } else { MAX_NAME };
                if v.chars().count() > max || v.chars().any(char::is_control) {
                    return Err(format!("{k} must be at most {max} characters and contain no control characters"));
                }
            }
            "buckets" => {
                if !v.is_empty() {
                    let buckets: Vec<_> = v.split(',').map(str::trim).collect();
                    if buckets.iter().any(|b| !crate::render::rustgis_tiles::s3::valid_bucket(b)) {
                        return Err("invalid S3 bucket name".into());
                    }
                    *v = buckets.join(",");
                }
            }
            "credentials" => {
                if !v.is_empty() && !S3_CREDENTIALS.contains(&v.as_str()) {
                    return Err(format!("credentials must be one of {}", S3_CREDENTIALS.join(", ")));
                }
            }
            _ => return Err(format!("'{k}' isn't an S3 connection option (region, buckets, credentials, profile)")),
        }
    }
    if c.options.get("credentials").map(String::as_str) != Some("profile") {
        c.options.remove("profile");
    }
    c.options.retain(|_, v| !v.is_empty());
    Ok((c, stripped))
}

impl Connections {
    /// The first saved connection, in list order, of a database's kind whose address names it.
    pub fn for_database(&self, kind: &str, database: &str) -> Option<&Connection> {
        let key = database_key(&sources::redact(database)).filter(|k| k.0 == kind)?;
        self.connections.iter().find(|c| c.kind == kind && database_key(&c.address).as_ref() == Some(&key))
    }

    /// The saved connection a layer that reads a database comes from.
    pub fn for_layer(&self, l: &rustgis_map::Layer) -> Option<&Connection> {
        let (kind, database) = layer_database(l.source.as_ref()?)?;
        self.for_database(kind, database)
    }

    pub fn from_json(text: &str) -> Self {
        let v: Value = serde_json::from_str(text).unwrap_or(Value::Null);
        let mut s = Self { version: 1, next: v.get("next").and_then(Value::as_u64).unwrap_or(1).max(1), ..Self::default() };
        let mut ids = BTreeSet::new();
        if let Some(rows) = v.get("connections").and_then(Value::as_array) {
            for row in rows {
                let Ok(c) = serde_json::from_value::<Connection>(row.clone()) else { continue };
                let Ok((c, _)) = clean(c) else { continue };
                if c.id.is_empty()
                    || c.id.len() > 32
                    || !c.id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
                    || !ids.insert(c.id.clone())
                {
                    continue;
                }
                if let Some(n) = c.id.strip_prefix('c').and_then(|n| n.parse::<u64>().ok()) {
                    s.next = s.next.max(n.saturating_add(1));
                }
                s.connections.push(c);
                if s.connections.len() == MAX_CONNECTIONS {
                    break;
                }
            }
        }
        // All IDs must be known before a name is accepted: a later row can reserve it.
        // Normalize in order so the first duplicate name keeps precedence.
        for i in 0..s.connections.len() {
            let Some(c) = s.connections.get(i) else { continue };
            let name = s.unique_name(&c.name, None, s.connections.get(..i).unwrap_or_default());
            if let Some(c) = s.connections.get_mut(i) {
                c.name = name;
            }
        }
        s
    }

    pub fn import(saved: &Sources, s3_saved: &str) -> Self {
        let mut s = Self::default();
        for old in &saved.saved {
            let kind = match old.kind.as_str() {
                "service" => old
                    .extra
                    .get("type")
                    .and_then(Value::as_str)
                    .filter(|k| matches!(*k, "wms" | "wmts" | "wfs" | "wcs" | "ogcapi" | "arcgis" | "csw"))
                    .unwrap_or("service"),
                "database" => match database_kind(&old.address) {
                    Some(k) => k,
                    None => continue,
                },
                "stac" if old.extra.get("api").is_some() => "stac",
                _ => continue,
            };
            if s.connections.iter().any(|c| c.kind == kind && c.address == old.address) {
                continue;
            }
            let _ = s.add_unique(Connection {
                name: old.name.clone(),
                kind: kind.into(),
                address: old.address.clone(),
                ..Connection::default()
            });
        }
        let rows: Value = serde_json::from_str(s3_saved).unwrap_or(Value::Null);
        if let Some(rows) = rows.as_array() {
            for row in rows.iter().take(32) {
                let Some(row) = row.as_object() else { continue };
                let text = |k: &str| row.get(k).and_then(Value::as_str).unwrap_or("").to_owned();
                let source = text("source");
                let credentials = if S3_CREDENTIALS.contains(&source.as_str()) { source } else { "keys".into() };
                let _ = s.add_unique(Connection {
                    name: text("name"),
                    kind: "s3".into(),
                    address: text("endpoint"),
                    options: BTreeMap::from([
                        ("region".into(), text("region")),
                        ("credentials".into(), credentials),
                        ("profile".into(), text("profile")),
                    ]),
                    ..Connection::default()
                });
            }
        }
        s
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(&json!({"version": 1, "next": self.next, "connections": self.connections})).unwrap_or_default()
    }

    pub fn find(&self, key: &str) -> Option<&Connection> {
        self.connections.iter().find(|c| c.id == key).or_else(|| self.connections.iter().find(|c| same_name(&c.name, key.trim())))
    }

    /// IDs win lookup, so names must not occupy either a stored ID or a future generated ID.
    fn name_is_id(&self, name: &str) -> bool {
        let generated = name.get(..1).is_some_and(|c| c.eq_ignore_ascii_case("c"))
            && name.get(1..).is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
        generated || self.connections.iter().any(|c| same_name(&c.id, name))
    }

    // Loading checks names only against earlier rows, but IDs against the whole collection.
    fn unique_name(&self, base: &str, except: Option<&str>, named: &[Connection]) -> String {
        let taken = |name: &str| self.name_is_id(name) || named.iter().any(|c| Some(c.id.as_str()) != except && same_name(&c.name, name));
        if !taken(base) {
            return base.into();
        }
        for n in 2.. {
            let suffix = format!(" ({n})");
            let name = format!("{}{suffix}", base.chars().take(MAX_NAME.saturating_sub(suffix.len())).collect::<String>());
            if !taken(&name) {
                return name;
            }
        }
        base.into()
    }

    pub fn add(&mut self, c: Connection, explicit_name: bool) -> Result<(Connection, bool), String> {
        let (mut c, stripped) = clean(c)?;
        if self.connections.len() >= MAX_CONNECTIONS {
            return Err(format!("up to {MAX_CONNECTIONS} connections"));
        }
        if explicit_name && self.name_is_id(&c.name) {
            return Err("a connection name cannot be a connection id".into());
        }
        let name = self.unique_name(&c.name, None, &self.connections);
        if explicit_name && name != c.name {
            return Err(format!("a connection named '{}' exists", c.name));
        }
        c.name = name;
        let mut next = self.next.max(1);
        while self.connections.iter().any(|c| c.id == format!("c{next}")) {
            next = next.checked_add(1).ok_or("connection ids exhausted")?;
        }
        self.next = next.checked_add(1).ok_or("connection ids exhausted")?;
        c.id = format!("c{next}");
        self.connections.push(c.clone());
        self.version = 1;
        self.generation = self.generation.wrapping_add(1);
        Ok((c, stripped))
    }

    fn add_unique(&mut self, c: Connection) -> Result<(Connection, bool), String> {
        self.add(c, false)
    }

    pub fn update(&mut self, id: &str, c: Connection) -> Result<(Connection, bool), String> {
        let explicit_name = !c.name.trim().is_empty();
        let (mut c, stripped) = clean(c)?;
        if explicit_name && self.name_is_id(&c.name) {
            return Err("a connection name cannot be a connection id".into());
        }
        let name = self.unique_name(&c.name, Some(id), &self.connections);
        if explicit_name && name != c.name {
            return Err(format!("a connection named '{}' exists", c.name));
        }
        c.name = name;
        let slot = self.connections.iter_mut().find(|c| c.id == id).ok_or_else(|| format!("no connection '{id}'"))?;
        c.id = id.into();
        *slot = c.clone();
        self.generation = self.generation.wrapping_add(1);
        Ok((c, stripped))
    }

    pub fn remove(&mut self, id: &str) -> Option<Connection> {
        let index = self.connections.iter().position(|c| c.id == id)?;
        let c = self.connections.remove(index);
        self.generation = self.generation.wrapping_add(1);
        Some(c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_connections() {
        let saved = Sources::from_json(
            &json!({"saved": [
                {"kind":"service", "name":"Shared", "address":"https://w.example/wms", "extra":{"type":"wms"}},
                {"kind":"service", "name":"Shared", "address":"https://s.example"},
                {"kind":"database", "name":"PG", "address":"postgres:host=h dbname=g user=u"},
                {"kind":"database", "name":"My", "address":"mysql://u@h/db"},
                {"kind":"database", "name":"MS", "address":"mssql://u@h/db"},
                {"kind":"database", "name":"File", "address":"/x/a.duckdb"},
                {"kind":"stac", "name":"API", "address":"https://stac.example", "extra":{"api":"true"}},
                {"kind":"stac", "name":"Item", "address":"https://stac.example/item"},
                {"kind":"service", "name":"Duplicate", "address":"https://w.example/wms", "extra":{"type":"wms"}}
            ]})
            .to_string(),
        );
        let s = Connections::import(
            &saved,
            r#"[{"name":"Keys","source":""},{"name":"Profile","source":"profile","profile":"work"},{"name":"Public","source":"none"}]"#,
        );
        let kinds: Vec<_> = s.connections.iter().map(|c| c.kind.as_str()).collect();
        assert_eq!(kinds, ["wms", "service", "postgres", "mysql", "sqlserver", "stac", "s3", "s3", "s3"]);
        assert_eq!(s.connections[1].name, "Shared (2)");
        assert_eq!(s.find("Keys").unwrap().options["credentials"], "keys");
        assert_eq!(s.find("Profile").unwrap().options["profile"], "work");
        assert_eq!(s.find("Public").unwrap().options["credentials"], "none");
        assert_eq!(Connections::from_json(&s.to_json()).connections, s.connections);
    }

    #[test]
    fn connections_tolerate_junk_and_preserve_identity() {
        let mut s = Connections::from_json(
            &json!({"next":2,"connections":[
                null, {"id":"bad!","name":"Bad","kind":"wms","address":"https://a"},
                {"id":"c90","name":"A","kind":"wms","address":"https://a"},
                {"id":"c90","name":"B","kind":"wms","address":"https://b"},
                {"id":"custom","name":"a","kind":"wms","address":"https://b"},
                {"id":"c999","name":"Bad","kind":"unknown","address":"https://b"}
            ]})
            .to_string(),
        );
        assert_eq!(s.connections.len(), 2);
        assert_eq!(s.connections[1].name, "a (2)");
        let (c, stripped) = s
            .add(
                Connection {
                    kind: "postgres".into(),
                    name: "DB".into(),
                    address: "postgresql://u:pw@h/db".into(),
                    ..Connection::default()
                },
                true,
            )
            .unwrap();
        assert_eq!(c.id, "c91");
        assert!(stripped);
        assert!(!s.to_json().contains("pw"));
        let (other, _) = s.add(Connection { name: "Other".into(), ..c.clone() }, true).unwrap();
        assert_ne!(c.id, other.id);
        assert_eq!(c.address, other.address);
        assert_eq!(Connections::from_json(&s.to_json()).connections, s.connections);
    }

    #[test]
    fn connections_names_cannot_shadow_stored_or_future_ids() {
        let mut s = Connections::from_json(
            &json!({"connections": [
                {"id":"first","name":"REMOTE","kind":"s3"},
                {"id":"remote","name":"Remote (2)","kind":"s3"},
                {"id":"third","name":"c4","kind":"s3"},
                {"id":"fourth","name":"remote","kind":"s3"}
            ]})
            .to_string(),
        );
        assert_eq!(s.connections[0].name, "REMOTE (2)");
        assert_eq!(s.connections[1].name, "Remote (2) (2)");
        assert_eq!(s.connections[2].name, "c4 (2)");
        assert_eq!(s.connections[3].name, "remote (3)");
        for c in &s.connections {
            assert_eq!(s.find(&c.name), Some(c));
        }
        let template = s.connections[0].clone();
        for name in ["remote", "FIRST", "c1", "c5", "C999"] {
            let c = Connection { name: name.into(), ..template.clone() };
            assert!(s.add(c.clone(), true).is_err(), "{name}");
            assert!(s.update("first", c).is_err(), "{name}");
        }
        let (generated, _) = s.add(Connection { name: "c1".into(), ..template.clone() }, false).unwrap();
        assert_eq!(generated.id, "c1");
        assert_eq!(generated.name, "c1 (2)");
        assert_eq!(s.find(&generated.name), Some(&generated));
        assert_eq!(Connections::from_json(&s.to_json()).connections, s.connections);
    }

    #[test]
    fn connections_blank_edit_name_uses_a_unique_default() {
        let mut s = Connections::default();
        let c = Connection { kind: "wms".into(), address: "https://a.example/wms".into(), ..Connection::default() };
        let (first, _) = s.add(c.clone(), false).unwrap();
        let (second, _) = s.add(Connection { name: "Other".into(), ..c }, true).unwrap();
        let (edited, _) = s.update(&second.id, Connection { name: "  ".into(), ..second.clone() }).unwrap();
        assert_eq!(edited.name, "wms (2)");
        assert_eq!(s.find(&first.name), Some(&first));
        assert!(s.update(&second.id, Connection { name: first.name, ..edited }).is_err());
    }

    #[test]
    fn connections_validate_options_and_bounds() {
        let c = Connection {
            kind: "s3".into(),
            options: BTreeMap::from([
                ("credentials".into(), "profile".into()),
                ("profile".into(), "work".into()),
                ("buckets".into(), " b11, b22 ".into()),
            ]),
            ..Connection::default()
        };
        let (c, _) = clean(c).unwrap();
        assert_eq!(c.options["buckets"], "b11,b22");
        assert!(clean(Connection { options: BTreeMap::from([("password".into(), "secret".into())]), ..c.clone() }).is_err());
        assert!(clean(Connection { options: BTreeMap::from([("buckets".into(), "NO_".into())]), ..c }).is_err());
        let mut s = Connections::from_json(r#"{"next":18446744073709551615}"#);
        assert!(s.add(Connection { kind: "s3".into(), ..Connection::default() }, false).is_err());
    }
}
