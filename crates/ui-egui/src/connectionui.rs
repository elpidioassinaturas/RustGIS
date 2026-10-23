//! Named endpoint definitions and session-only logins, with web probes fetched off the frame.

use egui::vec2;
use rustgis_engine::connections::{Connection, DATABASE_KINDS};
use rustgis_engine::render::rustgis_tiles::{cache, s3};
use rustgis_engine::sources::Source;
use serde_json::{Value, json};

use crate::{RustGisApp, Slot, i18n};

#[derive(Default)]
pub struct State {
    pub id: Option<String>,
    pub kind: String,
    pub name: String,
    pub group: String,
    pub address: String,
    pub region: String,
    pub buckets: String,
    pub credentials: String,
    pub profile: String,
    pub password: String,
    pub access_key: String,
    pub secret_key: String,
    pub session_token: String,
    pub test: Option<(Value, Slot<Result<Vec<u8>, String>>)>,
}

pub(crate) const KINDS: [(&str, &str); 13] = [
    ("postgres", "PostgreSQL/PostGIS"),
    ("mysql", "MySQL/MariaDB"),
    ("sqlserver", "SQL Server"),
    ("service", "Web service (detected)"),
    ("wms", "WMS"),
    ("wmts", "WMTS"),
    ("wfs", "WFS"),
    ("wcs", "WCS"),
    ("ogcapi", "OGC API"),
    ("arcgis", "ArcGIS REST"),
    ("csw", "Catalogue (CSW)"),
    ("stac", "STAC API"),
    ("s3", "S3"),
];

/// The kinds Add From Service connects to.
pub(crate) const SERVICE_KINDS: &[&str] = &["service", "wms", "wmts", "wfs", "wcs", "ogcapi", "arcgis", "csw"];

/// The connection a dialog was opened on, while its trimmed, redacted address still matches.
/// Never infers identity from an address alone.
pub(crate) fn picked<'a>(list: &'a [Connection], id: Option<&str>, address: &str) -> Option<&'a Connection> {
    let id = id?;
    let c = list.iter().find(|c| c.id == id)?;
    (c.address == rustgis_engine::sources::redact(address.trim())).then_some(c)
}

/// Draws a saved-connection row when these kinds exist; returns only a newly picked connection.
pub(crate) fn picker(ui: &mut egui::Ui, id: &str, list: &[Connection], kinds: &[&str], current: Option<&Connection>) -> Option<Connection> {
    if !list.iter().any(|c| kinds.contains(&c.kind.as_str())) {
        return None;
    }
    ui.horizontal(|ui| {
        ui.label(tl!("Saved connection"));
        picker_combo(ui, id, list, kinds, current)
    })
    .inner
}

/// The picker control without its label, for a dialog's existing form grid.
pub(crate) fn picker_combo(
    ui: &mut egui::Ui,
    id: &str,
    list: &[Connection],
    kinds: &[&str],
    current: Option<&Connection>,
) -> Option<Connection> {
    let label = |c: &Connection| if c.group.is_empty() { c.name.clone() } else { format!("{} › {}", c.group, c.name) };
    let mut picked = None;
    egui::ComboBox::from_id_salt(id)
        .width(280.0)
        .selected_text(current.map(label).unwrap_or_else(|| tl!("Pick one…").to_string()))
        .show_ui(ui, |ui| {
            for c in list.iter().filter(|c| kinds.contains(&c.kind.as_str())) {
                let selected = current.is_some_and(|x| x.id == c.id);
                let hover = if c.kind == "s3" && c.address.is_empty() { tl!("Amazon S3") } else { &c.address };
                if ui.selectable_label(selected, label(c)).on_hover_text(hover).clicked() && !selected {
                    picked = Some(c.clone());
                }
            }
        });
    picked
}

/// Keeps an endpoint, never its secrets, as a new connection even at an existing address.
pub(crate) fn keep(app: &mut RustGisApp, p: Value) -> Result<String, String> {
    let v = app.run("connection.new", p)?;
    let id = v["id"].as_str().map(str::to_owned).ok_or_else(|| "connection.new returned no id".to_string())?;
    stripped_notice(app, &v);
    app.session.notify(i18n::fmt("Saved as the connection {name}", &[("name", v["name"].as_str().unwrap_or(""))]), false);
    Ok(id)
}

/// Offers to keep the endpoint connected by an add dialog.
pub(crate) fn keep_button(ui: &mut egui::Ui) -> bool {
    ui.button(tl!("Save as Connection"))
        .on_hover_text(tl!("Keep this endpoint under Data Sources › Connections, without its password or keys"))
        .clicked()
}

fn stripped_notice(app: &mut RustGisApp, v: &Value) {
    if v["stripped"] == true {
        app.session.notify(tl!("A password or token in the address was left out: connections never keep secrets.").to_string(), false);
    }
}

pub(crate) fn label(kind: &str) -> &'static str {
    KINDS.iter().find(|(k, _)| *k == kind).map(|(_, l)| *l).unwrap_or("Connection")
}

pub fn open_new(app: &mut RustGisApp, kind: &str) -> Result<(), String> {
    if !KINDS.iter().any(|(k, _)| *k == kind) {
        return Err(format!("unknown kind '{kind}'"));
    }
    if cfg!(target_arch = "wasm32") && DATABASE_KINDS.contains(&kind) {
        return Err(tl!("Database connections are used by the desktop app").into());
    }
    app.ui.connection =
        State { kind: kind.into(), credentials: if kind == "s3" { "keys".into() } else { String::new() }, ..Default::default() };
    app.dialogs = crate::dialogs::Dialogs { open: Some("newConnection"), ..Default::default() };
    Ok(())
}

pub fn open_edit(app: &mut RustGisApp, id: &str) -> Result<(), String> {
    let c = app.session.connections.find(id).cloned().ok_or_else(|| format!("no connection '{id}'"))?;
    app.ui.connection = State {
        region: c.options.get("region").cloned().unwrap_or_default(),
        buckets: c.options.get("buckets").cloned().unwrap_or_default(),
        credentials: c.options.get("credentials").cloned().unwrap_or_else(|| "none".into()),
        profile: c.options.get("profile").cloned().unwrap_or_default(),
        id: Some(c.id),
        kind: c.kind,
        name: c.name,
        group: c.group,
        address: c.address,
        ..Default::default()
    };
    app.dialogs = crate::dialogs::Dialogs { open: Some("editConnection"), ..Default::default() };
    Ok(())
}

pub fn log_in(app: &mut RustGisApp, id: &str) {
    match app.run("ui.connection", json!({"id": id})) {
        Ok(_) => app.dialogs.info = Some(tl!("Type the password or keys to log in for this session, then Save.").to_string()),
        Err(e) => app.session.notify(e, true),
    }
}

fn definition(st: &State) -> Value {
    let address = st.address.trim();
    let lower = address.to_ascii_lowercase();
    let address = if st.kind == "postgres" && !address.is_empty() && !lower.starts_with("postgres:") && !lower.starts_with("postgresql:") {
        format!("postgres:{address}")
    } else {
        address.to_owned()
    };
    let mut p = json!({"kind":st.kind, "name":st.name, "group":st.group, "address":address});
    if st.kind == "s3" {
        p["options"] = json!({"region":st.region, "buckets":st.buckets, "credentials":st.credentials});
        if st.credentials == "profile" {
            p["options"]["profile"] = json!(st.profile);
        }
    }
    p
}

fn secrets(st: &State, p: &mut Value) {
    if DATABASE_KINDS.contains(&st.kind.as_str()) && !st.password.is_empty() {
        p["password"] = json!(st.password);
    }
    if st.kind == "s3" && st.credentials == "keys" {
        for (key, value) in [("accessKeyId", &st.access_key), ("secretAccessKey", &st.secret_key), ("sessionToken", &st.session_token)] {
            if !value.is_empty() {
                p[key] = json!(value);
            }
        }
    }
}

pub fn start_test(app: &mut RustGisApp) {
    app.dialogs.error = None;
    app.dialogs.info = None;
    app.ui.connection.test = None;
    let mut p = definition(&app.ui.connection);
    secrets(&app.ui.connection, &mut p);
    if DATABASE_KINDS.contains(&app.ui.connection.kind.as_str()) {
        match app.run("connection.test", p) {
            Ok(v) => app.dialogs.info = Some(summary(&app.ui.connection.kind, &v["result"])),
            Err(e) => app.dialogs.error = Some(e),
        }
        return;
    }
    p["probe"] = json!(true);
    let v = match app.run("connection.test", p) {
        Ok(v) => v,
        Err(e) => {
            app.dialogs.error = Some(e);
            return;
        }
    };
    let Some(url) = v["fetch"].as_str() else {
        app.dialogs.error = Some(tl!("Nothing to test").into());
        return;
    };
    let q = definition(&app.ui.connection);
    let slot: Slot<Result<Vec<u8>, String>> = Slot::default();
    let land = slot.clone();
    cache::download(url, Box::new(move |r| *land.lock().unwrap_or_else(|e| e.into_inner()) = Some(r)));
    app.ui.connection.test = Some((q, slot));
    app.dialogs.info = Some(tl!("Testing…").into());
}

pub fn poll(app: &mut RustGisApp, ctx: &egui::Context) {
    if !matches!(app.dialogs.open, Some("newConnection" | "editConnection")) {
        app.ui.connection.test = None;
        return;
    }
    let Some((q, slot)) = app.ui.connection.test.as_mut() else { return };
    let result = slot.lock().unwrap_or_else(|e| e.into_inner()).take();
    let Some(result) = result else {
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
        return;
    };
    let mut q = q.take();
    app.ui.connection.test = None;
    app.dialogs.info = None;
    match result {
        Ok(bytes) => {
            q["answer"] = json!(String::from_utf8_lossy(&bytes));
            let kind = q["kind"].as_str().unwrap_or("").to_owned();
            match app.run("connection.test", q) {
                Ok(v) => app.dialogs.info = Some(summary(&kind, &v["result"])),
                Err(e) => app.dialogs.error = Some(e),
            }
        }
        Err(e) => app.dialogs.error = Some(e),
    }
}

fn summary(kind: &str, r: &Value) -> String {
    let count = r["count"].as_u64().unwrap_or(0).to_string();
    match kind {
        "postgres" | "mysql" | "sqlserver" => i18n::fmt("{n} tables with geometry", &[("n", &count)]),
        "csw" => {
            let n = r["matched"].as_u64().map(|n| n.to_string()).unwrap_or(count);
            i18n::fmt("{n} records in the catalogue", &[("n", &n)])
        }
        "stac" => i18n::fmt("{n} collections", &[("n", &count)]),
        "s3" => {
            if let Some(bucket) = r["bucket"].as_str() {
                let n = if r["truncated"] == true { format!("{count}+") } else { count };
                i18n::fmt("{n} items in {bucket}", &[("n", &n), ("bucket", bucket)])
            } else {
                i18n::fmt("{n} buckets", &[("n", &r["buckets"].as_array().map_or(0, Vec::len).to_string())])
            }
        }
        _ => {
            let kind = r["kind"].as_str().unwrap_or("");
            let title = r["title"].as_str().unwrap_or("");
            let count = r["layers"].as_array().map_or(0, Vec::len);
            if count == 0 && r["next"].is_string() {
                i18n::fmt("Connected to {kind} {title}", &[("kind", kind), ("title", title)])
            } else {
                i18n::fmt("{kind} {title}: {count} layers", &[("kind", kind), ("title", title), ("count", &count.to_string())])
            }
        }
    }
}

fn save(app: &mut RustGisApp) -> Result<(), String> {
    let st = &app.ui.connection;
    let mut p = definition(st);
    let command = if let Some(id) = &st.id {
        p["id"] = json!(id);
        if st.kind != "s3"
            && let Some(object) = p.as_object_mut()
        {
            object.remove("options");
        }
        "connection.edit"
    } else {
        if st.name.trim().is_empty()
            && let Some(object) = p.as_object_mut()
        {
            object.remove("name");
        }
        "connection.new"
    };
    let v = app.run(command, p)?;
    stripped_notice(app, &v);
    let mut login = json!({"id": v["id"]});
    secrets(&app.ui.connection, &mut login);
    if login.as_object().is_some_and(|p| p.len() > 1)
        && let Err(e) = app.run("connection.login", login)
    {
        app.ui.connection.id = v["id"].as_str().map(str::to_owned);
        app.dialogs.open = Some("editConnection");
        return Err(e);
    }
    let st = &mut app.ui.connection;
    st.password.clear();
    st.access_key.clear();
    st.secret_key.clear();
    st.session_token.clear();
    Ok(())
}

pub fn dialog(app: &mut RustGisApp, ui: &mut egui::Ui) -> bool {
    ui.label(
        egui::RichText::new(tl!("A named endpoint you can reuse. Passwords and keys are kept until RustGIS closes, never saved.")).weak(),
    );
    let st = &mut app.ui.connection;
    let database = DATABASE_KINDS.contains(&st.kind.as_str());
    let mut changed = false;
    egui::Grid::new("connection").num_columns(2).spacing(vec2(10.0, 8.0)).show(ui, |ui| {
        ui.label(tl!("Type"));
        ui.label(tl!(label(&st.kind)));
        ui.end_row();
        ui.label(tl!("Name"));
        changed |= ui.add(egui::TextEdit::singleline(&mut st.name).hint_text(tl!("From the address")).desired_width(360.0)).changed();
        ui.end_row();
        ui.label(tl!("Group"));
        changed |= ui.add(egui::TextEdit::singleline(&mut st.group).hint_text(tl!("None")).desired_width(360.0)).changed();
        ui.end_row();
        if database {
            ui.label(tl!("Connection"));
            let hint = match st.kind.as_str() {
                "mysql" => "mysql://user@host:3306/db",
                "sqlserver" => "mssql://user@host:1433/db",
                _ => "host=… dbname=… user=…",
            };
            changed |= ui.add(egui::TextEdit::singleline(&mut st.address).hint_text(hint).desired_width(360.0)).changed();
            ui.end_row();
            ui.label(tl!("Password"));
            changed |= ui
                .add(
                    egui::TextEdit::singleline(&mut st.password)
                        .password(true)
                        .hint_text(tl!("This session only (optional)"))
                        .desired_width(360.0),
                )
                .changed();
            ui.end_row();
        } else if st.kind == "s3" {
            for (label, field, hint) in [
                (tl!("Endpoint"), &mut st.address, tl!("Blank for Amazon S3")),
                (tl!("Region"), &mut st.region, s3::DEFAULT_REGION),
                (tl!("Buckets"), &mut st.buckets, tl!("Comma-separated (optional)")),
            ] {
                ui.label(label);
                changed |= ui.add(egui::TextEdit::singleline(field).hint_text(hint).desired_width(360.0)).changed();
                ui.end_row();
            }
            ui.label(tl!("Keys from"));
            let shown = crate::s3ui::SOURCES.iter().find(|(c, _)| *c == st.credentials).map(|(_, l)| tl!(*l)).unwrap_or_default();
            egui::ComboBox::from_id_salt("connection_credentials").selected_text(shown).width(360.0).show_ui(ui, |ui| {
                for (code, label) in crate::s3ui::SOURCES {
                    if cfg!(target_arch = "wasm32") && matches!(code, "profile" | "environment") {
                        continue;
                    }
                    changed |= ui.selectable_value(&mut st.credentials, code.to_owned(), tl!(label)).changed();
                }
            });
            ui.end_row();
            if st.credentials == "profile" {
                ui.label(tl!("Profile"));
                changed |= ui.add(egui::TextEdit::singleline(&mut st.profile).desired_width(360.0)).changed();
                ui.end_row();
            }
            if st.credentials == "keys" {
                for (label, field, secret) in [
                    (tl!("Access key ID"), &mut st.access_key, false),
                    (tl!("Secret access key"), &mut st.secret_key, true),
                    (tl!("Session token"), &mut st.session_token, true),
                ] {
                    ui.label(label);
                    changed |= ui.add(egui::TextEdit::singleline(field).password(secret).desired_width(360.0)).changed();
                    ui.end_row();
                }
            }
        } else {
            ui.label(tl!("Address"));
            let hint = match st.kind.as_str() {
                "stac" => "https://…/stac/v1",
                "arcgis" => "https://…/rest/services",
                "csw" => "https://…/csw",
                _ => "https://…",
            };
            changed |= ui.add(egui::TextEdit::singleline(&mut st.address).hint_text(hint).desired_width(360.0)).changed();
            ui.end_row();
        }
    });
    if changed {
        // A late download may finish, but its detached slot cannot report on the edited form.
        st.test = None;
        app.dialogs.info = None;
        app.dialogs.error = None;
    }
    let testing = st.test.is_some();
    let unavailable = database && cfg!(target_arch = "wasm32");
    let clicked = ui
        .horizontal(|ui| {
            let response = ui
                .add_enabled_ui(!testing && !unavailable, |ui| crate::widgets::dialog_button(ui, tl!("Test"), None, vec2(84.0, 28.0)))
                .inner;
            let response = if unavailable {
                response.on_disabled_hover_text(tl!("Database connections are used by the desktop app"))
            } else {
                response
            };
            if testing {
                ui.spinner();
            }
            response.clicked()
        })
        .inner;
    if clicked {
        start_test(app);
    }
    crate::dialogs::buttons(app, ui, "Save", save)
}

pub fn open_connection(app: &mut RustGisApp, c: &Connection) -> Result<(), String> {
    let (kind, extra) = match c.kind.as_str() {
        "postgres" | "mysql" | "sqlserver" => {
            if cfg!(target_arch = "wasm32") {
                return Err(tl!("Database connections are used by the desktop app").into());
            }
            ("database", Value::Null)
        }
        "service" => ("service", json!({"type":"auto"})),
        "wms" | "wmts" | "wfs" | "wcs" | "ogcapi" | "arcgis" | "csw" => ("service", json!({"type":c.kind})),
        "stac" => ("stac", json!({"api":"true"})),
        "s3" => {
            if s3::connection(&c.name).is_none() {
                if c.options.get("credentials").is_some_and(|v| v == "keys") {
                    open_edit(app, &c.id)?;
                    app.dialogs.info = Some(tl!("Type the keys to log in for this session, then Save.").into());
                    return Ok(());
                }
                app.run("connection.login", json!({"id":c.id}))?;
            }
            crate::s3ui::fill(app, c);
            let r = crate::sourcespane::open(
                app,
                &Source { kind: "s3".into(), name: c.name.clone(), address: c.name.clone(), extra: Value::Null },
            );
            if r.is_ok() {
                app.ui.s3.connection = Some(c.id.clone());
            }
            return r;
        }
        _ => return Err(format!("unknown kind '{}'", c.kind)),
    };
    let r = crate::sourcespane::open(app, &Source { kind: kind.into(), name: c.name.clone(), address: c.address.clone(), extra });
    if r.is_ok() {
        if kind == "stac" {
            app.ui.stac.connection = Some(c.id.clone());
        } else {
            app.dialogs.connection = Some(c.id.clone());
        }
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picked_requires_identity_and_the_unchanged_public_address() {
        let list = vec![
            Connection { id: "c1".into(), address: "https://example.com/wms".into(), ..Default::default() },
            Connection { id: "c2".into(), address: "https://example.com/wms".into(), ..Default::default() },
        ];
        assert!(picked(&list, None, &list[0].address).is_none());
        assert_eq!(picked(&list, Some("c2"), " https://example.com/wms ").map(|c| c.id.as_str()), Some("c2"));
        assert!(picked(&list, Some("removed"), &list[0].address).is_none());
        assert!(picked(&list, Some("c1"), "https://example.com/other").is_none());
        assert_eq!(picked(&list, Some("c1"), "https://example.com/wms?token=secret").map(|c| c.id.as_str()), Some("c1"));
    }
}
