//! Saved connections and session-only logins. Listing handlers are called directly: these
//! commands never edit the project, create undo snapshots or note recently added sources.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use super::{b, ps};
use crate::connections::{Connection, DATABASE_KINDS, DESKTOP_ONLY, KINDS};
use crate::render::rustgis_tiles::{s3, stac};
use crate::{CmdResult, CommandSpec as C, Session};

#[path = "connection_browse.rs"]
mod browse;

pub fn commands() -> Vec<C> {
    vec![
        C::new("connection.list", "List Connections", "Home › Add › Data Sources", list).view().params(
            r#"{"kind"?: "postgres"|"mysql"|"sqlserver"|"service"|"wms"|"wmts"|"wfs"|"wcs"|"ogcapi"|"arcgis"|"csw"|"stac"|"s3", "group"?: text}"#,
        ),
        C::new("connection.new", "New Connection", "Home › Add › Data Sources", new).view().params(
            r#"{"kind": "postgres"|"mysql"|"sqlserver"|"service"|"wms"|"wmts"|"wfs"|"wcs"|"ogcapi"|"arcgis"|"csw"|"stac"|"s3", "address"?: "as the kind's add command takes it, without its password or token (postgres:host=… dbname=… user=…, mysql://user@host/db, mssql://user@host/db, a service's or STAC API's https:// address, an S3 endpoint: AWS when absent)", "name"?: "unique (default from the address)", "group"?: text, "options"?: {"region"?: text, "buckets"?: "a,b", "credentials"?: "keys"|"profile"|"environment"|"none", "profile"?: text} (S3 only)}"#,
        ),
        C::new("connection.edit", "Edit Connection", "Home › Add › Data Sources", edit).view().params(
            r#"{"id": "a connection's id or name", "name"?: text, "group"?: text, "address"?: text, "options"?: "as connection.new (replaces them)", "kind"?: "its kind (unchanged)"}"#,
        ),
        C::new("connection.duplicate", "Duplicate Connection", "Home › Add › Data Sources", duplicate).view().params(r#"{"id": "a connection's id or name", "name"?: text}"#),
        C::new("connection.remove", "Remove Connection", "Home › Add › Data Sources", remove).view().params(r#"{"id": "a connection's id or name"}"#),
        C::new("connection.login", "Log In to Connection", "Home › Add › Data Sources", login_command).view().params(
            r#"{"id": "a connection's id or name", "password"?: "a database connection's, kept for this session only; the project's layers that read its database and couldn't be read open again", "accessKeyId"?: text, "secretAccessKey"?: text, "sessionToken"?: "an S3 connection's keys (credentials: keys), kept for this session only"}"#,
        ),
        C::new("connection.test", "Test Connection", "Home › Add › Data Sources", test).view().params(
            r#"{"id"?: "a connection's id or name", "kind"?: "postgres"|"mysql"|"sqlserver"|"service"|"wms"|"wmts"|"wfs"|"wcs"|"ogcapi"|"arcgis"|"csw"|"stac"|"s3", "name"?: text, "group"?: text, "address"?: text, "options"?: "an unsaved connection, as connection.new (when no id)", "password"?: text, "accessKeyId"?: text, "secretAccessKey"?: text, "sessionToken"?: "as connection.login (logs in first)", "probe"?: "true: only the address the test reads, {fetch} (null for databases, which their driver tests)", "answer"?: "non-database kinds only: what that address returned, when the app fetched it (no network request then); databases ignore answer and test through their driver"}"#,
        ),
        C::new("connection.browse", "Browse Connection", "Home › Add › Data Sources", browse::browse).view().params(
            r#"{"id": "a connection's id or name", "node"?: "a child's node, as a listing gives it (the connection's top level when absent)", "probe"?: "true: only the address this level reads, {fetch} (null: read without a fetch, or by the database driver); an S3 connection logs in first, as connection.test does", "answer"?: "what that address returned, when the app fetched it", "documents"?: [{"url": "a further document a listing asked for (next)", "text": "its answer"}]} → {connection, node, children: [{key, name, what, detail?, node? (browse it), children? (listed already), add? {command, params}: the command that adds it}], truncated?, matched?, notes?, service? {url, type}, next? (fetch it, then ask again with answer and documents)}"#,
        ),
    ]
}

fn shown(c: &Connection) -> Value {
    let database = DATABASE_KINDS.contains(&c.kind.as_str());
    let available = !database || !cfg!(target_arch = "wasm32");
    let logged_in = if c.kind == "s3" {
        s3::connection(&c.name).is_some()
    } else if database {
        #[cfg(not(target_arch = "wasm32"))]
        {
            use rustgis_data::source::duckdb::{self, Database};
            match Database::parse(&c.address) {
                Database::Postgres(d) | Database::Mysql(d) | Database::Sqlserver(d) => duckdb::has_password(&d),
                Database::File(_) => false,
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            false
        }
    } else {
        false
    };
    let add_with = "connection.browse {id}, then {id, node} for a child's node; run a dataset's add {command, params}";
    let mut out = serde_json::to_value(c).unwrap_or(Value::Null);
    out["available"] = json!(available);
    if !available {
        out["why"] = json!(DESKTOP_ONLY);
    }
    out["loggedIn"] = json!(logged_in);
    out["addWith"] = json!(add_with);
    out
}

fn found<'a>(s: &'a Session, p: &Value) -> Result<&'a Connection, String> {
    let id = ps(p, "id").unwrap_or("");
    s.connections.find(id).ok_or_else(|| format!("no connection '{id}'"))
}

fn options(p: &Value) -> Result<BTreeMap<String, String>, String> {
    let object = p.as_object().ok_or("options is an object of texts")?;
    object
        .iter()
        .map(|(k, v)| v.as_str().map(|t| (k.clone(), t.to_owned())).ok_or_else(|| "options is an object of texts".into()))
        .collect()
}

/// Saved database connections, in list order, with how many project layers wait for their login.
pub(crate) fn waiting(s: &Session) -> Vec<(Connection, usize)> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut counts = BTreeMap::<String, usize>::new();
        for layer in s.project.maps.iter().flat_map(|m| &m.layers) {
            let Some(src) = layer.source.as_ref().filter(|src| src.login) else { continue };
            let Some((kind, database)) = crate::connections::layer_database(src) else { continue };
            if let Some(c) = s.connections.for_database(kind, database) {
                *counts.entry(c.id.clone()).or_default() += 1;
            }
        }
        s.connections.connections.iter().filter_map(|c| counts.get(&c.id).map(|&n| (c.clone(), n))).collect()
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = s;
        Vec::new()
    }
}

/// Says, once per connection, which layers wait for its login (a project just opened).
pub(crate) fn note_waiting(s: &mut Session) {
    for (c, n) in waiting(s) {
        s.notify(format!("{n} layer(s) read the database of the connection '{}' and need its password: right-click the connection in Data Sources › Connections, or one of the layers, and choose Log In…", c.name), false);
    }
}

fn list(s: &mut Session, p: &Value) -> CmdResult {
    let kind = ps(p, "kind");
    if let Some(k) = kind
        && !KINDS.contains(&k)
    {
        return Err(format!("unknown kind '{k}' ({})", KINDS.join(", ")));
    }
    let group = ps(p, "group");
    let waiting: BTreeMap<_, _> = waiting(s).into_iter().map(|(c, n)| (c.id, n)).collect();
    Ok(json!({"connections": s.connections.connections.iter()
        .filter(|c| kind.is_none_or(|k| c.kind == k) && group.is_none_or(|g| c.group.chars().flat_map(char::to_lowercase).eq(g.chars().flat_map(char::to_lowercase))))
        .map(|c| {
            let mut out = shown(c);
            if let Some(&n) = waiting.get(&c.id) {
                out["waiting"] = json!(n);
            }
            out
        }).collect::<Vec<_>>() }))
}

fn new(s: &mut Session, p: &Value) -> CmdResult {
    let c = Connection {
        kind: ps(p, "kind").ok_or_else(|| format!("kind is required ({})", KINDS.join(", ")))?.trim().into(),
        address: ps(p, "address").unwrap_or("").into(),
        name: ps(p, "name").unwrap_or("").into(),
        group: ps(p, "group").unwrap_or("").into(),
        options: p.get("options").map(options).transpose()?.unwrap_or_default(),
        ..Connection::default()
    };
    let explicit = !c.name.trim().is_empty();
    let (c, stripped) = s.connections.add(c, explicit)?;
    Ok(shown_stripped(&c, stripped))
}

fn shown_stripped(c: &Connection, stripped: bool) -> Value {
    let mut out = shown(c);
    if stripped {
        out["stripped"] = json!(true);
    }
    out
}

fn edit(s: &mut Session, p: &Value) -> CmdResult {
    let old = found(s, p)?.clone();
    let mut c = old.clone();
    if let Some(kind) = ps(p, "kind")
        && kind.trim() != c.kind
    {
        return Err("a connection's kind doesn't change: make a new one".into());
    }
    for (key, field) in [("name", &mut c.name), ("group", &mut c.group), ("address", &mut c.address)] {
        if let Some(value) = p.get(key) {
            *field = value.as_str().ok_or_else(|| format!("{key} is text"))?.into();
        }
    }
    if let Some(v) = p.get("options") {
        c.options = options(v)?;
    }
    let (c, stripped) = s.connections.update(&old.id, c)?;
    if c.kind == "s3" && (old.name != c.name || old.address != c.address || old.options != c.options) {
        s3::disconnect(&old.name);
        if old.name != c.name {
            s3::disconnect(&c.name);
        }
    }
    Ok(shown_stripped(&c, stripped))
}

fn duplicate(s: &mut Session, p: &Value) -> CmdResult {
    let mut c = found(s, p)?.clone();
    c.id.clear();
    let explicit = ps(p, "name").is_some_and(|n| !n.trim().is_empty());
    if let Some(name) = ps(p, "name") {
        c.name = name.into();
    }
    let (c, _) = s.connections.add(c, explicit)?;
    Ok(shown(&c))
}

fn remove(s: &mut Session, p: &Value) -> CmdResult {
    let c = found(s, p)?.clone();
    s.connections.remove(&c.id);
    if c.kind == "s3" {
        s3::disconnect(&c.name);
    }
    Ok(json!({"removed": c.id}))
}

#[derive(Default)]
struct Reopened {
    opened: usize,
    failed: usize,
}

#[cfg(not(target_arch = "wasm32"))]
fn reopen(s: &mut Session, c: &Connection, password: &str) -> Reopened {
    let Some(key) = crate::connections::database_key(&c.address) else { return Reopened::default() };
    let mut result = Reopened::default();
    let mut first_error = None;
    for layer in s.project.maps.iter_mut().flat_map(|m| m.layers.iter_mut()) {
        let Some(src) = layer.source.as_mut().filter(|src| src.login) else { continue };
        let Some((kind, database)) = crate::connections::layer_database(src) else { continue };
        if kind != key.0 || crate::connections::database_key(&crate::sources::redact(database)).as_ref() != Some(&key) {
            continue;
        }
        if let Some(password_key) = crate::connections::database_key(database) {
            rustgis_data::source::duckdb::remember_password(&password_key.1, password);
        }
        src.error = None;
        match src.open() {
            Ok(()) => result.opened += 1,
            Err(e) => {
                result.failed += 1;
                first_error.get_or_insert(e);
            }
        }
    }
    if result.opened > 0 {
        s.notify(format!("{} layer(s) from the connection '{}' opened again", result.opened, c.name), false);
    }
    if result.failed > 0 {
        s.notify(
            format!(
                "{} layer(s) from the connection '{}' still can't be read: {}",
                result.failed,
                c.name,
                first_error.as_deref().unwrap_or("")
            ),
            true,
        );
    }
    result
}

fn login(s: &mut Session, c: &Connection, p: &Value) -> Result<Reopened, String> {
    if DATABASE_KINDS.contains(&c.kind.as_str()) {
        #[cfg(target_arch = "wasm32")]
        {
            return Err(DESKTOP_ONLY.into());
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let pw = ps(p, "password").filter(|t| !t.is_empty()).ok_or("password is required")?;
            super::layer::database_param(&json!({"database": c.address, "password": pw}));
            return Ok(reopen(s, c, pw));
        }
    }
    if c.kind != "s3" {
        return Err(format!("{} connections don't log in yet", c.kind));
    }
    let mut q = json!({"name": c.name, "endpoint": c.address});
    for key in ["region", "buckets"] {
        if let Some(v) = c.options.get(key).filter(|v| !v.is_empty()) {
            q[key] = json!(v);
        }
    }
    match c.options.get("credentials").map(String::as_str).unwrap_or("none") {
        "keys" => {
            if ["accessKeyId", "secretAccessKey"].iter().any(|k| ps(p, k).is_none_or(|v| v.trim().is_empty())) {
                return Err("accessKeyId and secretAccessKey are required: an S3 connection's keys are typed each session".into());
            }
            for key in ["accessKeyId", "secretAccessKey", "sessionToken"] {
                if let Some(v) = p.get(key) {
                    q[key] = v.clone();
                }
            }
        }
        "profile" => {
            let profile = c.options.get("profile").filter(|v| !v.trim().is_empty()).ok_or("the connection names no AWS profile")?;
            q["profile"] = json!(profile);
        }
        "environment" => q["environment"] = json!(true),
        _ => q["anonymous"] = json!(true),
    }
    super::s3::connect(s, &q)?;
    Ok(Reopened::default())
}

fn login_command(s: &mut Session, p: &Value) -> CmdResult {
    let c = found(s, p)?.clone();
    let result = login(s, &c, p)?;
    let mut out = shown(&c);
    if result.opened + result.failed > 0 {
        out["reopened"] = json!(result.opened);
        out["failed"] = json!(result.failed);
    }
    Ok(out)
}

fn tested(s: &Session, p: &Value) -> Result<Connection, String> {
    if ps(p, "id").is_some() {
        return Ok(found(s, p)?.clone());
    }
    let c = Connection {
        kind: ps(p, "kind").ok_or_else(|| format!("id, or kind and address, is required ({})", KINDS.join(", ")))?.trim().into(),
        address: ps(p, "address").unwrap_or("").into(),
        name: ps(p, "name").unwrap_or("").into(),
        group: ps(p, "group").unwrap_or("").into(),
        options: p.get("options").map(options).transpose()?.unwrap_or_default(),
        ..Connection::default()
    };
    Ok(crate::connections::clean(c)?.0)
}

fn first_bucket(c: &Connection) -> Option<String> {
    c.options.get("buckets")?.split(',').map(str::trim).find(|b| !b.is_empty()).map(str::to_owned)
}

fn probe_url(s: &Session, c: &Connection) -> Result<Option<String>, String> {
    let url = match c.kind.as_str() {
        "postgres" | "mysql" | "sqlserver" => return Ok(None),
        "service" => rustgis_data::ogc::capabilities_url(&c.address, None),
        "wms" | "wmts" | "wfs" | "wcs" | "ogcapi" | "arcgis" => rustgis_data::ogc::capabilities_url(&c.address, Some(&c.kind)),
        "csw" => super::service::records_request(s, &json!({"url": c.address}))?,
        "stac" => {
            let api = stac::api(Some(&c.address))?;
            if stac::catalog::is_static(&api) { api } else { stac::collections_url(&api) }
        }
        "s3" => {
            let conn = s3::connection(&c.name).ok_or("log in first: connection.login {id, accessKeyId, secretAccessKey}")?;
            if let Some(bucket) = first_bucket(c) {
                s3::grant(&conn.name, &bucket);
                s3::list_url(&bucket, "", None, Some(&conn))
            } else if conn.credentials.is_some() {
                s3::buckets_url(&conn)
            } else {
                return Err("name a bucket under Buckets to test an anonymous S3 connection".into());
            }
        }
        _ => return Err(format!("unknown kind '{}'", c.kind)),
    };
    Ok(Some(url))
}

fn answered(s: &mut Session, c: &Connection, text: &str) -> CmdResult {
    match c.kind.as_str() {
        "service" => super::service::layers(s, &json!({"url": c.address, "text": text})),
        "wms" | "wmts" | "wfs" | "wcs" | "ogcapi" | "arcgis" => {
            super::service::layers(s, &json!({"url": c.address, "type": c.kind, "text": text}))
        }
        "csw" => super::service::catalog_search(s, &json!({"url": c.address, "text": text})),
        "stac" => super::stac::collections_json(&stac::api(Some(&c.address))?, text.as_bytes()),
        "s3" => {
            if let Some(bucket) = first_bucket(c) {
                let l = s3::parse_list(text.as_bytes())?;
                Ok(json!({"bucket": bucket, "count": l.prefixes.len() + l.objects.len(), "truncated": l.next.is_some()}))
            } else {
                Ok(json!({"buckets": s3::parse_buckets(text.as_bytes())?}))
            }
        }
        _ => Err(format!("unknown kind '{}'", c.kind)),
    }
}

fn test(s: &mut Session, p: &Value) -> CmdResult {
    let c = tested(s, p)?;
    let secrets = ["password", "accessKeyId", "secretAccessKey", "sessionToken"].iter().any(|k| ps(p, k).is_some_and(|v| !v.is_empty()));
    if secrets {
        login(s, &c, p)?;
    }
    if c.kind == "s3" && !secrets && c.options.get("credentials").is_none_or(|v| v != "keys") {
        login(s, &c, &Value::Null)?;
    }
    let mut out = json!({});
    if !c.id.is_empty() {
        out["connection"] = json!(c.id);
    }
    if b(p, "probe").unwrap_or(false) {
        out["fetch"] = json!(probe_url(s, &c)?);
        return Ok(out);
    }
    if let Some(text) = ps(p, "answer")
        && !DATABASE_KINDS.contains(&c.kind.as_str())
    {
        out["result"] = answered(s, &c, text)?;
        return Ok(out);
    }
    let result = match c.kind.as_str() {
        "postgres" | "mysql" | "sqlserver" => {
            if cfg!(target_arch = "wasm32") {
                return Err(DESKTOP_ONLY.into());
            }
            super::layer::database_tables(s, &json!({"database": c.address}))?
        }
        "service" => super::service::layers(s, &json!({"url": c.address}))?,
        "wms" | "wmts" | "wfs" | "wcs" | "ogcapi" | "arcgis" => super::service::layers(s, &json!({"url": c.address, "type": c.kind}))?,
        "csw" => super::service::catalog_search(s, &json!({"url": c.address}))?,
        "stac" => super::stac::collections(s, &json!({"url": c.address}))?,
        "s3" => {
            s3::connection(&c.name).ok_or("log in first: connection.login {id, accessKeyId, secretAccessKey}")?;
            let mut q = json!({"connection": c.name, "limit": 100});
            if let Some(bucket) = first_bucket(&c) {
                q["bucket"] = json!(bucket);
            }
            super::s3::list(s, &q)?
        }
        _ => return Err(format!("unknown kind '{}'", c.kind)),
    };
    out["result"] = result;
    Ok(out)
}
