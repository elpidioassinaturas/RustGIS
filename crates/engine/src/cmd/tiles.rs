//! Home › Add › Basemap: web tile layers (XYZ/TMS/WMS raster tiles, vector tiles and
//! MapLibre styles) over the basemap.

use rustgis_map::effects::Blend;
use rustgis_map::tiles::{MAX_TILE_LAYERS, TileKind, TileLayer};
use serde_json::{Value, json};

use super::{b, f, ps};
use crate::{CmdResult, CommandSpec as C, Session};

/// Commands that make the map fetch from an address they're given (a plugin needs the
/// `network` permission to run them).
pub const NETWORK: &[&str] = &[
    "map.route",
    "map.addTiles",
    "layer.addUrl",
    "service.layers",
    "service.add",
    "service.import",
    "service.refresh",
    "service.groupPlan",
    "service.coverage",
    "catalog.search",
    "stac.collections",
    "stac.search",
    "stac.addAsset",
    "map.findPlace",
    "layer.geocode",
    "osm.download",
    "boundaries.add",
    "boundaries.countries",
    "s3.list",
    "overture.add",
    "sourcecoop.products",
    "sourcecoop.list",
    "portal.search",
    "portal.add",
];

pub fn commands() -> Vec<C> {
    vec![
        C::new("map.addTiles", "Add Tile Layer", "Home › Add", add).params(
            r#"{"url": "https://…/{z}/{x}/{y}.png" | vector tiles ".pbf" or TileJSON | MapLibre style.json | a PMTiles archive (web address or file path) | an MBTiles file path, "kind"?: "raster"|"vector"|"style"|"pmtiles"|"mbtiles" (guessed from the url), "name"?: text, "attribution"?: text, "minZoom"?: number, "maxZoom"?: number, "tileSize"?: 256|512, "tms"?: bool, "subdomains"?: [text], "layers"?: [vector tile layer names, drawn in colours of their own], "opacity"?: 0–1, "blend"?: "normal"|"multiply"|"screen"|"overlay"|"darken"|"lighten"|"difference" (how it combines with what is beneath), "overlay"?: bool (a transparent layer kept on top), "replace"?: bool (removes the other tile basemaps, not overlays), "id"?: catalog id, "view"?: bool (a MapLibre style: go to the place, zoom, bearing and pitch its root properties give, once it loads)}"#,
        ),
        C::new("map.removeTiles", "Remove Tile Layer", "Home › Add", remove)
            .params(r#"{"name"?: text, "index"?: number, "id"?: catalog id} (removes every tile layer when none is given)"#),
        C::new("map.setTiles", "Tile Layer Properties", "Home › Add", set).params(
            r#"{"name"?: text | "index"?: number (0 = top) | "id"?: catalog id, "visible"?: bool, "opacity"?: 0–1, "blend"?: "normal"|"multiply"|"screen"|"overlay"|"darken"|"lighten"|"difference", "move"?: "up"|"down"|"top"|"bottom", "rename"?: text}"#,
        ),
        C::new("map.tiles", "Tile Layers", "Home › Add", |s, _| Ok(list(s, false))).view(),
    ]
}

/// The map's tile layers (index 0 on top), as `map.tiles` and `info.project` list them; `redact`
/// drops the credentials from their addresses.
pub(crate) fn list(s: &Session, redact: bool) -> Value {
    Value::Array(
        s.map()
            .tiles
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let url = if redact { crate::sources::redact(&t.url) } else { t.url.clone() };
                json!({"index": i, "name": t.name, "id": t.id, "kind": t.kind.code(), "url": url, "visible": t.visible,
                    "opacity": t.opacity, "blend": t.blend.code(), "overlay": t.overlay, "attribution": t.attribution, "minZoom": t.min_zoom, "maxZoom": t.max_zoom})
            })
            .collect(),
    )
}

pub(crate) fn add(s: &mut Session, p: &Value) -> CmdResult {
    let url = ps(p, "url").map(str::trim).filter(|u| !u.is_empty()).ok_or("url is required")?;
    let kind = match ps(p, "kind") {
        Some(k) => TileKind::from_code(k).ok_or_else(|| format!("unknown kind '{k}' (raster, vector, style, pmtiles or mbtiles)"))?,
        None => TileKind::guess(url),
    };
    let mut t = TileLayer::new(ps(p, "name").unwrap_or_default(), kind, url);
    if let Some(a) = ps(p, "attribution") {
        t.attribution = a.to_string();
    }
    if let Some(z) = f(p, "minZoom") {
        t.min_zoom = z.clamp(0.0, 24.0) as u8;
    }
    if let Some(z) = f(p, "maxZoom") {
        t.max_zoom = z.clamp(0.0, 24.0) as u8;
    } else if kind != TileKind::Raster {
        t.max_zoom = 14;
    }
    if let Some(n) = f(p, "tileSize") {
        t.tile_size = n.clamp(0.0, 4096.0) as u32;
    }
    t.tms = b(p, "tms").unwrap_or(false);
    t.overlay = b(p, "overlay").unwrap_or(false);
    if let Some(o) = f(p, "opacity") {
        t.opacity = o as f32;
    }
    if let Some(m) = blend(p)? {
        t.blend = m;
    }
    // Vector tiles' layer names (an OGC API collection's), each drawn in a colour of its own.
    if let Some(names) = p.get("layers").and_then(Value::as_array) {
        t.layers = names.iter().filter_map(Value::as_str).map(str::to_string).collect();
    }
    if let Some(subs) = p.get("subdomains") {
        t.subdomains = match subs {
            Value::Array(a) => a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect(),
            Value::String(s) => s.chars().map(String::from).collect(),
            _ => Vec::new(),
        };
    }
    if let Some(id) = ps(p, "id") {
        t.id = id.to_string();
    }
    t.repair()?;
    let replace = b(p, "replace").unwrap_or(false);
    let view = b(p, "view").unwrap_or(false);
    if view && t.kind != TileKind::Style {
        return Err("view is read from a MapLibre style's root properties; this is no style".into());
    }
    let url = t.url.clone();
    let m = s.map_mut();
    if replace {
        m.tiles.retain(|o| o.overlay);
    }
    // The same address again replaces the old one.
    m.tiles.retain(|o| !(o.url == t.url && o.kind == t.kind));
    if m.tiles.len() >= MAX_TILE_LAYERS {
        return Err(format!("a map holds at most {MAX_TILE_LAYERS} tile layers"));
    }
    // Overlays go on top; basemaps go on top of the other basemaps, under the overlays.
    let at = if t.overlay { 0 } else { m.tiles.iter().position(|o| !o.overlay).unwrap_or(m.tiles.len()) };
    let out = json!({"name": t.name, "kind": t.kind.code(), "index": at});
    m.tiles.insert(at, t);
    if view {
        s.style_view = Some((url, s.map().camera));
        follow_style_view(s, web_time::Duration::ZERO);
    }
    Ok(out)
}

/// The most passes made waiting for a style to load (each waits for the fetches under way).
const STYLE_WAITS: usize = 8;

/// Moves the map to the view in the root properties of the style added with `"view": true`
/// once it has loaded, unless the map has moved since, waiting up to `wait` for it (natively; the
/// browser loads between frames). True when the map moved.
pub(crate) fn follow_style_view(s: &mut Session, wait: web_time::Duration) -> bool {
    let Some((url, camera)) = s.style_view.clone() else { return false };
    if s.map().camera != camera {
        // Moved (panned, a bookmark) while the style loaded: the map stays where it was put.
        s.style_view = None;
        return false;
    }
    let Some(t) = s.map().tiles.iter().find(|t| t.kind == TileKind::Style && t.url == url).cloned() else {
        // Removed, or another map is active: nothing to follow.
        s.style_view = None;
        return false;
    };
    let wait = if cfg!(target_arch = "wasm32") { web_time::Duration::ZERO } else { wait };
    let start = web_time::Instant::now();
    let mut passes = 0;
    let view = loop {
        match rustgis_render::style_view(&t) {
            Ok(Some(v)) => break v,
            Ok(None) if passes < STYLE_WAITS && start.elapsed() < wait => {
                rustgis_render::rustgis_tiles::cache::wait(wait.saturating_sub(start.elapsed()));
                passes += 1;
            }
            Ok(None) => return false,
            Err(e) => {
                s.style_view = None;
                s.notify(format!("{url}: {e}"), true);
                return false;
            }
        }
    };
    s.style_view = None;
    if view.is_empty() {
        return false;
    }
    let center = view.center.map(|[lon, lat]| rustgis_geom::Coord::new(lon, lat));
    match super::navigate::go_to_view(s, center, view.zoom, view.bearing, view.pitch) {
        Ok(()) => true,
        Err(e) => {
            s.notify(format!("{url}: {e}"), true);
            false
        }
    }
}

/// The tile layer a command names by `index`, `name` or `id`.
fn find(s: &Session, p: &Value) -> Result<Option<usize>, String> {
    let tiles = &s.map().tiles;
    if let Some(i) = f(p, "index") {
        let i = (i >= 0.0 && i.fract() == 0.0).then_some(i as usize).filter(|i| *i < tiles.len());
        return i.map(Some).ok_or_else(|| "no tile layer at that index".to_string());
    }
    if let Some(n) = ps(p, "name") {
        return tiles.iter().position(|t| t.name.eq_ignore_ascii_case(n.trim())).map(Some).ok_or_else(|| format!("no tile layer '{n}'"));
    }
    if let Some(id) = ps(p, "id") {
        return tiles.iter().position(|t| t.id == id).map(Some).ok_or_else(|| format!("no tile layer '{id}'"));
    }
    Ok(None)
}

fn remove(s: &mut Session, p: &Value) -> CmdResult {
    match find(s, p)? {
        Some(i) => {
            let t = s.map_mut().tiles.remove(i);
            Ok(json!({"removed": [t.name]}))
        }
        None => {
            let gone: Vec<String> = s.map_mut().tiles.drain(..).map(|t| t.name).collect();
            Ok(json!({"removed": gone}))
        }
    }
}

/// The `blend` mode a command gives, if any.
fn blend(p: &Value) -> Result<Option<Blend>, String> {
    ps(p, "blend").map(|m| Blend::from_code(m).ok_or_else(|| format!("unknown blend mode '{m}'"))).transpose()
}

fn set(s: &mut Session, p: &Value) -> CmdResult {
    let i = find(s, p)?.ok_or("name or index is required")?;
    let mode = blend(p)?;
    let m = s.map_mut();
    let n = m.tiles.len();
    let Some(t) = m.tiles.get_mut(i) else { return Err("no tile layer at that index".into()) };
    if let Some(v) = b(p, "visible") {
        t.visible = v;
    }
    if let Some(o) = f(p, "opacity") {
        t.opacity = if o.is_finite() { o.clamp(0.0, 1.0) as f32 } else { 1.0 };
    }
    if let Some(m) = mode {
        t.blend = m;
    }
    if let Some(r) = ps(p, "rename").map(str::trim).filter(|r| !r.is_empty()) {
        t.name = r.chars().take(120).collect();
    }
    if let Some(mv) = ps(p, "move") {
        let to = match mv {
            "up" => i.saturating_sub(1),
            "down" => (i + 1).min(n - 1),
            "top" => 0,
            "bottom" => n - 1,
            other => return Err(format!("unknown move '{other}' (up, down, top or bottom)")),
        };
        let t = m.tiles.remove(i);
        m.tiles.insert(to, t);
    }
    Ok(list(s, false))
}

#[cfg(test)]
mod tests {
    use crate::{Project, Session};
    use serde_json::json;

    #[test]
    fn stacks_tile_layers() {
        let mut s = Session::new(Project::default());
        let osm = json!({"url": "https://tile.openstreetmap.org/{z}/{x}/{y}.png", "name": "OSM", "attribution": "© OpenStreetMap"});
        assert!(s.run("map.addTiles", &osm).is_ok());
        let labels = json!({"url": "https://x.example.com/labels/{z}/{x}/{y}.png", "name": "Labels", "overlay": true});
        assert!(s.run("map.addTiles", &labels).is_ok());
        let style = json!({"url": "https://tiles.openfreemap.org/styles/liberty", "name": "Liberty"});
        assert!(s.run("map.addTiles", &style).is_ok());
        let names: Vec<&str> = s.map().tiles.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["Labels", "Liberty", "OSM"]);
        assert_eq!(s.map().tiles[1].kind, rustgis_map::TileKind::Style);
        // Replacing keeps the overlays.
        assert!(s.run("map.addTiles", &json!({"url": "https://y.example.com/{z}/{x}/{y}.jpg", "replace": true})).is_ok());
        let names: Vec<&str> = s.map().tiles.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["Labels", "y.example.com"]);
        assert!(s.run("map.setTiles", &json!({"name": "labels", "opacity": 0.5, "move": "bottom"})).is_ok());
        assert_eq!(s.map().tiles[1].opacity, 0.5);
        assert!(s.run("edit.undo", &json!({})).is_ok());
        assert_eq!(s.map().tiles[0].name, "Labels");
        assert!(s.run("map.addTiles", &json!({"url": "file:///etc/passwd"})).is_err());
        assert!(s.run("map.addTiles", &json!({"url": "https://a/{z}", "kind": "nope"})).is_err());
        assert!(s.run("map.removeTiles", &json!({"index": 7})).is_err());
        assert!(s.run("map.removeTiles", &json!({})).is_ok());
        assert!(s.map().tiles.is_empty());
        // Saved and read back with the project.
        assert!(s.run("map.addTiles", &osm).is_ok());
        let text = serde_json::to_string(&s.project).unwrap();
        let p = Project::from_json(&text).unwrap();
        assert_eq!(p.map().tiles.len(), 1);
        assert_eq!(p.map().tiles[0].attribution, "© OpenStreetMap");
    }

    #[test]
    fn stacks_basemaps_and_blends_them() {
        use rustgis_map::effects::Blend;
        let mut s = Session::new(Project::default());
        let relief = json!({"url": "https://relief.example.com/{z}/{x}/{y}.png", "name": "Relief", "replace": true});
        assert!(s.run("map.addTiles", &relief).is_ok());
        // Without `replace`, a second basemap goes over the first, here multiplied with it.
        let imagery = json!({"url": "https://imagery.example.com/{z}/{x}/{y}.jpg", "name": "Imagery", "blend": "multiply"});
        assert!(s.run("map.addTiles", &imagery).is_ok());
        let names: Vec<&str> = s.map().tiles.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["Imagery", "Relief"]);
        assert_eq!(s.map().tiles[0].blend, Blend::Multiply);
        assert_eq!(s.map().tiles[1].blend, Blend::Normal);
        let listed = s.run("map.tiles", &json!({})).unwrap();
        assert_eq!(listed[0]["blend"], "multiply");
        assert!(s.run("map.setTiles", &json!({"name": "Relief", "blend": "Screen"})).is_ok());
        assert_eq!(s.map().tiles[1].blend, Blend::Screen);
        assert!(s.run("edit.undo", &json!({})).is_ok());
        assert_eq!(s.map().tiles[1].blend, Blend::Normal);
        assert!(s.run("map.setTiles", &json!({"index": 0, "blend": "nope"})).is_err());
        assert!(s.run("map.addTiles", &json!({"url": "https://z.example.com/{z}/{x}/{y}.png", "blend": "nope"})).is_err());
        assert_eq!(s.map().tiles.len(), 2);
        // Saved with the project; a normal layer saves no blend.
        let text = serde_json::to_string(&s.project).unwrap();
        let p = Project::from_json(&text).unwrap();
        assert_eq!(p.map().tiles[0].blend, Blend::Multiply);
        assert_eq!(text.matches("\"blend\"").count(), 1, "{text}");
    }
}
