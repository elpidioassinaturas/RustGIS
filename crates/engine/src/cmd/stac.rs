//! STAC: an API's collections, a search for items (with their footprints as a layer), and adding
//! an item's asset by its web address.

use serde_json::{Value, json};

use super::{b, f, ps};
use crate::render::rustgis_tiles::stac::{self, Item, Search};
use crate::{CmdResult, CommandSpec as C, Session};

pub fn commands() -> Vec<C> {
    vec![
        C::new("stac.collections", "List STAC Collections", "Home › Add › Web Data › STAC Search", collections)
            .view()
            .params(r#"{"url"?: "a STAC API (default: Earth Search, https://earth-search.aws.element84.com/v1; Planetary Computer: https://planetarycomputer.microsoft.com/api/stac/v1), or a static catalog's catalog.json or collection.json (its child catalogs and collections are listed)"}"#),
        C::new("stac.search", "STAC Search", "Home › Add › Web Data › STAC Search", search).params(
            r#"{"url"?: "a STAC API (default: Earth Search), or a static catalog's catalog.json or collection.json (walked from there, at most 400 documents)", "collections"?: id | [ids] (APIs), "bbox"?: [west, south, east, north] | "view" (the map's extent), "datetime"?: "2024-06-01" | "2024-06-01/2024-06-30", "limit"?: 1–200 (default 10), "maxCloud"?: percent, "sortby"?: "eo:cloud_cover" | "-datetime" (APIs that sort), "phase"?: "pre" | "post" (static catalogs of events), "properties"?: bool (every property of each item, not only its date and cloud cover; default false), "footprints"?: bool (adds the items' footprints as a layer, default true)}"#,
        ),
        C::new("stac.addFootprints", "Add STAC Footprints", "Home › Add › Web Data › STAC Search", add_footprints_cmd)
            .params(r#"{"footprints": GeoJSON FeatureCollection (items' footprints, as STAC Search finds them), "name"?: text}"#),
        C::new("stac.addCube", "Add STAC Data Cube", "Home › Add › Web Data › STAC Search", add_cube).params(
            r#"{"url"?: a STAC API (default: Earth Search), "collection": id, "asset": key (a band, such as NDVI), "bbox"?: [west, south, east, north] | "view" (default: the map's extent), "start", "end": dates, "maxCloud"?: percent, "range"?: [low, high] (the values shown from black to white, in the asset's own units after its raster:bands scale and offset, the same for every date and tile; default: each file's 2nd–98th percentile), "name"?: text} → a tile layer per tile of the grid the items cover, each a file per date following the time slider, which turns on over their dates when it is off (desktop)"#,
        ),
        C::new("stac.addAsset", "Add STAC Asset", "Home › Add › Web Data › STAC Search", add_asset).params(
            r#"{"item": a STAC item (as stac.search returns it) | "url": an item's address (Planetary Computer assets are signed), "asset"?: key (default: visual, else the first GeoTIFF, else the first data asset), "name"?: text}"#,
        ),
    ]
}

/// The search a command describes (`view` is the map's current extent).
pub fn search_params(s: &Session, p: &Value) -> Result<Search, String> {
    let collections = match p.get("collections") {
        Some(Value::String(c)) => c.split(',').map(|c| c.trim().to_string()).filter(|c| !c.is_empty()).collect(),
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(str::to_string).collect(),
        _ => Vec::new(),
    };
    let bbox = match p.get("bbox") {
        Some(Value::String(v)) if v == "view" => Some(view(s)?),
        Some(v @ Value::Array(_)) => {
            let n: Vec<f64> = v.as_array().into_iter().flatten().filter_map(Value::as_f64).collect();
            match n.as_slice() {
                [w, south, e, n] => Some([*w, *south, *e, *n]),
                _ => return Err("bbox is [west, south, east, north], or \"view\"".into()),
            }
        }
        _ if b(p, "view").unwrap_or(false) => Some(view(s)?),
        _ => None,
    };
    Ok(Search {
        collections,
        bbox,
        datetime: ps(p, "datetime").map(str::to_string),
        limit: f(p, "limit").map(|l| l.clamp(1.0, stac::MAX_ITEMS as f64) as usize).unwrap_or(10),
        max_cloud: f(p, "maxCloud"),
        sortby: ps(p, "sortby").map(str::to_string),
    })
}

/// The map's extent in longitude/latitude, within the globe.
fn view(s: &Session) -> Result<[f64; 4], String> {
    let e = super::workspace::view_extent(s);
    if e.is_empty() {
        return Err("the map's extent isn't known; give bbox".into());
    }
    Ok([e.min_x.max(-180.0), e.min_y.max(-90.0), e.max_x.min(180.0), e.max_y.min(90.0)])
}

/// Adds items' footprints as a polygon layer; its id.
pub fn add_footprints(s: &mut Session, items: &[Item], name: &str) -> Result<Option<u32>, String> {
    add_collection(s, &stac::footprints_geojson(items), name)
}

/// `stac.addFootprints`: the footprints the app found (a GeoJSON FeatureCollection) as a layer,
/// through the command so the change is undoable like any other.
fn add_footprints_cmd(s: &mut Session, p: &Value) -> CmdResult {
    let fc = p.get("footprints").ok_or("footprints is a GeoJSON FeatureCollection")?;
    let name = ps(p, "name").map(str::trim).filter(|n| !n.is_empty()).unwrap_or("STAC footprints");
    match add_collection(s, fc, name)? {
        Some(id) => Ok(serde_json::json!({"layer": id})),
        None => Err("no footprints to add".into()),
    }
}

pub(crate) fn add_collection(s: &mut Session, fc: &Value, name: &str) -> Result<Option<u32>, String> {
    if fc["features"].as_array().is_none_or(Vec::is_empty) {
        return Ok(None);
    }
    let text = fc.to_string();
    if text.len() > 64 << 20 {
        return Err("the footprints are too large".into());
    }
    let (ds, _) = rustgis_data::io::read_geojson(&text, name).map_err(|e| e.to_string())?;
    let id = super::layer::add_dataset(s, ds);
    // Outlines over a faint fill, so the map (and the scenes added later) show through.
    if let Some(l) = s.map_mut().layer_mut(id)
        && let rustgis_map::Renderer::Single(sym) = &mut l.renderer
    {
        let [r, g, b, _] = sym.fill.0;
        sym.fill = rustgis_map::Rgba::new(r, g, b, 25);
        sym.stroke = rustgis_map::Rgba::new(r, g, b, 255);
        sym.stroke_width = 1.5;
    }
    Ok(Some(id))
}

fn collections(_s: &mut Session, p: &Value) -> CmdResult {
    let api = stac::api(ps(p, "url"))?;
    #[cfg(target_arch = "wasm32")]
    {
        let _ = api;
        Err("in the web app, browse STAC with Home › Web Data › STAC Search".into())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        // A static catalog: its child catalogs and collections, by their addresses.
        if stac::catalog::is_static(&api) {
            crate::render::rustgis_tiles::address::check(&api, None)?;
            let bytes = crate::render::rustgis_tiles::cache::download_blocking(&api)?;
            let c = stac::catalog::parse_children(&bytes, &api)?;
            return Ok(
                json!({"count": c.len(), "static": true, "collections": c.iter().map(|c| json!({"id": c.url, "title": c.title})).collect::<Vec<_>>()}),
            );
        }
        let bytes = crate::render::rustgis_tiles::cache::download_blocking(&stac::collections_url(&api))?;
        let c = stac::parse_collections(&bytes)?;
        Ok(json!({"count": c.len(), "collections": c.iter().map(|c| json!({"id": c.id, "title": c.title})).collect::<Vec<_>>()}))
    }
}

fn search(s: &mut Session, p: &Value) -> CmdResult {
    let api = stac::api(ps(p, "url"))?;
    if stac::catalog::is_static(&api) {
        return search_static(s, p, &api);
    }
    let url = stac::search_url(&api, &search_params(s, p)?)?;
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (s, url);
        Err("in the web app, search STAC with Home › Web Data › STAC Search".into())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let bytes = crate::render::rustgis_tiles::cache::download_blocking(&url)?;
        let (items, next) = stac::parse_items(&bytes, &url)?;
        let all = b(p, "properties").unwrap_or(false);
        let mut out =
            json!({"count": items.len(), "items": items.iter().map(|i| stac::item_json_with(i, all)).collect::<Vec<_>>(), "next": next});
        if b(p, "footprints").unwrap_or(true)
            && let Some(id) = add_footprints(s, &items, "STAC footprints")?
        {
            out["layer"] = json!(id);
            if let Some(e) = s.map().layer(id).map(|l| l.extent()) {
                s.zoom_to(&e);
            }
        }
        Ok(out)
    }
}

/// A search of a static catalog: a walk from its document, keeping the items that match.
fn search_static(s: &mut Session, p: &Value, root: &str) -> CmdResult {
    crate::render::rustgis_tiles::address::check(root, None)?;
    let search = search_params(s, p)?;
    // A static catalog has no search: it is walked in its own order, and its nodes are its
    // collections.
    let mut ignored = Vec::new();
    if search.sortby.as_deref().is_some_and(|b| !b.trim().is_empty()) {
        ignored.push("sortby");
    }
    if !search.collections.is_empty() {
        ignored.push("collections");
    }
    let note = (!ignored.is_empty()).then(|| {
        format!(
            "{} ignored: a static catalog is walked in its own order (the first items found that match, then sorted by date); give a collection's collection.json as url",
            ignored.join(" and ")
        )
    });
    let filter = stac::catalog::Filter::from_search(&search, ps(p, "phase"))?;
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (s, filter, root, note);
        Err("in the web app, search STAC with Home › Web Data › STAC Search".into())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let walk = stac::catalog::walk_blocking(root, filter, search.limit);
        let complete = walk.complete();
        let (read, failed) = (walk.reads, walk.failed);
        let items = walk.into_items();
        let all = b(p, "properties").unwrap_or(false);
        let mut out = json!({"count": items.len(), "items": items.iter().map(|i| stac::item_json_with(i, all)).collect::<Vec<_>>(),
            "complete": complete, "read": read, "failed": failed});
        if let Some(n) = note {
            out["note"] = json!(n);
        }
        if b(p, "footprints").unwrap_or(true)
            && let Some(id) = add_footprints(s, &items, "STAC footprints")?
        {
            out["layer"] = json!(id);
            if let Some(e) = s.map().layer(id).map(|l| l.extent()) {
                s.zoom_to(&e);
            }
        }
        Ok(out)
    }
}

/// The item a command names: given whole, or by its address.
fn item(p: &Value) -> Result<Item, String> {
    if let Some(v) = p.get("item").filter(|v| v.is_object()) {
        // Given whole (by the assistant or a plugin, maybe from another answer): it came from no
        // address, so its assets may only be public web addresses.
        return stac::item_from_json(v, None).ok_or_else(|| "item isn't a STAC item".to_string());
    }
    let url = ps(p, "url").map(str::trim).filter(|u| !u.is_empty()).ok_or("give item (a STAC item) or url (an item's address)")?;
    #[cfg(target_arch = "wasm32")]
    {
        let _ = url;
        Err("in the web app, add STAC assets with Home › Web Data › STAC Search".into())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let bytes = crate::render::rustgis_tiles::cache::download_blocking(url)?;
        let mut i = stac::parse_item(&bytes, url)?;
        i.url.get_or_insert_with(|| url.to_string());
        Ok(i)
    }
}

fn add_asset(s: &mut Session, p: &Value) -> CmdResult {
    let i = item(p)?;
    let (key, href) = i.asset_href(ps(p, "asset"))?;
    let name = ps(p, "name").map(str::trim).filter(|n| !n.is_empty()).map(str::to_string).unwrap_or_else(|| format!("{} {key}", i.id));
    // A Planetary Computer asset is read with its storage container's token.
    #[cfg(not(target_arch = "wasm32"))]
    let href = stac::sign_blocking(&href)?;
    super::layer::add_url(s, &json!({"url": href, "name": name}))
}

/// The most items a data cube takes (its search is paged up to this).
pub const MAX_CUBE_ITEMS: usize = 2000;
/// The most pages of a search a data cube reads, whatever they hold.
pub const MAX_CUBE_PAGES: usize = 100;

/// `stac.addCube` (opengeos/RustGIS#559): a STAC collection's band as a data cube on the time
/// slider. The items found are grouped by the grid tile they cover (the tile their properties
/// name, else their bounding box) and by day; each tile becomes a tile layer that reads a file
/// per date in place, the slider showing the last date in its window. A range, when given, is
/// the same for every date and tile, so a grey means one value throughout.
fn add_cube(s: &mut Session, p: &Value) -> CmdResult {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (s, p);
        Err("in the web app, a STAC data cube can't be added yet".into())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let api = stac::api(ps(p, "url"))?;
        if stac::catalog::is_static(&api) {
            return Err("a data cube needs a STAC API's search; a static catalog can't be searched".into());
        }
        let collection = ps(p, "collection").map(str::trim).filter(|c| !c.is_empty()).ok_or("collection is required")?;
        let asset = ps(p, "asset").map(str::trim).filter(|a| !a.is_empty()).ok_or("asset is required (a band, such as NDVI)")?;
        let date = |k: &str| {
            p.get(k)
                .and_then(rustgis_data::time::parse_input)
                .ok_or_else(|| format!("{k} is a date (2024-06-01, a year, or milliseconds since 1970)"))
        };
        let (start, mut end) = (date("start")?, date("end")?);
        // An end given as a year or a month is the end of it: "end": "2024" takes all of 2024.
        let given = p.get("end").map(|v| v.as_str().map_or_else(|| v.to_string(), |s| s.trim().to_string())).unwrap_or_default();
        let digits = |s: &str| !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit());
        let next = match given.split_once('-') {
            None if given.len() == 4 && digits(&given) => given.parse::<i32>().ok().map(|y| format!("{}-01-01", y + 1)),
            Some((y, m)) if y.len() == 4 && m.len() == 2 && digits(y) && digits(m) => match (y.parse::<i32>(), m.parse::<u32>()) {
                (Ok(y), Ok(12)) => Some(format!("{}-01-01", y + 1)),
                (Ok(y), Ok(m)) if (1..12).contains(&m) => Some(format!("{y:04}-{:02}-01", m + 1)),
                _ => None,
            },
            _ => None,
        };
        if let Some(after) = next.and_then(|n| rustgis_data::time::parse_text(&n)) {
            end = after - 86_400_000.0;
        }
        if end < start {
            return Err("end is before start".into());
        }
        let range = match p.get("range").filter(|v| !v.is_null()) {
            None => None,
            Some(v) => {
                // Two finite numbers, nothing else: a value that isn't one is refused, not skipped.
                let r: Option<Vec<f64>> =
                    v.as_array().filter(|a| a.len() == 2).and_then(|a| a.iter().map(|x| x.as_f64().filter(|x| x.is_finite())).collect());
                match r.as_deref() {
                    Some([lo, hi]) if lo < hi => Some((*lo, *hi)),
                    _ => return Err("range is [low, high], low below high".into()),
                }
            }
        };
        let day = |ms: f64| rustgis_data::time::format(ms, "YYYY-MM-DD");
        // RFC 3339, as STAC asks: the whole of the last day included.
        let mut q =
            json!({"collections": collection, "datetime": format!("{}T00:00:00Z/{}T23:59:59Z", day(start), day(end)), "limit": 200});
        for k in ["bbox", "maxCloud"] {
            if let Some(v) = p.get(k).filter(|v| !v.is_null()) {
                q[k] = v.clone();
            }
        }
        if q.get("bbox").is_none() {
            q["bbox"] = json!("view");
        }
        let search = search_params(s, &q)?;
        // Every page of the search, up to the cap.
        let mut items: Vec<Item> = Vec::new();
        // An empty page, or a next link back to a page already read, ends it; so does the page cap.
        let mut next = Some(stac::search_url(&api, &search)?);
        let mut read: Vec<String> = Vec::new();
        // Pages are read from the API's own host: a next link elsewhere ends the search.
        // Scheme, user part, host and port compared: `https://host:443`, never another host or http.
        let host = |u: &str| {
            u.split_once("://").map(|(scheme, r)| {
                let authority = r.split(['/', '?', '#']).next().unwrap_or("");
                let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h).to_ascii_lowercase();
                let scheme = scheme.to_ascii_lowercase();
                // The scheme's own port written out is the same host.
                let default = if scheme == "https" { ":443" } else { ":80" };
                format!("{scheme}://{}", host.strip_suffix(default).unwrap_or(&host))
            })
        };
        // An address with no scheme has no host to match: a next link is never taken on it.
        let api_host = host(&api);
        while let Some(url) = next.take() {
            if read.contains(&url) || (!read.is_empty() && (api_host.is_none() || host(&url) != api_host)) {
                break;
            }
            if read.len() >= MAX_CUBE_PAGES {
                return Err(format!("more than {MAX_CUBE_PAGES} pages of results: take a shorter period or a smaller area"));
            }
            let bytes = crate::render::rustgis_tiles::cache::download_blocking(&url)?;
            let (page, more) = stac::parse_items(&bytes, &url)?;
            read.push(url);
            if page.is_empty() {
                break;
            }
            items.extend(page);
            if items.len() > MAX_CUBE_ITEMS {
                return Err(format!("more than {MAX_CUBE_ITEMS} items: take a shorter period or a smaller area"));
            }
            next = more;
        }
        if items.is_empty() {
            return Err(format!("no item of {collection} in the area and period"));
        }
        // By tile, then by day: the first item found for a tile and day (the search's order).
        let mut tiles: std::collections::BTreeMap<String, std::collections::BTreeMap<i64, String>> = Default::default();
        let mut scale: Option<(f64, f64)> = None;
        let mut scales_differ = false;
        let mut duplicates = 0usize;
        let mut without = 0usize;
        for i in &items {
            let Some(ms) = rustgis_data::time::parse_text(&i.datetime) else {
                without += 1;
                continue;
            };
            let Some(a) = i.assets.iter().find(|a| a.key.eq_ignore_ascii_case(asset)) else {
                without += 1;
                continue;
            };
            let key = i.tile.clone().unwrap_or_else(|| match i.bbox {
                Some([w, so, e, n]) => format!("{w:.2},{so:.2},{e:.2},{n:.2}"),
                None => i.id.clone(),
            });
            let d = (ms / 86_400_000.0).floor() as i64;
            let by_day = tiles.entry(key).or_default();
            if by_day.contains_key(&d) {
                // A tile and day already have a file; this one is left out (and counted), its scale too.
                duplicates += 1;
                continue;
            }
            // The file's address without a fragment of its own, so the range look is the only one.
            by_day.insert(d, a.href.split('#').next().unwrap_or(&a.href).to_string());
            // No scale or offset in the metadata reads as 1 and 0, the values as stored.
            let this = (a.scale.unwrap_or(1.0), a.offset.unwrap_or(0.0));
            scales_differ |= scale.is_some_and(|s| s != this);
            scale.get_or_insert(this);
        }
        if tiles.is_empty() {
            return Err(format!("no item of {collection} has an asset {asset}"));
        }
        // Files reached through signed addresses (Planetary Computer's) expire within hours, so a
        // cube of them couldn't be kept in a project or read a day later.
        if tiles.values().flat_map(|by_day| by_day.values()).any(|href| stac::needs_signing(href)) {
            return Err(format!(
                "{collection}'s files are reached through signed addresses that expire, so they can't make a data cube kept in the project: use an API with public files (Earth Search, the Brazil Data Cube)"
            ));
        }
        // One range can't hold if the files store their values alike no longer (a processing
        // change of scale or offset partway).
        if range.is_some() && scales_differ {
            return Err(format!(
                "the {asset} files don't all have the same scale and offset, so one range can't fit them all: leave range out"
            ));
        }
        // The range in the files' stored values.
        let look = match (range, scale) {
            (Some(_), Some((k, o))) if k == 0.0 || !k.is_finite() || !o.is_finite() => {
                return Err(format!(
                    "the {asset} files' metadata gives a scale of {k} and an offset of {o}, so a range in the band's units can't be set: leave range out"
                ));
            }
            (Some((lo, hi)), Some((k, o))) => {
                let (a, b) = ((lo - o) / k, (hi - o) / k);
                format!("#range={},{}", a.min(b), a.max(b))
            }
            (Some((lo, hi)), None) => format!("#range={lo},{hi}"),
            (None, _) => String::new(),
        };
        let base = ps(p, "name").map(str::trim).filter(|n| !n.is_empty()).map_or_else(|| format!("{collection} {asset}"), str::to_string);
        let mut dates: Vec<f64> = Vec::new();
        let many = tiles.len() > 1;
        let mut added = Vec::new();
        // What the map had: a cube that can't be added whole puts its tile layers back as they
        // were (those it replaced, the same cube added before, included) and takes out the layers
        // it added. The rest (the view a file framed, the slider) comes back with the project
        // snapshot the command runs under (`Session::guarded`), as for any command that fails.
        let (tiles_before, layers_before): (Vec<rustgis_map::TileLayer>, Vec<u32>) =
            (s.map().tiles.clone(), s.map().layers.iter().map(|l| l.id).collect());
        let undo = |s: &mut Session| {
            let m = s.map_mut();
            m.tiles = tiles_before.clone();
            m.layers.retain(|l| layers_before.contains(&l.id));
        };
        // A tile layer per grid tile, each reading its first file's header, within the map's room
        // for tile layers: a catalog without grid tiles (keyed by footprint) could make hundreds.
        let room = rustgis_map::tiles::MAX_TILE_LAYERS.saturating_sub(s.map().tiles.len());
        if tiles.len() > room {
            // Items naming no grid tile are told apart by footprint, which can differ scene by scene.
            let why = if items.iter().all(|i| i.tile.is_none()) {
                format!("{collection}'s items name no grid tile, so each footprint counts as one")
            } else {
                "take a smaller area".to_string()
            };
            return Err(format!(
                "{} grid tiles, and the map has room for {room} more tile layers (at most {}): {why}",
                tiles.len(),
                rustgis_map::tiles::MAX_TILE_LAYERS
            ));
        }
        // Every file a web address of sane length, as a project keeps them (TileLayer::repair),
        // so the cube reopens as it was made.
        if let Some(bad) = tiles.values().flat_map(|by_day| by_day.values()).find(|h| {
            let lower = h.to_ascii_lowercase();
            !(lower.starts_with("https://") || lower.starts_with("http://")) || h.len() > 4000
        }) {
            return Err(format!(
                "{}: a data cube's files are read in place from web addresses (http, https)",
                bad.chars().take(200).collect::<String>()
            ));
        }
        for (tile, by_day) in &tiles {
            let urls: Vec<(f64, String)> = by_day.iter().map(|(d, u)| (*d as f64 * 86_400_000.0, format!("{u}{look}"))).collect();
            dates.extend(urls.iter().map(|(d, _)| *d));
            let Some((_, first)) = urls.first() else { continue };
            let name = if many { format!("{base} {tile}") } else { base.clone() };
            let r = match super::layer::add_url(s, &json!({"url": first, "name": name})) {
                Ok(r) => r,
                Err(e) => {
                    undo(s);
                    return Err(e);
                }
            };
            // The tile layer add_url put in (at the index it gives, and the file's address) follows
            // the slider through its dates; a file read whole instead (a GeoTIFF that isn't
            // cloud-optimised) can't.
            let m = s.map_mut();
            let file = first.split('#').next();
            let at = r.get("index").and_then(Value::as_u64).and_then(|i| usize::try_from(i).ok());
            match at.and_then(|i| m.tiles.get_mut(i)).filter(|t| t.url.split('#').next() == file) {
                Some(t) => {
                    t.time_urls = urls;
                    t.name = name;
                    added.push(r);
                }
                None => {
                    undo(s);
                    return Err(format!(
                        "{}: not a cloud-optimised GeoTIFF read in place; a data cube needs those",
                        first.split('#').next().unwrap_or(first)
                    ));
                }
            }
        }
        dates.sort_by(f64::total_cmp);
        dates.dedup();
        // At most MAX_CUBE_ITEMS dates, well under the slider's steps; were it ever more, the cube
        // is refused rather than left with files the slider can't reach.
        if dates.len() > rustgis_map::time::MAX_STEPS {
            undo(s);
            return Err(format!("{} dates; the time slider takes {} at most", dates.len(), rustgis_map::time::MAX_STEPS));
        }
        // The slider over the cube's dates: on over them when it is off; when it is on, taking
        // them in (its own dates and the cube's, or its span widened to theirs), so every file
        // can be reached.
        let slider = match s.map().time.as_ref() {
            None => {
                let d: Vec<Value> = dates.iter().map(|d| json!(d)).collect();
                json!({"dates": d})
            }
            Some(t) if !t.dates.is_empty() => {
                let mut all: Vec<f64> = t.dates.iter().chain(&dates).copied().collect();
                all.sort_by(f64::total_cmp);
                all.dedup();
                if all.len() > rustgis_map::time::MAX_STEPS {
                    undo(s);
                    return Err(format!(
                        "the time slider's dates and the cube's make {}, more than its {} steps: take a shorter period",
                        all.len(),
                        rustgis_map::time::MAX_STEPS
                    ));
                }
                json!({"dates": all, "date": t.current})
            }
            Some(t) => {
                let (first, last) = (dates.first().copied().unwrap_or(t.start), dates.last().copied().unwrap_or(t.end));
                json!({"start": t.start.min(first), "end": t.end.max(last), "date": t.current})
            }
        };
        if let Err(e) = super::time::set(s, &slider) {
            undo(s);
            return Err(e);
        }
        let mut out = json!({"items": items.len(), "tiles": tiles.len(), "dates": dates.len(), "layers": added.len()});
        if without > 0 {
            out["skipped"] = json!(without);
        }
        if duplicates > 0 {
            out["duplicates"] = json!(duplicates);
        }
        Ok(out)
    }
}
