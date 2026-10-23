//! `connection.browse`: the datasets a saved connection offers, one level at a time, each with
//! the command that adds it.
//!
//! The app reads over the network in the background, so browsing is a probe/answer pair like
//! `connection.test`: `probe` names the address a level reads, the app fetches it and asks again
//! with the `answer`. Without an answer, the command line and the assistant read it themselves.

use serde_json::{Value, json};
use std::borrow::Cow;

use super::super::{b, ps};
use crate::connections::{Connection, DESKTOP_ONLY};
use crate::render::rustgis_tiles::{address, s3, stac};
use crate::{CmdResult, Session};

/// The most children of one level that are listed.
const MAX_CHILDREN: usize = 500;
/// The items of a STAC collection that are listed.
const ITEMS: usize = 20;
/// The longest node a caller may give.
const MAX_NODE: usize = 4096;

/// What a node of a connection stands for.
enum Level {
    Database,
    Service { url: String, kind: Option<String> },
    Catalogue,
    StacRoot { api: String },
    StacCollection { api: String, id: String },
    StacCatalog { url: String },
    StacItem { url: String },
    S3Root,
    S3Prefix { bucket: String, prefix: String },
}

/// The types a `service:<type>:<url>` node names (`auto` leaves the type to be guessed).
const SERVICE_TYPES: [&str; 7] = ["auto", "wms", "wmts", "wfs", "wcs", "ogcapi", "arcgis"];

/// The level a connection's `node` names (its top level when empty).
fn level(c: &Connection, node: &str) -> Result<Level, String> {
    let kind = c.kind.as_str();
    let not_a_node = || format!("'{node}' isn't a node of {}", c.name);
    if node.is_empty() {
        return Ok(match kind {
            "postgres" | "mysql" | "sqlserver" => {
                if cfg!(target_arch = "wasm32") {
                    return Err(DESKTOP_ONLY.into());
                }
                Level::Database
            }
            "service" => Level::Service { url: c.address.clone(), kind: None },
            "wms" | "wmts" | "wfs" | "wcs" | "ogcapi" | "arcgis" => Level::Service { url: c.address.clone(), kind: Some(c.kind.clone()) },
            "csw" => Level::Catalogue,
            "stac" => Level::StacRoot { api: stac::api(Some(&c.address))? },
            "s3" => Level::S3Root,
            _ => return Err(format!("unknown kind '{}'", c.kind)),
        });
    }
    if let Some(rest) = node.strip_prefix("service:") {
        if !matches!(kind, "service" | "wms" | "wmts" | "wfs" | "wcs" | "ogcapi" | "arcgis" | "csw") {
            return Err(not_a_node());
        }
        let (t, url) = rest.split_once(':').filter(|(t, _)| SERVICE_TYPES.contains(t)).ok_or_else(not_a_node)?;
        // Judged as a document of the connection would be: a link may not lead into the local network.
        address::check(url, Some(&c.address))?;
        return Ok(Level::Service { url: url.to_string(), kind: (t != "auto").then(|| t.to_string()) });
    }
    if let Some(id) = node.strip_prefix("collection:") {
        if kind != "stac" || id.is_empty() {
            return Err(not_a_node());
        }
        let api = stac::api(Some(&c.address))?;
        if stac::catalog::is_static(&api) {
            return Err(not_a_node());
        }
        return Ok(Level::StacCollection { api, id: id.to_string() });
    }
    if let Some(url) = node.strip_prefix("catalog:") {
        if kind != "stac" {
            return Err(not_a_node());
        }
        address::check(url, Some(&c.address))?;
        return Ok(Level::StacCatalog { url: url.to_string() });
    }
    if let Some(url) = node.strip_prefix("item:") {
        // A static catalog's item: its own file, read when opened.
        if kind != "stac" {
            return Err(not_a_node());
        }
        address::check(url, Some(&c.address))?;
        return Ok(Level::StacItem { url: url.to_string() });
    }
    if let Some(rest) = node.strip_prefix("s3:") {
        let (bucket, prefix) = rest
            .split_once('/')
            .filter(|(bucket, prefix)| kind == "s3" && s3::valid_bucket(bucket) && prefix.len() <= 1024)
            .ok_or_else(not_a_node)?;
        return Ok(Level::S3Prefix { bucket: bucket.to_string(), prefix: prefix.to_string() });
    }
    Err(not_a_node())
}

/// The session's S3 connection for an S3 level, logging in with what needs no typing.
/// (A probe logs in too, as `connection.test` does: the address it names, and the signature the
/// fetch needs, depend on the connection.)
fn s3_login(s: &mut Session, c: &Connection, level: &Level) -> Result<Option<s3::Connection>, String> {
    if !matches!(level, Level::S3Root | Level::S3Prefix { .. }) {
        return Ok(None);
    }
    if s3::connection(&c.name).is_none() && c.options.get("credentials").is_none_or(|v| v != "keys") {
        super::login(s, c, &Value::Null)?;
    }
    let conn = s3::connection(&c.name).ok_or("log in first: connection.login {id, accessKeyId, secretAccessKey}")?;
    if let Level::S3Prefix { bucket, .. } = level {
        s3::grant(&conn.name, bucket);
    }
    Ok(Some(conn))
}

/// The address a level reads (`None`: nothing is fetched, or the database driver reads it).
fn fetch_url(s: &Session, c: &Connection, level: &Level, conn: Option<&s3::Connection>) -> Result<Option<String>, String> {
    Ok(match level {
        Level::Database => None,
        Level::Service { url, kind } => Some(rustgis_data::ogc::capabilities_url(url, kind.as_deref())),
        Level::Catalogue => Some(super::super::service::records_request(s, &json!({"url": c.address}))?),
        Level::StacRoot { api } => Some(if stac::catalog::is_static(api) { api.clone() } else { stac::collections_url(api) }),
        Level::StacCollection { api, id } => {
            Some(stac::search_url(api, &stac::Search { collections: vec![id.clone()], limit: ITEMS, ..stac::Search::default() })?)
        }
        Level::StacCatalog { url } | Level::StacItem { url } => Some(url.clone()),
        Level::S3Root => match (super::first_bucket(c), conn) {
            (Some(_), _) => None,
            (None, Some(conn)) if conn.credentials.is_some() => Some(s3::buckets_url(conn)),
            _ => return Err("name a bucket under Buckets to browse an anonymous S3 connection".into()),
        },
        Level::S3Prefix { bucket, prefix } => Some(s3::list_url(bucket, prefix, None, conn)),
    })
}

/// What a level reads, fetched here (waiting for it).
#[cfg(not(target_arch = "wasm32"))]
fn download(url: &str) -> Result<String, String> {
    let bytes = crate::render::rustgis_tiles::cache::download_blocking(url)?;
    Ok(String::from_utf8(bytes).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned()))
}

#[cfg(target_arch = "wasm32")]
fn download(_: &str) -> Result<String, String> {
    Err("in the web app, expand the connection under Data Sources › Connections".into())
}

/// The children of a level, and what is known of the rest.
#[derive(Default)]
struct Found {
    children: Vec<Value>,
    truncated: bool,
    matched: Option<u64>,
}

impl Found {
    /// Keeps the first [`MAX_CHILDREN`] of a list (collected with one more, to know it was cut).
    fn new(mut children: Vec<Value>) -> Found {
        let truncated = children.len() > MAX_CHILDREN;
        children.truncate(MAX_CHILDREN);
        Found { children, truncated, matched: None }
    }
}

/// A node that fits what [`browse`] accepts.
fn node_ok(node: &str) -> bool {
    node.len() <= MAX_NODE && !node.chars().any(char::is_control)
}

pub(super) fn browse(s: &mut Session, p: &Value) -> CmdResult {
    let c = super::found(s, p)?.clone();
    let node = ps(p, "node").unwrap_or("");
    if !node_ok(node) {
        return Err("node is a child's node, as connection.browse lists it".into());
    }
    // The app's answers come from a fetch the cache already limits; a caller's may be any size.
    let documents = p.get("documents").and_then(Value::as_array).into_iter().flatten().filter_map(|d| ps(d, "text")).map(str::len);
    if ps(p, "answer").map(str::len).into_iter().chain(documents).sum::<usize>() > super::super::service::MAX_GROUP_BYTES {
        return Err("answer and documents are too large to read".into());
    }
    let level = level(&c, node)?;
    let conn = s3_login(s, &c, &level)?;
    let url = fetch_url(s, &c, &level, conn.as_ref())?;
    let mut out = json!({"connection": c.id, "node": node});
    if b(p, "probe").unwrap_or(false) {
        out["fetch"] = json!(url);
        return Ok(out);
    }
    let found = match &level {
        Level::Database => database(&c)?,
        Level::Service { url, kind } => service(s, p, &mut out, url, kind.as_deref())?,
        Level::S3Root if url.is_none() => {
            buckets(c.options.get("buckets").map(String::as_str).unwrap_or("").split(',').map(str::to_string))
        }
        _ => {
            let at = url.as_deref().unwrap_or("");
            let text: Cow<str> = match ps(p, "answer") {
                Some(a) => Cow::Borrowed(a),
                None => Cow::Owned(download(at)?),
            };
            let bytes = text.as_bytes();
            match &level {
                Level::Catalogue => catalogue(s, &c, &text, &mut out)?,
                Level::StacRoot { api } if stac::catalog::is_static(api) => catalogs(bytes, api)?,
                Level::StacRoot { .. } => collections(bytes)?,
                Level::StacCatalog { url } => catalogs(bytes, url)?,
                Level::StacItem { url } => item_file(bytes, url)?,
                Level::StacCollection { .. } => items(bytes, at)?,
                Level::S3Root => buckets(s3::parse_buckets(bytes)?.into_iter()),
                Level::S3Prefix { bucket, prefix } => objects(bytes, bucket, prefix)?,
                Level::Database | Level::Service { .. } => Found::default(),
            }
        }
    };
    out["children"] = json!(found.children);
    if found.truncated {
        out["truncated"] = json!(true);
    }
    if let Some(m) = found.matched {
        out["matched"] = json!(m);
    }
    Ok(out)
}

/// A live database's geometry tables, under their schemas.
fn database(c: &Connection) -> Result<Found, String> {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = c;
        Err(DESKTOP_ONLY.into())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::collections::BTreeMap;

        let tables = super::super::layer::database_table_details(&json!({"database": c.address}))?;
        let text = |t: &Value, k: &str| t.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let mut schemas: BTreeMap<String, Vec<Value>> = BTreeMap::new();
        let truncated = tables.len() > MAX_CHILDREN;
        for t in tables.iter().take(MAX_CHILDREN) {
            let (label, name) = (text(t, "label"), text(t, "name"));
            let shown = if label.contains(':') { format!("{name}:{}", text(t, "column")) } else { name };
            schemas.entry(text(t, "schema")).or_default().push(json!({
                "key": label,
                "name": shown,
                "what": "table",
                "detail": text(t, "kind"),
                "add": {"command": "layer.addDatabase", "params": {"database": c.address, "layer": label}},
                "options": true,
            }));
        }
        let children = schemas
            .into_iter()
            .map(|(s, tables)| json!({"key": format!("schema:{s}"), "name": s, "what": "schema", "children": tables}))
            .collect();
        Ok(Found { children, truncated, matched: None })
    }
}

/// A service's layers (capabilities read here, or the app's `answer` and `documents`).
fn service(s: &mut Session, p: &Value, out: &mut Value, url: &str, kind: Option<&str>) -> Result<Found, String> {
    let mut q = json!({"url": url});
    if let Some(k) = kind {
        q["type"] = json!(k);
    }
    if let Some(a) = ps(p, "answer") {
        q["text"] = json!(a);
        if let Some(d) = p.get("documents") {
            q["documents"] = d.clone();
        }
    }
    let r = super::super::service::layers(s, &q)?;
    out["service"] = json!({"url": url, "type": kind.unwrap_or("auto")});
    if let Some(n) = r.get("notes").filter(|n| n.as_array().is_some_and(|a| !a.is_empty())) {
        out["notes"] = n.clone();
    }
    if let Some(next) = r.get("next").filter(|n| !n.is_null()) {
        out["next"] = next.clone();
    }
    // The dialog imports an OGC API's collections together: a features layer isn't added alone there.
    let grouped = r.get("kind").and_then(Value::as_str).is_some_and(|k| k.starts_with("OGC API"));
    let text = |l: &Value, k: &str| l.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let children = r
        .get("layers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(MAX_CHILDREN + 1)
        .map(|l| {
            let (name, title) = (text(l, "name"), text(l, "title"));
            let offer = l.get("offer").cloned().unwrap_or(Value::Null);
            let offer_type = offer.get("type").and_then(Value::as_str).unwrap_or("");
            if offer_type == "service" {
                let node = format!("service:arcgis:{}", text(&offer, "url"));
                if !node_ok(&node) {
                    return Value::Null;
                }
                let what = if name.ends_with('/') { "folder" } else { "service" };
                return json!({"key": node, "node": node, "name": title.trim_end_matches('/'), "what": what});
            }
            let what = match offer_type {
                "vectorTiles" => "vectorTiles",
                "features" => "features",
                "coverage" => "coverage",
                "style" => "style",
                _ => "map",
            };
            let source = text(l, "source_url");
            let mut params = json!({"url": url, "layer": name, "source_url": source});
            if let Some(k) = kind {
                params["type"] = json!(k);
            }
            let mut child = json!({
                "key": format!("{source}#{name}"),
                "name": if title.is_empty() { name.as_str() } else { title.as_str() },
                "what": what,
                "detail": name,
                "layer": l,
                "options": true,
            });
            if !(grouped && what == "features") {
                child["add"] = json!({"command": "service.add", "params": params});
            }
            child
        })
        .collect::<Vec<Value>>();
    // A folder or service whose address is too long to follow as a node is left out, and said so.
    let listed = children.len();
    let children: Vec<Value> = children.into_iter().filter(|c| !c.is_null()).collect();
    let left_out = children.len() < listed;
    let mut found = Found::new(children);
    found.truncated |= left_out;
    Ok(found)
}

/// A catalogue's records, each with the services and files it links to.
fn catalogue(s: &mut Session, c: &Connection, text: &str, out: &mut Value) -> Result<Found, String> {
    let r = super::super::service::catalog_search(s, &json!({"url": c.address, "text": text}))?;
    if let Some(n) = r.get("notes") {
        out["notes"] = n.clone();
    }
    let str_of = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let mut dropped = false;
    let children = r
        .get("records")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(MAX_CHILDREN + 1)
        .enumerate()
        .map(|(i, rec)| {
            let id = str_of(rec, "id");
            let title = str_of(rec, "title");
            let links: Vec<Value> = rec
                .get("links")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|l| {
                    let (url, kind, name) = (str_of(l, "url"), str_of(l, "kind"), str_of(l, "name"));
                    match kind.as_str() {
                        "wms" | "wmts" | "wfs" | "wcs" | "ogcapi" | "arcgis" => {
                            let node = format!("service:{kind}:{url}");
                            if !node_ok(&node) {
                                dropped = true;
                                return None;
                            }
                            Some(json!({"key": node, "node": node, "name": if name.is_empty() { url.as_str() } else { name.as_str() }, "what": "service", "detail": kind}))
                        }
                        "download" => {
                            let last = url.split(['?', '#']).next().unwrap_or("").trim_end_matches('/').rsplit('/').next().unwrap_or("");
                            let shown: String = if name.is_empty() { last } else { name.as_str() }.chars().take(200).collect();
                            let named: String = title.chars().take(200).collect();
                            Some(json!({
                                "key": url,
                                "name": shown,
                                "what": "download",
                                "add": {"command": "layer.addUrl", "params": {"url": url, "name": named}},
                            }))
                        }
                        _ => None,
                    }
                })
                .collect();
            let summary: String = str_of(rec, "abstract").chars().take(200).collect();
            json!({"key": format!("record:{}", if id.is_empty() { i.to_string() } else { id }), "name": title, "what": "record", "detail": summary, "children": links})
        })
        .collect();
    let mut found = Found::new(children);
    found.matched = r.get("matched").and_then(Value::as_u64);
    found.truncated |= dropped || r.get("next").is_some_and(|n| !n.is_null());
    Ok(found)
}

/// A STAC API's collections.
fn collections(bytes: &[u8]) -> Result<Found, String> {
    let list = stac::parse_collections(bytes)?;
    Ok(Found::new(
        list.into_iter()
            .take(MAX_CHILDREN + 1)
            .map(|c| {
                let node = format!("collection:{}", c.id);
                json!({"key": node, "node": node, "name": if c.title.is_empty() { c.id.as_str() } else { c.title.as_str() }, "what": "collection", "detail": c.id, "options": true})
            })
            .collect(),
    ))
}

/// What a static catalog's document lists below it: its catalogs and collections (`child` links)
/// and its items (`item` links), each a node read when it is opened (an item's file holds its
/// assets).
fn catalogs(bytes: &[u8], from: &str) -> Result<Found, String> {
    let mut dropped = false;
    let mut children = Vec::new();
    for (prefix, what, list) in [
        ("catalog", "catalog", stac::catalog::parse_children(bytes, from)?),
        ("item", "item", stac::catalog::parse_item_links(bytes, from)?),
    ] {
        for c in list {
            let node = format!("{prefix}:{}", c.url);
            if node_ok(&node) {
                children.push(json!({"key": node, "node": node, "name": c.title, "what": what}));
            } else {
                dropped = true;
            }
        }
    }
    let mut found = Found::new(children);
    found.truncated |= dropped;
    Ok(found)
}

/// The assets of an item that can be added, each with the `stac.addAsset` that adds it.
fn assets_of(item: &stac::Item) -> Vec<Value> {
    item.assets
        .iter()
        .take(MAX_CHILDREN)
        .filter_map(|a| {
            let (key, href) = item.asset_href(Some(&a.key)).ok()?;
            let mut params = json!({"asset": key, "name": format!("{} {key}", item.id)});
            match &item.url {
                Some(u) => params["url"] = json!(u),
                None => params["item"] = stac::item_json_with(item, false),
            }
            Some(json!({
                "key": format!("{}#{key}", item.url.as_deref().unwrap_or(&item.id)),
                "name": if a.title.is_empty() { key.as_str() } else { a.title.as_str() },
                "what": "asset",
                "detail": a.media_type,
                "href": href,
                "add": {"command": "stac.addAsset", "params": params},
            }))
        })
        .collect()
}

/// A static catalog's item (its own file): its assets.
fn item_file(bytes: &[u8], from: &str) -> Result<Found, String> {
    Ok(Found::new(assets_of(&stac::parse_item(bytes, from)?)))
}

/// The items of a collection's search, each with the assets that can be added.
fn items(bytes: &[u8], from: &str) -> Result<Found, String> {
    let (list, next) = stac::parse_items(bytes, from)?;
    let matched = stac::parse_matched(bytes);
    let truncated = next.is_some() || matched.is_some_and(|m| m > list.len() as u64);
    let children = list
        .iter()
        .take(MAX_CHILDREN)
        .map(|item| json!({"key": format!("item:{}", item.url.as_deref().unwrap_or(&item.id)), "name": item.id, "what": "item", "detail": item.datetime, "children": assets_of(item)}))
        .collect();
    Ok(Found { children, truncated, matched })
}

/// Buckets to browse, each a node.
fn buckets(names: impl Iterator<Item = String>) -> Found {
    Found::new(
        names
            .map(|n| n.trim().to_string())
            .filter(|n| s3::valid_bucket(n))
            .take(MAX_CHILDREN + 1)
            .map(|n| {
                let node = format!("s3:{n}/");
                json!({"key": node, "node": node, "name": n, "what": "bucket"})
            })
            .collect(),
    )
}

/// One level of a bucket: its folders (each a node) and its files.
fn objects(bytes: &[u8], bucket: &str, prefix: &str) -> Result<Found, String> {
    let list = s3::parse_list(bytes)?;
    let last = |k: &str| k.trim_end_matches('/').rsplit('/').next().unwrap_or("").to_string();
    let folders = list.prefixes.iter().map(|p| {
        let node = format!("s3:{bucket}/{p}");
        json!({"key": node, "node": node, "name": format!("{}/", last(p)), "what": "prefix"})
    });
    let files = list.objects.iter().filter(|o| o.key != prefix && !o.key.ends_with('/')).map(|o| {
        let url = format!("s3://{bucket}/{}", o.key);
        json!({
            "key": url,
            "name": last(&o.key),
            "what": "object",
            "detail": o.size.to_string(),
            "add": {"command": "layer.addUrl", "params": {"url": url}},
        })
    });
    let mut found = Found::new(folders.chain(files).take(MAX_CHILDREN + 1).collect());
    found.truncated |= list.next.is_some();
    Ok(found)
}
