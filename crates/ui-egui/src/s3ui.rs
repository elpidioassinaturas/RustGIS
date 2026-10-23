//! S3…: connect to Amazon S3 or an S3-compatible service, browse a bucket a level at a time, and
//! add an object by its `s3://` address. Listings are fetched off the frame through the tile
//! cache's downloads (a thread natively, the browser's fetch on the web), signed by
//! `rustgis_tiles::s3` when the connection has credentials.

use egui::vec2;
use rustgis_engine::connections::Connection;
use rustgis_engine::render::rustgis_tiles::{cache, s3};
use serde_json::{Value, json};

use crate::{RustGisApp, Slot, i18n};

type Answer = Slot<Result<Vec<u8>, String>>;

/// The legacy preference of remembered connections, read once by the connections import (never written).
pub(crate) const SAVED: &str = "s3_connections";

/// Where a connection's keys come from: typed, an AWS profile or the environment (desktop), or
/// none (public buckets). The profile and the environment are used only when chosen here.
pub(crate) const SOURCES: [(&str, &str); 4] = [
    ("keys", "Typed below"),
    ("profile", "An AWS profile"),
    ("environment", "The environment (AWS_ACCESS_KEY_ID…)"),
    ("none", "None (public buckets)"),
];

/// The dialog's form, browsing place and requests under way.
#[derive(Default)]
pub struct State {
    /// The saved connection this dialog was opened on, by id (see `connectionui::picked`).
    pub(crate) connection: Option<String>,
    /// The saved definition the form was last filled from or kept as: a later edit or removal elsewhere is noticed against it.
    synced: Option<Connection>,
    /// What Connect last connected, as Save as Connection keeps it.
    connected: Option<Value>,
    name: String,
    endpoint: String,
    region: String,
    key: String,
    secret: String,
    /// Where the keys come from ([`SOURCES`]; empty: typed).
    source: String,
    profile: String,
    /// The connection browsed (a session connection's name; none: anonymous AWS).
    conn: String,
    bucket: String,
    prefix: String,
    /// Prefixes and objects (key, size) of the level shown, and whether there are more.
    listing: Option<(Vec<String>, Vec<(String, u64)>, bool)>,
    buckets: Vec<String>,
    listing_job: Option<Answer>,
    buckets_job: Option<Answer>,
    error: Option<String>,
}

fn fetch(url: &str) -> Answer {
    let slot: Answer = Slot::default();
    let land = slot.clone();
    cache::download(url, Box::new(move |r| *land.lock().unwrap_or_else(|e| e.into_inner()) = Some(r)));
    slot
}

fn take(slot: &Answer) -> Option<Result<Vec<u8>, String>> {
    slot.lock().unwrap_or_else(|e| e.into_inner()).take()
}

/// Lands listings.
pub fn poll(app: &mut RustGisApp, ctx: &egui::Context) {
    let st = &mut app.ui.s3;
    if let Some(slot) = &st.listing_job {
        match take(slot) {
            Some(r) => {
                st.listing_job = None;
                match r.and_then(|b| s3::parse_list(&b)) {
                    Ok(l) => {
                        let objects = l.objects.into_iter().map(|o| (o.key, o.size)).collect();
                        st.listing = Some((l.prefixes, objects, l.next.is_some()));
                        st.error = None;
                    }
                    Err(e) => st.error = Some(why(&e)),
                }
            }
            None => ctx.request_repaint_after(std::time::Duration::from_millis(100)),
        }
    }
    if let Some(slot) = &st.buckets_job {
        match take(slot) {
            Some(r) => {
                st.buckets_job = None;
                match r.and_then(|b| s3::parse_buckets(&b)) {
                    Ok(b) => {
                        st.buckets = b;
                        st.error = None;
                    }
                    Err(e) => st.error = Some(why(&e)),
                }
            }
            None => ctx.request_repaint_after(std::time::Duration::from_millis(100)),
        }
    }
}

/// An error with what it usually means.
fn why(e: &str) -> String {
    if e.contains("403") {
        i18n::fmt("{e}: access denied (check the connection's keys, region and the bucket's name)", &[("e", e)])
    } else {
        e.to_string()
    }
}

/// The connection browsed, if one is chosen and still connected.
fn current(st: &State) -> Option<s3::Connection> {
    s3::connection(&st.conn)
}

fn list(st: &mut State) {
    let bucket = st.bucket.trim().to_string();
    if !s3::valid_bucket(&bucket) {
        st.error = Some(i18n::fmt("'{b}' isn't a bucket name", &[("b", &bucket)]));
        return;
    }
    st.bucket = bucket;
    st.listing = None;
    st.error = None;
    // Through the connection chosen, which serves the bucket from then on; "AWS, anonymous"
    // reads AWS with no keys whatever the connections.
    let url = match current(st) {
        Some(c) => {
            s3::grant(&c.name, &st.bucket);
            s3::list_url(&st.bucket, &st.prefix, None, Some(&c))
        }
        None => s3::anonymous(&s3::list_url(&st.bucket, &st.prefix, None, None)),
    };
    st.listing_job = Some(fetch(&url));
}

/// Puts a saved connection in the form, without carrying secrets from the previous one.
pub(crate) fn fill(app: &mut RustGisApp, c: &Connection) {
    let st = &mut app.ui.s3;
    st.name = c.name.clone();
    st.endpoint = c.address.clone();
    st.region = c.options.get("region").cloned().unwrap_or_default();
    st.source = c.options.get("credentials").cloned().unwrap_or_else(|| "none".into());
    st.profile = c.options.get("profile").cloned().unwrap_or_default();
    st.key.clear();
    st.secret.clear();
    st.connected = None;
    st.error = None;
}

fn pick(app: &mut RustGisApp, c: &Connection) {
    fill(app, c);
    app.ui.s3.connection = Some(c.id.clone());
    if s3::connection(&c.name).is_some() {
        open_connection(app, &c.name);
    } else if app.ui.s3.source != "keys" {
        match app.run("connection.login", json!({"id": c.id})) {
            Ok(_) => open_connection(app, &c.name),
            Err(e) => app.ui.s3.error = Some(e),
        }
    }
}

fn connect(app: &mut RustGisApp) {
    let st = &app.ui.s3;
    let name = if st.name.trim().is_empty() { "default".to_string() } else { st.name.trim().to_string() };
    let endpoint = st.endpoint.trim().to_string();
    let region = st.region.trim().to_string();
    let profile = if st.profile.trim().is_empty() { "default".to_string() } else { st.profile.trim().to_string() };
    let mut p = json!({"name": name, "region": region});
    if !endpoint.is_empty() {
        p["endpoint"] = json!(endpoint);
    }
    match st.source.as_str() {
        "profile" => p["profile"] = json!(profile),
        "environment" => p["environment"] = json!(true),
        "keys" if !st.key.trim().is_empty() || !st.secret.is_empty() => {
            p["accessKeyId"] = json!(st.key.trim());
            p["secretAccessKey"] = json!(st.secret.trim());
        }
        // Nothing typed, or none chosen: anonymous.
        _ => {}
    }
    match app.run("s3.connect", p) {
        Ok(v) => {
            let st = &mut app.ui.s3;
            // The secret stays with the session's connection, not in the form.
            st.secret.clear();
            st.conn = name.clone();
            st.error = None;
            st.buckets.clear();
            let credentials = v["credentials"].as_str().filter(|c| matches!(*c, "keys" | "profile" | "environment")).unwrap_or("none");
            let mut def = json!({"kind": "s3", "name": name, "address": endpoint,
                "options": {"region": region, "credentials": credentials}});
            if credentials == "profile" {
                def["options"]["profile"] = json!(profile);
            }
            st.connected = Some(def);
        }
        Err(e) => {
            app.ui.s3.connected = None;
            app.ui.s3.error = Some(e);
        }
    }
}

/// Opens the S3 browser on a session connection (the Data Sources pane's S3 list).
pub fn open_connection(app: &mut RustGisApp, name: &str) {
    let st = &mut app.ui.s3;
    if st.conn != name {
        st.conn = name.to_string();
        st.bucket.clear();
        st.prefix.clear();
        st.listing = None;
        st.buckets.clear();
        st.error = None;
    }
}

/// Forgets what was listed for the endpoint browsed.
fn reset_browse(st: &mut State) {
    st.listing = None;
    st.listing_job = None;
    st.buckets.clear();
    st.buckets_job = None;
    st.bucket.clear();
    st.prefix.clear();
}

/// Keeps the form true to the saved connection it stands for. A connection edited elsewhere
/// (its endpoint, region or login changed, which also ends its session) is filled in again
/// and what was listed for the old settings dropped; one removed is let go. The first look at
/// a connection only records it.
fn sync(app: &mut RustGisApp) {
    let list = crate::sourcespane::connections_snapshot(app);
    let st = &mut app.ui.s3;
    let Some(id) = st.connection.clone() else {
        st.synced = None;
        return;
    };
    match list.iter().find(|c| c.id == id && c.kind == "s3") {
        None => {
            st.connection = None;
            st.synced = None;
            reset_browse(st);
        }
        Some(c) => match st.synced.as_ref().filter(|s| s.id == c.id) {
            None => st.synced = Some(c.clone()),
            Some(s) if (s.name.as_str(), s.address.as_str(), &s.options) != (c.name.as_str(), c.address.as_str(), &c.options) => {
                let c = c.clone();
                fill(app, &c);
                reset_browse(&mut app.ui.s3);
                app.ui.s3.synced = Some(c);
            }
            Some(_) => {}
        },
    }
}

/// The dialog: a connection, then a bucket browsed a level at a time.
pub fn dialog(app: &mut RustGisApp, ui: &mut egui::Ui) {
    sync(app);
    ui.label(egui::RichText::new(tl!("Amazon S3, or an S3-compatible service (MinIO, Cloudflare R2, Wasabi…) by its endpoint. Keys are kept until RustGIS closes; a saved connection keeps its name, endpoint and region, never its secret.")).weak());
    egui::CollapsingHeader::new(tl!("Connection")).default_open(s3::connections().is_empty()).show(ui, |ui| connection_form(app, ui));
    ui.separator();
    browse(app, ui);
    if let Some(e) = &app.ui.s3.error {
        ui.colored_label(ui.visuals().error_fg_color, e);
    }
}

fn connection_form(app: &mut RustGisApp, ui: &mut egui::Ui) {
    let list = crate::sourcespane::connections_snapshot(app);
    let st = &app.ui.s3;
    // A blank name connects as "default" (see `connect`), so it is that connection's name here too.
    let shown_name = if st.name.trim().is_empty() { "default" } else { st.name.trim() };
    let current = list.iter().find(|c| c.kind == "s3" && Some(c.id.as_str()) == st.connection.as_deref() && c.name == shown_name);
    if let Some(c) = crate::connectionui::picker(ui, "s3_saved", &list, &["s3"], current) {
        pick(app, &c);
        return;
    }
    let st = &mut app.ui.s3;
    if !SOURCES.iter().any(|(c, _)| *c == st.source) {
        st.source = "keys".into();
    }
    let typed = st.source == "keys";
    egui::Grid::new("s3_conn").num_columns(2).spacing(vec2(10.0, 6.0)).show(ui, |ui| {
        ui.label(tl!("Name"));
        ui.add(egui::TextEdit::singleline(&mut st.name).hint_text("default").desired_width(300.0));
        ui.end_row();
        ui.label(tl!("Endpoint"));
        ui.add(egui::TextEdit::singleline(&mut st.endpoint).hint_text(tl!("empty for AWS; https://… for others")).desired_width(300.0));
        ui.end_row();
        ui.label(tl!("Region"));
        ui.add(egui::TextEdit::singleline(&mut st.region).hint_text(s3::DEFAULT_REGION).desired_width(300.0));
        ui.end_row();
        ui.label(tl!("Keys from"));
        let shown = SOURCES.iter().find(|(c, _)| *c == st.source).map(|(_, l)| tl!(*l).to_string()).unwrap_or_default();
        egui::ComboBox::from_id_salt("s3_source").selected_text(shown).width(300.0).show_ui(ui, |ui| {
            for (code, label) in SOURCES {
                // AWS profiles and the environment are the desktop's.
                if cfg!(target_arch = "wasm32") && matches!(code, "profile" | "environment") {
                    continue;
                }
                ui.selectable_value(&mut st.source, code.to_string(), tl!(label));
            }
        });
        ui.end_row();
        if st.source == "profile" {
            ui.label(tl!("Profile"));
            ui.add(egui::TextEdit::singleline(&mut st.profile).hint_text("default").desired_width(300.0));
            ui.end_row();
        }
        ui.label(tl!("Access key ID"));
        ui.add_enabled(typed, egui::TextEdit::singleline(&mut st.key).desired_width(300.0));
        ui.end_row();
        ui.label(tl!("Secret access key"));
        ui.add_enabled(typed, egui::TextEdit::singleline(&mut st.secret).password(true).desired_width(300.0));
        ui.end_row();
    });
    if ui.button(tl!("Connect")).clicked() {
        connect(app);
    }
    let st = &app.ui.s3;
    if let Some(def) = &st.connected
        && def["name"].as_str() == Some(st.conn.as_str())
        && s3::connection(&st.conn).is_some()
        && current.is_none()
    {
        if list.iter().any(|c| c.name.eq_ignore_ascii_case(&st.conn)) {
            ui.label(egui::RichText::new(tl!("Rename it to save it: a connection has this name")).small().weak());
        } else if crate::connectionui::keep_button(ui) {
            match crate::connectionui::keep(app, def.clone()) {
                Ok(id) => app.ui.s3.connection = Some(id),
                Err(e) => app.ui.s3.error = Some(e),
            }
        }
    }
}

fn browse(app: &mut RustGisApp, ui: &mut egui::Ui) {
    let conns = s3::connections();
    let st = &mut app.ui.s3;
    // A connection since forgotten: back to anonymous (never another connection's keys).
    if !st.conn.is_empty() && !conns.iter().any(|c| c.name == st.conn) {
        st.conn.clear();
    }
    let shown = if st.conn.is_empty() { tl!("AWS, anonymous").to_string() } else { st.conn.clone() };
    let mut go = false;
    ui.horizontal(|ui| {
        ui.label(tl!("Browse with"));
        egui::ComboBox::from_id_salt("s3_browse_conn").selected_text(shown).width(180.0).show_ui(ui, |ui| {
            ui.selectable_value(&mut st.conn, String::new(), tl!("AWS, anonymous"));
            for c in &conns {
                ui.selectable_value(&mut st.conn, c.name.clone(), &c.name);
            }
        });
        let signed = conns.iter().any(|c| c.name == st.conn && c.credentials.is_some());
        if ui.add_enabled(signed && st.buckets_job.is_none(), egui::Button::new(tl!("My Buckets"))).clicked()
            && let Some(c) = s3::connection(&st.conn)
        {
            st.error = None;
            st.buckets_job = Some(fetch(&s3::buckets_url(&c)));
        }
    });
    if !st.buckets.is_empty() {
        ui.horizontal_wrapped(|ui| {
            for b in st.buckets.clone() {
                if ui.small_button(&b).clicked() {
                    st.bucket = b;
                    st.prefix.clear();
                    go = true;
                }
            }
        });
    }
    ui.horizontal(|ui| {
        ui.label(tl!("Bucket"));
        let r = ui.add(egui::TextEdit::singleline(&mut st.bucket).hint_text("sentinel-cogs").desired_width(260.0));
        go |= r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        go |= ui.add_enabled(st.listing_job.is_none(), egui::Button::new(tl!("List"))).clicked();
    });
    let mut add = None;
    if let Some((prefixes, objects, more)) = st.listing.clone() {
        ui.horizontal(|ui| {
            if ui.add_enabled(!st.prefix.is_empty(), egui::Button::new("↑")).on_hover_text(tl!("Up a level")).clicked() {
                let trimmed = st.prefix.trim_end_matches('/');
                st.prefix = trimmed.rfind('/').map(|i| trimmed.get(..=i).unwrap_or("").to_string()).unwrap_or_default();
                go = true;
            }
            ui.monospace(format!("s3://{}/{}", st.bucket, st.prefix));
        });
        egui::ScrollArea::vertical().max_height(280.0).show(ui, |ui| {
            for p in &prefixes {
                let label = p.strip_prefix(st.prefix.as_str()).unwrap_or(p);
                if ui.selectable_label(false, format!("📁 {label}")).clicked() {
                    st.prefix = p.clone();
                    go = true;
                }
            }
            for (key, size) in &objects {
                ui.horizontal(|ui| {
                    if ui.small_button(tl!("Add")).clicked() {
                        // Through the connection browsed with (it serves the bucket now), or by its
                        // public web address at AWS when browsing anonymously.
                        // Anonymous stays anonymous even for a bucket a connection serves.
                        add = Some(if st.conn.is_empty() {
                            s3::anonymous(&s3::object_url(&st.bucket, key, None))
                        } else {
                            format!("s3://{}/{key}", st.bucket)
                        });
                    }
                    ui.label(key.strip_prefix(st.prefix.as_str()).unwrap_or(key));
                    ui.label(egui::RichText::new(size_text(*size)).small().weak());
                });
            }
            if prefixes.is_empty() && objects.is_empty() {
                ui.label(egui::RichText::new(tl!("Nothing here")).weak());
            }
        });
        if more {
            ui.label(egui::RichText::new(tl!("The first 1,000 entries; narrow it down a folder at a time.")).small().weak());
        }
    } else if st.listing_job.is_some() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(tl!("Listing…"));
        });
    }
    if go {
        list(st);
    }
    if let Some(url) = add {
        // Added as any address is: read in place when a cloud-optimised GeoTIFF, else downloaded.
        if let Err(e) = app.run("ui.addUrl", json!({"url": url})) {
            app.ui.s3.error = Some(e);
        }
    }
}

fn size_text(n: u64) -> String {
    match n {
        n if n >= 1 << 30 => format!("{:.1} GB", n as f64 / f64::from(1u32 << 30)),
        n if n >= 1 << 20 => format!("{:.1} MB", n as f64 / f64::from(1u32 << 20)),
        n if n >= 1 << 10 => format!("{:.0} KB", n as f64 / 1024.0),
        n => format!("{n} B"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anonymous_connection_can_be_kept_and_picked_after_disconnect() {
        let mut app = RustGisApp::with_sample();
        app.ui.s3.name = "keep-s3-test".into();
        app.ui.s3.endpoint = "https://s3.example.com".into();
        app.ui.s3.region = "eu-west-1".into();
        app.ui.s3.source = "none".into();
        connect(&mut app);
        assert_eq!(app.ui.s3.error, None);
        let def = app.ui.s3.connected.clone().unwrap();
        assert_eq!(
            def,
            json!({"kind":"s3","name":"keep-s3-test","address":"https://s3.example.com",
            "options":{"region":"eu-west-1","credentials":"none"}})
        );
        assert!(app.session.connections.connections.is_empty(), "Connect alone must not save");
        let id = crate::connectionui::keep(&mut app, def).unwrap();
        let c = app.session.connections.find(&id).unwrap().clone();
        assert_eq!(c.options.get("credentials").map(String::as_str), Some("none"));
        s3::disconnect("keep-s3-test");
        pick(&mut app, &c);
        assert!(s3::connection("keep-s3-test").is_some());
        assert_eq!(app.ui.s3.conn, "keep-s3-test");
        assert_eq!(app.ui.s3.connection.as_deref(), Some(c.id.as_str()));
        s3::disconnect("keep-s3-test");
    }

    #[test]
    fn picking_typed_keys_clears_previous_secrets_and_waits_for_connect() {
        let mut app = RustGisApp::with_sample();
        let v = app
            .run(
                "connection.new",
                json!({"kind":"s3","name":"pick-typed-s3-test","address":"",
            "options":{"credentials":"keys"}}),
            )
            .unwrap();
        let c = app.session.connections.find(v["id"].as_str().unwrap()).unwrap().clone();
        app.ui.s3.key = "previous-key".into();
        app.ui.s3.secret = "previous-secret".into();
        app.ui.s3.connected = Some(json!({"name":"previous"}));
        pick(&mut app, &c);
        assert!(s3::connection(&c.name).is_none());
        assert!(app.ui.s3.key.is_empty());
        assert!(app.ui.s3.secret.is_empty());
        assert!(app.ui.s3.connected.is_none());
        assert_eq!(app.ui.s3.name, c.name);
        assert_eq!(app.ui.s3.connection.as_deref(), Some(c.id.as_str()));
        app.ui.s3.key = "test-access".into();
        app.ui.s3.secret = "test-secret".into();
        connect(&mut app);
        assert_eq!(app.ui.s3.error, None);
        assert!(app.ui.s3.secret.is_empty());
        assert_eq!(app.ui.s3.connected.as_ref().unwrap()["options"]["credentials"], "keys");
        assert_eq!(s3::connection(&c.name).unwrap().name, c.name);
        s3::disconnect(&c.name);
    }

    #[test]
    fn editing_or_removing_the_picked_connection_refills_the_form_and_drops_its_listing() {
        let mut app = RustGisApp::with_sample();
        let v = app
            .run(
                "connection.new",
                json!({"kind":"s3","name":"edited-s3-test","address":"https://old.example.com",
            "options":{"region":"eu-west-1","credentials":"none"}}),
            )
            .unwrap();
        let id = v["id"].as_str().unwrap().to_string();
        let c = app.session.connections.find(&id).unwrap().clone();
        pick(&mut app, &c);
        sync(&mut app);
        app.ui.s3.bucket = "old-bucket".into();
        app.ui.s3.listing = Some((vec!["old/".into()], Vec::new(), false));
        // Unchanged: left as it is.
        sync(&mut app);
        assert_eq!(app.ui.s3.bucket, "old-bucket");

        let edit = json!({"id": id, "address": "https://new.example.com", "options": {"region":"us-west-2","credentials":"none"}});
        app.run("connection.edit", edit).unwrap();
        sync(&mut app);
        assert_eq!(app.ui.s3.endpoint, "https://new.example.com");
        assert_eq!(app.ui.s3.region, "us-west-2");
        assert!(app.ui.s3.listing.is_none() && app.ui.s3.bucket.is_empty(), "nothing listed for the old endpoint is kept");
        assert_eq!(app.ui.s3.connection.as_deref(), Some(id.as_str()));

        app.ui.s3.bucket = "other".into();
        app.run("connection.remove", json!({"id": id})).unwrap();
        sync(&mut app);
        assert_eq!(app.ui.s3.connection, None);
        assert!(app.ui.s3.bucket.is_empty());
        s3::disconnect("edited-s3-test");
    }
}
