//! Keyboard shortcuts: one map from key combinations to commands. It drives the key handling
//! ([`crate::chrome::shortcuts`]), the shortcut shown in tooltips, menu rows and Command Search,
//! and the Help › Shortcuts window.
//!
//! A key combination is written `Mod+Shift+Z`: `Mod` is Ctrl, or ⌘ on macOS, so one spelling
//! works on every platform and in the browser.

use egui::Key;
use rustgis_engine::Registry;
use serde_json::Value;

use crate::RustGisApp;

/// The default shortcuts: (keys, command, parameters as JSON, or empty for none). Clipboard
/// keys are not here: they arrive as copy, cut and paste events, not key presses.
const DEFAULTS: &[(&str, &str, &str)] = &[
    ("Mod+K", "ui.commandSearch", ""),
    ("Mod+N", "project.new", ""),
    ("Mod+O", "ui.open", ""),
    ("Mod+S", "ui.save", ""),
    ("Mod+Shift+S", "ui.saveAs", ""),
    ("Mod+Z", "edit.undo", ""),
    ("Mod+Y", "edit.redo", ""),
    ("Mod+Shift+Z", "edit.redo", ""),
    ("Mod+T", "table.open", ""),
    ("Mod+F1", "ui.ribbon", ""),
    ("Mod+Equals", "map.zoomIn", ""),
    ("Mod+Plus", "map.zoomIn", ""),
    ("Mod+Minus", "map.zoomOut", ""),
    ("Home", "map.zoomFull", ""),
    ("Escape", "tool.set", r#"{"tool":"explore"}"#),
    ("Delete", "edit.delete", ""),
    ("Mod+D", "layout.duplicateElement", ""),
];

/// The clipboard keys: (keys, command). They arrive as copy, cut and paste events rather than key
/// presses, so they are shown but never bound.
pub const CLIPBOARD: &[(&str, &str)] = &[("Mod+C", "edit.copy"), ("Mod+X", "edit.cut"), ("Mod+V", "edit.paste")];

/// The map tools a shortcut can pick (`tool.set` with this tool): (tool, label).
pub const MAP_TOOLS: &[(&str, &str)] = &[
    ("explore", "Explore"),
    ("select", "Select"),
    ("zoomIn", "Zoom In"),
    ("measure", "Measure Distance"),
    ("measureArea", "Measure Area"),
    ("createPoint", "Create Point"),
    ("createLine", "Create Line"),
    ("createPolygon", "Create Polygon"),
    ("editVertices", "Edit Vertices"),
];

/// Labels of the `ui.*` commands that have a shortcut but no entry in Command Search.
const UI_LABELS: &[(&str, &str)] = &[
    ("ui.commandSearch", "Command Search"),
    ("ui.open", "Open Project…"),
    ("ui.save", "Save Project"),
    ("ui.saveAs", "Save Project As…"),
    ("ui.ribbon", "Collapse or Expand the Ribbon"),
];

/// A key with its modifiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Chord {
    /// Ctrl, or ⌘ on macOS.
    pub command: bool,
    pub alt: bool,
    pub shift: bool,
    pub key: Key,
}

impl Chord {
    /// Whether a focused text field takes this chord for itself: egui's undo (Mod+Z) and redo
    /// (Mod+Y, Mod+Shift+Z), matched as egui matches them, an extra Alt or Shift aside.
    pub fn is_text_undo(self) -> bool {
        self.command && matches!(self.key, Key::Z | Key::Y)
    }

    /// Reads `Mod+Shift+Z`, `Ctrl+T`, `Home`, `F5`, `Mod++`. `Ctrl`, `Cmd` and `⌘` all mean
    /// `Mod`; letters may be lower case. `None` for anything else.
    pub fn parse(s: &str) -> Option<Chord> {
        let s = s.trim();
        let (mods, key) = match s.strip_suffix('+') {
            // `Mod++` and `+`: the key is the plus sign.
            Some(rest) if rest.is_empty() || rest.ends_with('+') => (rest.trim_end_matches('+'), "+"),
            _ => s.rsplit_once('+').unwrap_or(("", s)),
        };
        let key = key.trim();
        let key = Key::from_name(key).or_else(|| Key::from_name(&key.to_ascii_uppercase()))?;
        let mut chord = Chord { command: false, alt: false, shift: false, key };
        for m in mods.split('+').map(str::trim).filter(|m| !m.is_empty()) {
            match m.to_ascii_lowercase().as_str() {
                "mod" | "ctrl" | "control" | "cmd" | "command" | "⌘" => chord.command = true,
                "alt" | "option" | "opt" | "⌥" => chord.alt = true,
                "shift" | "⇧" => chord.shift = true,
                _ => return None,
            }
        }
        Some(chord)
    }

    /// The modifiers to match, for egui.
    pub fn modifiers(self) -> egui::Modifiers {
        let mut m = egui::Modifiers::NONE;
        if self.command {
            m |= egui::Modifiers::COMMAND;
        }
        if self.alt {
            m |= egui::Modifiers::ALT;
        }
        if self.shift {
            m |= egui::Modifiers::SHIFT;
        }
        m
    }

    /// As shown to the user: `⇧⌘S` on macOS, `Ctrl+Shift+S` elsewhere.
    pub fn text(self) -> String {
        let key = match self.key {
            Key::Escape => "Esc",
            Key::Plus | Key::Minus | Key::Equals => self.key.symbol_or_name(),
            k => k.name(),
        };
        if cfg!(target_os = "macos") {
            let mut s = String::new();
            for (on, sym) in [(self.alt, "⌥"), (self.shift, "⇧"), (self.command, "⌘")] {
                if on {
                    s.push_str(sym);
                }
            }
            s + key
        } else {
            let mut s = String::new();
            for (on, name) in [(self.command, "Ctrl+"), (self.alt, "Alt+"), (self.shift, "Shift+")] {
                if on {
                    s.push_str(name);
                }
            }
            s + key
        }
    }
}

impl std::fmt::Display for Chord {
    /// The portable spelling [`Chord::parse`] reads: `Mod+Shift+Z`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (on, name) in [(self.command, "Mod+"), (self.alt, "Alt+"), (self.shift, "Shift+")] {
            if on {
                f.write_str(name)?;
            }
        }
        f.write_str(self.key.name())
    }
}

/// A key combination that runs a command.
#[derive(Clone, Debug, PartialEq)]
pub struct Binding {
    pub chord: Chord,
    pub command: String,
    /// `Value::Null` for none.
    pub params: Value,
}

impl Binding {
    /// Whether this runs `command` with `params` (`null` and `{}` are both no parameters).
    pub fn runs(&self, command: &str, params: &Value) -> bool {
        let none = |v: &Value| v.is_null() || v.as_object().is_some_and(|o| o.is_empty());
        self.command == command && (self.params == *params || (none(&self.params) && none(params)))
    }
}

/// Every shortcut.
#[derive(Clone, Debug, PartialEq)]
pub struct Keymap {
    pub bindings: Vec<Binding>,
}

impl Default for Keymap {
    fn default() -> Self {
        let bindings = DEFAULTS
            .iter()
            .filter_map(|(keys, command, params)| {
                Some(Binding {
                    chord: Chord::parse(keys)?,
                    command: (*command).to_string(),
                    params: serde_json::from_str(params).unwrap_or(Value::Null),
                })
            })
            .collect();
        Keymap { bindings }
    }
}

impl Keymap {
    /// The defaults, with the shortcut a command declares itself ([`CommandSpec::key`]) where
    /// its key is free.
    ///
    /// [`CommandSpec::key`]: rustgis_engine::CommandSpec::key
    pub fn new(registry: &Registry) -> Self {
        let mut map = Keymap::default();
        for spec in &registry.commands {
            if let Some(chord) = Chord::parse(spec.shortcut)
                && map.command_for(chord).is_none()
                && !is_clipboard(chord)
            {
                map.bindings.push(Binding { chord, command: spec.id.to_string(), params: Value::Null });
            }
        }
        map
    }

    /// The binding a key combination runs.
    pub fn command_for(&self, chord: Chord) -> Option<&Binding> {
        self.bindings.iter().find(|b| b.chord == chord)
    }

    /// The keys that run `command` with `params`, in order.
    pub fn keys_for<'a>(&'a self, command: &'a str, params: &'a Value) -> impl Iterator<Item = Chord> + 'a {
        self.bindings.iter().filter(move |b| b.runs(command, params)).map(|b| b.chord)
    }

    /// The bindings in the order to match key presses: those with more modifiers first, since
    /// egui ignores an extra Shift or Alt (Ctrl+Shift+Z would otherwise run Ctrl+Z's command).
    pub fn match_order(&self) -> Vec<&Binding> {
        let mut order: Vec<&Binding> = self.bindings.iter().collect();
        order.sort_by_key(|b| std::cmp::Reverse(u8::from(b.chord.shift) + u8::from(b.chord.alt)));
        order
    }

    /// The bindings grouped by what they run, in order of first appearance: (command, params,
    /// keys).
    pub fn grouped(&self) -> Vec<(&str, &Value, Vec<Chord>)> {
        let mut groups: Vec<(&str, &Value, Vec<Chord>)> = Vec::new();
        for b in &self.bindings {
            match groups.iter_mut().find(|(c, p, _)| b.runs(c, p)) {
                Some((_, _, keys)) => keys.push(b.chord),
                None => groups.push((b.command.as_str(), &b.params, vec![b.chord])),
            }
        }
        groups
    }
}

/// Whether a key combination is one of the [`CLIPBOARD`] keys.
pub fn is_clipboard(chord: Chord) -> bool {
    CLIPBOARD.iter().any(|(keys, _)| Chord::parse(keys) == Some(chord))
}

/// The first shortcut of `command` with `params` as shown to the user (a clipboard command's
/// fixed key included), or empty for none.
pub fn shortcut_text(app: &RustGisApp, command: &str, params: &Value) -> String {
    let clipboard = || CLIPBOARD.iter().find(|(_, c)| *c == command).and_then(|(keys, _)| Chord::parse(keys));
    app.ui.keymap.keys_for(command, params).next().or_else(clipboard).map(Chord::text).unwrap_or_default()
}

/// What a binding runs, translated: the command's label, or the map tool it picks.
pub fn label(app: &RustGisApp, command: &str, params: &Value) -> String {
    if command == "tool.set"
        && let Some(tool) = params.get("tool").and_then(Value::as_str)
    {
        let name = MAP_TOOLS.iter().find(|(code, _)| *code == tool).map_or(tool, |(_, label)| tl!(label));
        return crate::i18n::fmt("Map Tool: {tool}", &[("tool", name)]);
    }
    if let Some(spec) = app.session.registry.get(command) {
        return tl!(spec.label).to_string();
    }
    if let Some(c) = rustgis_engine::plugins::command(command) {
        return c.plugin.manifest.translate(app.lang().code(), &c.decl.label).to_string();
    }
    let ui = UI_LABELS.iter().copied().chain(crate::chrome::UI_COMMANDS.iter().map(|&(id, label, _)| (id, label)));
    for (id, label) in ui {
        if id == command {
            return tl!(label).to_string();
        }
    }
    command.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Undo and redo are a text field's own, an extra Alt or Shift aside; nothing else is.
    #[test]
    fn text_undo_chords_are_undo_and_redo() {
        let is = |s: &str| Chord::parse(s).unwrap().is_text_undo();
        assert!(is("Mod+Z") && is("Mod+Shift+Z") && is("Mod+Y") && is("Mod+Alt+Z"));
        assert!(!is("Mod+S") && !is("Z") && !is("Mod+K"));
    }

    #[test]
    fn chords_read_and_write() {
        let c = Chord::parse("Mod+Shift+Z");
        assert_eq!(c, Some(Chord { command: true, alt: false, shift: true, key: Key::Z }));
        assert_eq!(c.map(|c| c.to_string()).as_deref(), Some("Mod+Shift+Z"));
        assert_eq!(Chord::parse("ctrl+t").map(|c| c.to_string()).as_deref(), Some("Mod+T"));
        assert_eq!(Chord::parse("Mod++").map(|c| (c.command, c.key)), Some((true, Key::Plus)));
        assert_eq!(Chord::parse("+").map(|c| (c.command, c.key)), Some((false, Key::Plus)));
        assert_eq!(Chord::parse("Mod+-").map(|c| c.key), Some(Key::Minus));
        assert_eq!(Chord::parse("d").map(|c| c.key), Some(Key::D));
        assert_eq!(Chord::parse("Esc").map(|c| c.key), Some(Key::Escape));
        for bad in ["", "Mod+", "Hyper+X", "Mod+Shift", "Mod+NoSuchKey"] {
            assert_eq!(Chord::parse(bad), None, "{bad}");
        }
        // Every spelling written out reads back the same.
        for b in Keymap::default().bindings {
            assert_eq!(Chord::parse(&b.chord.to_string()), Some(b.chord));
        }
    }

    #[test]
    fn every_default_parses_once() {
        let map = Keymap::default();
        assert_eq!(map.bindings.len(), DEFAULTS.len(), "a default failed to parse");
        let mut seen = std::collections::HashSet::new();
        for b in &map.bindings {
            assert!(seen.insert(b.chord), "{} is bound twice", b.chord);
        }
    }

    #[test]
    fn more_modifiers_match_first() {
        let map = Keymap::default();
        let order = map.match_order();
        let pos = |keys: &str| Chord::parse(keys).and_then(|c| order.iter().position(|b| b.chord == c));
        assert!(pos("Mod+Shift+Z") < pos("Mod+Z"));
        assert!(pos("Mod+Shift+S") < pos("Mod+S"));
    }

    #[test]
    fn clipboard_keys_stay_events() {
        // Copy, Cut and Paste declare Mod+C/X/V: binding them too would run each twice.
        let app = RustGisApp::with_sample();
        assert!(app.ui.keymap.bindings.iter().all(|b| !is_clipboard(b.chord)));
        assert!(!shortcut_text(&app, "edit.copy", &Value::Null).is_empty(), "but Copy still shows its key");
        // A command's own key joins the defaults only where it is free.
        assert_eq!(app.ui.keymap.keys_for("layer.remove", &Value::Null).count(), 0, "Delete deletes features");
    }

    #[test]
    fn redo_has_two_keys() {
        let map = Keymap::default();
        let keys: Vec<String> = map.keys_for("edit.redo", &Value::Null).map(|c| c.to_string()).collect();
        assert_eq!(keys, ["Mod+Y", "Mod+Shift+Z"]);
        let explore = serde_json::json!({"tool": "explore"});
        assert_eq!(map.keys_for("tool.set", &explore).count(), 1);
        assert_eq!(map.keys_for("tool.set", &serde_json::json!({"tool": "select"})).count(), 0);
        // `{}` and `null` are both no parameters.
        assert_eq!(map.keys_for("table.open", &serde_json::json!({})).count(), 1);
    }
}
