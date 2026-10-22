//! Autocomplete for the Script pane (#662): a floating list of suggestions just under the cursor,
//! as in a code editor. ↑/↓ move the highlight, Enter or Tab accept it, Escape closes the list, or
//! a click accepts an item. What to offer at a cursor (`offer`) is worked out apart from the box
//! and tested on its own; this first step completes command ids (a line's first word).

use egui::text::{CCursor, CCursorRange};
use egui::widgets::text_edit::{TextEditOutput, TextEditState};
use egui::{Id, Key, Modifiers, Ui};
use rustgis_engine::cmd::workflows::refused_in_script;

/// The most suggestions the list shows.
const MAX_ITEMS: usize = 10;
/// The fewest characters typed before ids are offered (one would offer most of them).
const MIN_TYPED: usize = 2;
/// The list's narrowest width.
const MIN_WIDTH: f32 = 220.0;

/// What to offer at a cursor: the word being typed (char indexes, `end` at the cursor) and what
/// can replace it.
#[derive(Clone, Debug, PartialEq)]
pub struct Offer {
    pub start: usize,
    pub end: usize,
    pub items: Vec<String>,
}

/// The ids that complete the command id being typed at char `at` of `script`: the cursor at the
/// end of a line's first word (comments aside), at least `MIN_TYPED` characters typed. `ids` are
/// the commands a script can run. Case aside, an id equal to the word isn't offered again.
#[cfg(test)]
pub fn offer(script: &str, at: usize, ids: &[String]) -> Option<Offer> {
    let (start, word) = word_at(script, at)?;
    complete(start, at, &word, ids)
}

/// The command id being typed at char `at` of `script`, and where it starts (a char index): the
/// cursor at the end of a line's first word (comments aside), at least `MIN_TYPED` characters.
/// It reads the cursor's line only, so working an offer out costs no more for a long script.
pub fn word_at(script: &str, at: usize) -> Option<(usize, String)> {
    let (before, after) = script.split_at(byte_at(script, at)?);
    // The cursor ends the word: no word character right after it.
    if after.chars().next().is_some_and(|c| !c.is_whitespace()) {
        return None;
    }
    let line = before.rsplit('\n').next().unwrap_or_default();
    let word = line.trim_start();
    if word.chars().count() < MIN_TYPED || word.chars().any(char::is_whitespace) || word.starts_with('#') {
        return None;
    }
    Some((at - word.chars().count(), word.to_string()))
}

/// The byte char `at` of `script` starts at (its end for `at` at the end); None past it.
fn byte_at(script: &str, at: usize) -> Option<usize> {
    if at == 0 { Some(0) } else { script.char_indices().nth(at - 1).map(|(b, c)| b + c.len_utf8()) }
}

/// The command whose parameters a cursor is in (`hint_at`): its line's start and its `{` (char
/// indexes), and its id.
#[derive(Clone, Debug, PartialEq)]
pub struct Hint {
    pub line: usize,
    pub brace: usize,
    pub id: String,
}

/// The command whose parameters the cursor at char `at` is in: the cursor inside its line's
/// `{…}` (quoted text aside: a `}` in a string doesn't close it). None outside the braces, or on
/// a line with no id.
pub fn hint_at(script: &str, at: usize) -> Option<Hint> {
    let before = script.get(..byte_at(script, at)?)?;
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    let line = before.get(line_start..)?;
    let id = crate::scriptcolor::line_id(line)?;
    let brace = line.find('{')?;
    let (mut depth, mut quoted, mut escaped) = (0usize, false, false);
    for c in line.get(brace..)?.chars() {
        let was_escaped = escaped;
        escaped = c == '\\' && !was_escaped;
        match c {
            '"' if !was_escaped => quoted = !quoted,
            '{' if !quoted => depth += 1,
            '}' if !quoted => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    let chars = |s: Option<&str>| s.map_or(0, |s| s.chars().count());
    let line_chars = chars(before.get(..line_start));
    (depth > 0).then(|| Hint { line: line_chars, brace: line_chars + chars(line.get(..brace)), id: id.to_string() })
}

/// Where a popup of `size` (last frame's) goes by `cursor`: under it if it fits, else above if it
/// fits, else on the side with more room; kept on screen sideways. Its corner, and the height its
/// content may take (it scrolls beyond: a short window, a long help).
fn place(cursor: egui::Rect, size: egui::Vec2, screen: egui::Rect, min: f32) -> (egui::Pos2, f32) {
    let (below, above) = (screen.bottom() - cursor.bottom(), cursor.top() - screen.top());
    let under = size.y <= below || (size.y > above && below >= above);
    let room = (if under { below } else { above } - FRAME).max(min);
    let y = if under { cursor.bottom() } else { cursor.top() - size.y.min(above) };
    let x = cursor.left().min(screen.right() - size.x).max(screen.left());
    (egui::pos2(x, y), room)
}

/// A popup at `corner`, its content scrolling past `room` high: where it was drawn, and its whole
/// size, the part scrolled out of view included, for `place` to choose a side next frame by what
/// it holds, not by what last fit (it would stay on a side too short for it).
fn popup(ui: &Ui, id: Id, corner: egui::Pos2, room: f32, content: impl FnOnce(&mut Ui)) -> (egui::Rect, egui::Vec2) {
    let area = egui::Area::new(id).order(egui::Order::Foreground).fixed_pos(corner).show(ui.ctx(), |ui| {
        egui::Frame::popup(ui.style())
            .show(ui, |ui| {
                // The area starts from its last size, so after a short list the scroll area would get
                // only that room back, scrolling with room to spare: ask for the full height whenever
                // it scrolls, as Command Search does (#255). Short content still shrinks it to fit.
                let out =
                    egui::ScrollArea::vertical().max_height(room).min_scrolled_height(room).auto_shrink([true, true]).show(ui, content);
                (out.content_size.y - out.inner_rect.height()).max(0.0)
            })
            .inner
    });
    let rect = area.response.rect;
    (rect, rect.size() + egui::vec2(0.0, area.inner))
}

/// The ids in `ids` (the commands a script can run, `script_ids`) that complete `word`, typed
/// from char `start` to `end`, case aside; the id typed whole isn't offered again.
fn complete(start: usize, end: usize, word: &str, ids: &[String]) -> Option<Offer> {
    let mut items: Vec<String> = ids
        .iter()
        .filter(|id| id.get(..word.len()).is_some_and(|p| p.eq_ignore_ascii_case(word)) && !id.eq_ignore_ascii_case(word))
        .cloned()
        .collect();
    items.sort();
    items.truncate(MAX_ITEMS);
    (!items.is_empty()).then_some(Offer { start, end, items })
}

/// `script_ids`, kept until the loaded plugins or the session's commands change (the built-in
/// commands don't while the app runs; their count tells another session's registry apart).
fn script_ids_cached(ui: &Ui, session: &rustgis_engine::Session) -> std::sync::Arc<Vec<String>> {
    type Kept = ((u64, usize), std::sync::Arc<Vec<String>>);
    let key = Id::new("script_complete_ids");
    let now = (rustgis_engine::plugins::revision(), session.registry.commands.len());
    if let Some((was, ids)) = ui.data(|d| d.get_temp::<Kept>(key))
        && was == now
    {
        return ids;
    }
    let ids = std::sync::Arc::new(script_ids(session));
    ui.data_mut(|d| d.insert_temp::<Kept>(key, (now, ids.clone())));
    ids
}

/// The ids a script can run: the built-in commands and enabled plugins' (`plugin:…`), less those a
/// script refuses.
pub fn script_ids(session: &rustgis_engine::Session) -> Vec<String> {
    let builtin = session.registry.commands.iter().map(|c| c.id.to_string());
    let plugins = rustgis_engine::plugins::commands().into_iter().map(|c| c.id);
    builtin.chain(plugins).filter(|id| !refused_in_script(id)).collect()
}

/// What the help beside the list says about a command (#662), most useful first: its signature
/// (required keys bare, optional ones with `?`), its name and help (a plugin's command has a help
/// text; a built-in one, its label), its parameters one per line as described, and last where it
/// sits in the interface.
#[derive(Clone, Debug, PartialEq)]
pub struct Help {
    pub id: String,
    /// The keys after the id, as the signature reads (`labels.style` then ` {layer?, size?}`);
    /// empty for a command with none.
    pub keys: String,
    pub label: String,
    pub about: String,
    pub params: Vec<String>,
    pub location: String,
}

/// The help for command `id`, as Command Search names and places it (translated); None for an id
/// no command has.
pub fn help(session: &rustgis_engine::Session, id: &str) -> Option<Help> {
    let lang = crate::i18n::current().code();
    let (label, about, location, desc) = match session.registry.get(id) {
        Some(c) => (tl!(c.label).to_string(), String::new(), crate::i18n::location(c.location), c.params.to_string()),
        None => {
            let c = rustgis_engine::plugins::command(id)?;
            let m = &c.plugin.manifest;
            let location = format!("{} › {}", tl!("Plugins"), m.translate(lang, &m.name));
            (m.translate(lang, &c.decl.label).to_string(), m.translate(lang, &c.decl.help).to_string(), location, c.decl.params.clone())
        }
    };
    let params = rustgis_engine::param_list(&desc).unwrap_or_default();
    let names: Vec<String> = params.iter().map(|p| format!("{}{}", p.key, if p.optional { "?" } else { "" })).collect();
    let keys = if names.is_empty() { String::new() } else { format!(" {{{}}}", names.join(", ")) };
    Some(Help { id: id.to_string(), keys, label, about, params: params.iter().map(|p| p.text.to_string()).collect(), location })
}

/// The parameter description of command `id`: a built-in command's, or an enabled plugin's.
fn params_of(session: &rustgis_engine::Session, id: &str) -> Option<String> {
    match session.registry.get(id) {
        Some(c) => Some(c.params.to_string()),
        None => rustgis_engine::plugins::command(id).map(|c| c.decl.params),
    }
}

/// What accepting a command whose parameters `desc` describes puts after its id, and where the
/// cursor goes in it (chars): its required keys, each with its value to type, the cursor at the
/// first (` {"lon": |, "lat": }`); empty braces when every key may be left out (` {|}`); nothing
/// for a command with no parameters, or a description that isn't an object (no keys to start
/// from).
pub fn template(desc: &str) -> (String, usize) {
    let params = rustgis_engine::param_list(desc).unwrap_or_default();
    if params.is_empty() {
        return (String::new(), 0);
    }
    let required: Vec<String> = params.iter().filter(|p| !p.optional).map(|p| format!("\"{}\": ", p.key)).collect();
    let Some(first) = required.first() else { return (" {}".to_string(), 2) };
    // After ` {` and the first key.
    let at = 2 + first.chars().count();
    (format!(" {{{}}}", required.join(", ")), at)
}

/// The help's widest line, so a long signature or parameter wraps rather than widen the list
/// without end.
const HELP_WIDTH: f32 = 360.0;
/// The narrowest the help gets beside the list; with less room it goes under the list.
const HELP_MIN_WIDTH: f32 = 200.0;
/// The gap between the list and the help, and the popup frame's margins (both sides).
const GAP: f32 = 12.0;
const FRAME: f32 = 24.0;

/// Draws `help` beside the list: the signature and parameters coloured as the script is, in its
/// monospace font (smaller for the parameters).
fn draw_help(ui: &mut Ui, help: &Help, width: f32) {
    let palette = crate::scriptcolor::Palette::of(&crate::theme::Tokens::get(ui.ctx()));
    let mono = egui::TextStyle::Monospace.resolve(ui.style());
    let small = egui::FontId::monospace(mono.size * 0.85);
    let coloured = |id: Option<&str>, text: &str, font: &egui::FontId| {
        let mut job = crate::scriptcolor::help_job(id, text, palette, font);
        job.wrap.max_width = width;
        job
    };
    ui.vertical(|ui| {
        ui.set_max_width(width);
        ui.label(coloured(Some(&help.id), &help.keys, &mono));
        ui.label(&help.label);
        if !help.about.is_empty() {
            ui.label(egui::RichText::new(&help.about).small());
        }
        if !help.params.is_empty() {
            ui.add_space(4.0);
            for p in &help.params {
                ui.label(coloured(None, p, &small));
            }
        }
        ui.add_space(4.0);
        ui.label(egui::RichText::new(&help.location).small().weak());
    });
}

/// The list's state between frames, kept in one place.
#[derive(Clone, Default)]
struct List {
    /// What was worked out to offer, and for which cursor, script (its hash: one replaced from
    /// outside the box, as long as before, still tells) and plugins (their revision: a plugin
    /// turned off mustn't stay on offer, to be accepted). While none changes (the cursor
    /// blinking, the pointer moving) it isn't worked out again. Kept whether the list shows or not.
    /// The hash is taken each frame the box has focus: the length alone misses a script replaced by
    /// one as long, and a change made by code comes with no input event to wait for. It reads the
    /// text once, as egui's own layout already does each frame (the layouter's cache key, the
    /// galley cache), so it adds a constant, not a new order of cost.
    worked: Option<Worked<u64, Option<Offer>>>,
    /// The list shown, and its highlighted item: what the keys act on.
    open: Option<(Offer, usize)>,
    /// The offer Escape closed: not shown again until what's typed changes.
    dismissed: Option<Offer>,
    /// Where the list was drawn last frame: a press on it takes the focus from the box (a click
    /// elsewhere), so the list stays while the pointer is on it, for the press to reach it. And
    /// its whole size, scroll aside (`popup`): where it goes.
    drawn: Option<(egui::Rect, egui::Vec2)>,
    /// Whether the help follows the item under the pointer (it moved last) or the highlighted one
    /// (an arrow key moved it last): a pointer resting on the list mustn't hold the help while
    /// the arrows move the highlight.
    pointer_leads: bool,
    /// The highlighted item the list last scrolled to: it scrolls to a new one only (so a wheel
    /// can still move it).
    scrolled_to: Option<usize>,
    /// The help by the cursor (`hint`).
    hint: HintState,
}

/// What was worked out at a cursor, and for which script (its hash) and `with` (what else it
/// depends on: the plugins' revision for the list; the language and the plugins' revision for
/// the help): worked out again only when one changes.
#[derive(Clone)]
struct Worked<W, T> {
    at: Option<usize>,
    text: u64,
    with: W,
    value: T,
}

impl<W: PartialEq, T: Clone> Worked<W, T> {
    /// The value, if worked out for this cursor, script and `with`.
    fn get(this: &Option<Self>, at: Option<usize>, text: u64, with: &W) -> Option<T> {
        this.as_ref().filter(|w| w.at == at && w.text == text && w.with == *with).map(|w| w.value.clone())
    }
}

/// The help shown by the cursor while it's in a line's `{…}`.
#[derive(Clone, Default)]
struct HintState {
    /// The command it's for: what Escape acts on.
    shown: Option<Hint>,
    /// What Escape closed: not shown again until the cursor leaves those braces.
    dismissed: Option<Hint>,
    /// Its whole size last frame, scroll aside (`popup`): where it goes.
    size: Option<egui::Vec2>,
    /// What it was worked out to show, for which cursor, script (its hash), language (the help is
    /// translated) and plugins' revision (a plugin's command comes and goes with its plugin): not
    /// again each frame the cursor blinks.
    worked: Option<Worked<(&'static str, u64), Option<(Hint, Help)>>>,
}

fn load(ui: &Ui, box_id: Id) -> List {
    ui.data(|d| d.get_temp::<List>(box_id.with("complete"))).unwrap_or_default()
}

fn store(ui: &Ui, box_id: Id, list: List) {
    ui.data_mut(|d| d.insert_temp(box_id.with("complete"), list));
}

/// The keys the box keeps while it has focus: code_editor's (the arrows, Tab to indent) and, with
/// the list or the help by the cursor open, Escape, which egui would otherwise take as "drop the
/// focus" before `keys` can read it as "close it".
pub fn filter(ui: &Ui, box_id: Id) -> egui::EventFilter {
    let list = load(ui, box_id);
    egui::EventFilter {
        tab: true,
        horizontal_arrows: true,
        vertical_arrows: true,
        escape: list.open.is_some() || list.hint.shown.is_some(),
    }
}

/// Before the box sees the keys: with the list open and the box focused, ↑/↓ move the highlight,
/// Enter or Tab accept it (so Enter doesn't break the line, nor Tab indent), Escape closes the list
/// (and the box keeps the focus). Returns the item to accept, if any. A list worked out for
/// another text than `script` (changed from outside the box since) is dropped, not acted on: its
/// word could be elsewhere now.
pub fn keys(ui: &Ui, box_id: Id, script: &str) -> Option<(Offer, String)> {
    let mut list = load(ui, box_id);
    // The help by the cursor (no list open): Escape closes it.
    if list.open.is_none() && list.hint.shown.is_some() && ui.memory(|m| m.has_focus(box_id)) {
        if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            list.hint.dismissed = list.hint.shown.take();
            list.hint.size = None;
            store(ui, box_id, list);
        }
        return None;
    }
    let (offer, selected) = list.open.clone()?;
    if !ui.memory(|m| m.has_focus(box_id)) {
        return None;
    }
    if list.worked.as_ref().is_none_or(|w| w.text != egui::util::hash(script)) {
        list.open = None;
        store(ui, box_id, list);
        return None;
    }
    let n = offer.items.len();
    let (up, down, accept, close) = ui.input_mut(|i| {
        (
            i.consume_key(Modifiers::NONE, Key::ArrowUp),
            i.consume_key(Modifiers::NONE, Key::ArrowDown),
            i.consume_key(Modifiers::NONE, Key::Enter) || i.consume_key(Modifiers::NONE, Key::Tab),
            i.consume_key(Modifiers::NONE, Key::Escape),
        )
    });
    let picked = if close {
        list.open = None;
        list.dismissed = Some(offer);
        None
    } else if accept {
        list.open = None;
        offer.items.get(selected).cloned().map(|item| (offer, item))
    } else {
        if up || down {
            let selected = if up { (selected + n - 1) % n } else { (selected + 1) % n };
            list.open = Some((offer, selected));
            list.pointer_leads = false;
        }
        None
    };
    store(ui, box_id, list);
    picked
}

/// After the box: offers for the word at its cursor (a new offer keeps the highlight on the same
/// item, when it is still there), and the list drawn just under the cursor. `visible` is the part
/// of the box that shows (its scroll's viewport: the text edit's own clip rect is its whole text).
/// Returns an item clicked, to accept. The list closes when the box loses focus or nothing matches.
pub fn show(
    ui: &Ui,
    box_id: Id,
    out: &TextEditOutput,
    visible: egui::Rect,
    script: &str,
    session: &rustgis_engine::Session,
) -> Option<(Offer, String)> {
    let mut list = load(ui, box_id);
    // The window left (another app took the focus, the box keeps its own): the list closes, as a
    // code editor's does, and comes back only once what's typed or the cursor changes.
    if !ui.input(|i| i.focused) {
        if let Some((offer, _)) = list.open.take() {
            list.dismissed = Some(offer);
        }
        list.drawn = None;
        store(ui, box_id, list);
        return None;
    }
    // A press on the list: only then does it outlive the box's focus.
    let on_list =
        list.drawn.is_some_and(|(r, _)| ui.input(|i| i.pointer.any_down() && i.pointer.hover_pos().is_some_and(|p| r.contains(p))));
    let offer = if out.response.response.has_focus() {
        // A cursor, not a selection: with text selected, Enter and Tab act on it as usual.
        let at = out.cursor_range.filter(|r| r.is_empty()).map(|r| r.primary.index.0);
        let (text, plugins) = (egui::util::hash(script), rustgis_engine::plugins::revision());
        let kept = Worked::get(&list.worked, at, text, &plugins);
        match kept {
            Some(offer) => offer,
            None => {
                // The ids only once there's a word to complete.
                let offer = at
                    .and_then(|at| word_at(script, at).map(|w| (at, w)))
                    .and_then(|(at, (start, word))| complete(start, at, &word, &script_ids_cached(ui, session)));
                list.worked = Some(Worked { at, text, with: plugins, value: offer.clone() });
                offer
            }
        }
    } else if on_list {
        // Pressed on the list: the box let go of the focus (and maybe of its cursor), so the list
        // shown stays as it is until the press reaches it.
        list.open.as_ref().map(|(offer, _)| offer.clone())
    } else {
        None
    };
    // Closed with Escape: not again for the same word.
    if offer.is_some() && list.dismissed == offer {
        list.open = None;
        list.drawn = None;
        store(ui, box_id, list);
        return None;
    }
    list.dismissed = None;
    // A cursor scrolled out of the box's view shows no list, and keeps none open: Enter mustn't
    // accept a suggestion no one sees. It comes back with the cursor, as `worked` is kept.
    let cursor = offer.as_ref().map(|o| out.galley.pos_from_cursor(CCursor::new(o.end)).translate(out.galley_pos.to_vec2()));
    let (Some(offer), Some(cursor)) = (offer, cursor.filter(|c| visible.intersects(*c))) else {
        list.open = None;
        list.drawn = None;
        store(ui, box_id, list);
        return None;
    };
    if ui.input(|i| i.pointer.delta() != egui::Vec2::ZERO) {
        list.pointer_leads = true;
    }
    let pointer_leads = list.pointer_leads;
    // A new list starts from its top: its scroll is the old list's otherwise.
    if list.open.as_ref().is_none_or(|(o, _)| o.items != offer.items) {
        list.scrolled_to = None;
    }
    let kept = list.open.as_ref().and_then(|(o, s)| o.items.get(*s));
    let selected = kept.and_then(|item| offer.items.iter().position(|i| i == item)).unwrap_or(0);
    // Just under the cursor, or above it when the list would run off the bottom; kept on screen
    // sideways. Its size is last frame's (a long plugin id, another font), estimated only on the
    // first frame it shows.
    let row = ui.text_style_height(&egui::TextStyle::Monospace) + ui.spacing().item_spacing.y + 4.0;
    let size = list.drawn.map_or(egui::vec2(MIN_WIDTH + 16.0, row * offer.items.len() as f32 + 12.0), |(_, s)| s);
    let screen = ui.ctx().content_rect();
    let (corner, room) = place(cursor, size, screen, row * 3.0);
    // The help beside the list in the room the screen leaves, or under it when that's too narrow
    // to read (a narrow window), so neither is cut off at the screen's edge.
    let beside = (screen.width() - MIN_WIDTH - GAP - FRAME).min(HELP_WIDTH);
    let (beside, help_width) =
        if beside >= HELP_MIN_WIDTH { (true, beside) } else { (false, (screen.width() - FRAME).clamp(0.0, HELP_WIDTH)) };
    let layout = if beside { egui::Layout::left_to_right(egui::Align::Min) } else { egui::Layout::top_down(egui::Align::Min) };
    let mut clicked = None;
    let (area, size) = popup(ui, box_id.with("complete_list"), corner, room, |ui| {
        ui.with_layout(layout, |ui| {
            // The help follows the item under the pointer, else the highlighted one.
            let mut hovered = None;
            ui.vertical(|ui| {
                ui.set_min_width(MIN_WIDTH);
                for (i, item) in offer.items.iter().enumerate() {
                    let r = ui.selectable_label(i == selected, egui::RichText::new(item).monospace());
                    // Taken as the button goes down, as in a code editor: the press already
                    // takes the focus from the box, and a click (down, then up a frame or more
                    // later) could find the list gone by then. A click still counts
                    // (assistive technology).
                    if r.clicked() || (r.hovered() && ui.input(|i| i.pointer.primary_pressed())) {
                        clicked = Some((offer.clone(), item.clone()));
                    }
                    if r.hovered() {
                        hovered = Some(i);
                    }
                    if i == selected && list.scrolled_to != Some(selected) {
                        r.scroll_to_me(None);
                        list.scrolled_to = Some(selected);
                    }
                }
            });
            // Worked out each frame the list shows, for one command: a registry lookup and a
            // short description split. Kept, it would need the id and the language as its
            // key (the help is translated), for next to nothing saved.
            let shown = hovered.filter(|_| pointer_leads).unwrap_or(selected);
            if let Some(help) = offer.items.get(shown).and_then(|id| help(session, id)) {
                // A gap, not a separator: in a row, egui's separator takes all the height it
                // can, and would stretch the list down to the bottom of the screen.
                ui.add_space(GAP);
                draw_help(ui, &help, help_width);
            }
        });
    });
    if clicked.is_some() {
        list.open = None;
        list.drawn = None;
    } else {
        list.open = Some((offer, selected));
        list.drawn = Some((area, size));
    }
    store(ui, box_id, list);
    clicked
}

/// After the list: with no list open, the help of the command whose parameters the cursor is in
/// (its line's `{…}`), floating by the cursor, so the signature and parameters stay in view while
/// they're typed. Not with text selected, a cursor out of view, the window left, or once Escape
/// closed it (until the cursor leaves those braces).
pub fn hint(ui: &Ui, box_id: Id, out: &TextEditOutput, visible: egui::Rect, script: &str, session: &rustgis_engine::Session) {
    let mut list = load(ui, box_id);
    let focused = out.response.response.has_focus() && ui.input(|i| i.focused);
    let at = out.cursor_range.filter(|r| focused && list.open.is_none() && r.is_empty()).map(|r| r.primary.index.0);
    // Its own hash of the script, not the list's: it mustn't depend on how `show` ran this frame.
    let text = egui::util::hash(script);
    // The help is translated, and a plugin's command comes and goes with its plugin.
    let with = (crate::i18n::current().code(), rustgis_engine::plugins::revision());
    let kept = Worked::get(&list.hint.worked, at, text, &with);
    let found = match kept {
        Some(found) => found,
        None => {
            let found = at.and_then(|at| hint_at(script, at)).and_then(|h| Some((h.clone(), help(session, &h.id)?)));
            list.hint.worked = Some(Worked { at, text, with, value: found.clone() });
            found
        }
    };
    // Out of the braces, what Escape closed may show again next time.
    if found.is_none() {
        list.hint.dismissed = None;
    }
    // Anchored at the line's `{` sideways, so it doesn't slide with each key typed; on the
    // cursor's line, which it doesn't cover.
    let galley_rect = |i: usize| out.galley.pos_from_cursor(CCursor::new(i)).translate(out.galley_pos.to_vec2());
    let shown = found.filter(|(h, _)| list.hint.dismissed.as_ref() != Some(h)).and_then(|(h, help)| {
        let cursor = galley_rect(at?);
        let brace = galley_rect(h.brace).left();
        let anchor = egui::Rect::from_x_y_ranges(brace..=brace, cursor.y_range());
        visible.intersects(cursor).then_some((anchor, h, help))
    });
    let Some((cursor, at_line, help)) = shown else {
        list.hint.shown = None;
        list.hint.size = None;
        store(ui, box_id, list);
        return;
    };
    let row = ui.text_style_height(&egui::TextStyle::Monospace) + ui.spacing().item_spacing.y + 4.0;
    let screen = ui.ctx().content_rect();
    let width = (screen.width() - FRAME).clamp(0.0, HELP_WIDTH);
    let size = list.hint.size.unwrap_or(egui::vec2(width + FRAME, row * (help.params.len() + 3) as f32));
    let (corner, room) = place(cursor, size, screen, row * 3.0);
    let (_, size) = popup(ui, box_id.with("complete_hint"), corner, room, |ui| draw_help(ui, &help, width));
    list.hint.shown = Some(at_line);
    list.hint.size = Some(size);
    store(ui, box_id, list);
}

/// Puts `item` in place of the offer's word in `script`, with its parameters' `template` when
/// nothing follows the word on its line (a line that has parameters keeps them: only its id
/// changes), the cursor where the first value goes; as one undo step of the box (its own undo
/// takes the pick back), and gives the box the focus again.
pub fn accept(ui: &Ui, box_id: Id, script: &mut String, offer: &Offer, item: &str, session: &rustgis_engine::Session) {
    let chars: Vec<char> = script.chars().collect();
    let (Some(before), Some(after)) = (chars.get(..offer.start), chars.get(offer.end..)) else { return };
    let bare = after.iter().take_while(|c| **c != '\n').all(|c| c.is_whitespace());
    let (tail, at) = if bare { params_of(session, item).map(|d| template(&d)).unwrap_or_default() } else { Default::default() };
    let mut new: String = before.iter().collect();
    new.push_str(item);
    new.push_str(&tail);
    new.extend(after);
    let mut state = TextEditState::load(ui.ctx(), box_id).unwrap_or_default();
    let mut undoer = state.undoer();
    let was = state.cursor.char_range().unwrap_or(CCursorRange::one(CCursor::new(offer.end)));
    undoer.add_undo(&(was, script.clone()));
    state.set_undoer(undoer);
    *script = new;
    state.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(offer.start + item.chars().count() + at))));
    state.store(ui.ctx(), box_id);
    ui.memory_mut(|m| m.request_focus(box_id));
    // What was worked out was for the text before the pick.
    let mut list = load(ui, box_id);
    list.worked = None;
    store(ui, box_id, list);
}

#[cfg(test)]
mod tests {
    use super::{Offer, offer};

    /// The ids as the pane has them (`script_ids`): those a script refuses already left out.
    fn ids() -> Vec<String> {
        ["labels.style", "labels.placement", "labels.class.add", "map.zoomFull", "plugin:demo/hello"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    fn items(o: Option<Offer>) -> Vec<String> {
        o.map(|o| o.items).unwrap_or_default()
    }

    #[test]
    fn a_lines_first_word_offers_the_ids_it_starts() {
        let s = "map.zoomFull\n  labels.";
        let o = offer(s, s.chars().count(), &ids()).unwrap();
        assert_eq!(o.items, ["labels.class.add", "labels.placement", "labels.style"]);
        assert_eq!((o.start, o.end), (15, 22), "the word, after the indent");
        assert_eq!(items(offer("LABELS.ST", 9, &ids())), ["labels.style"], "case aside");
        assert_eq!(items(offer("plu", 3, &ids())), ["plugin:demo/hello"], "plugins' too");
    }

    #[test]
    fn only_a_lines_first_word_at_the_cursor_is_completed() {
        let none = |s: &str, at: usize| offer(s, at, &ids()).is_none();
        assert!(none("labels.style {\"la", 17), "in the JSON");
        assert!(none("# labels", 8), "in a comment");
        assert!(none("l", 1), "one character");
        assert!(none("labels.st", 4), "the cursor mid-word");
        assert!(none("labels.style", 12), "the id typed whole");
        assert!(none("ui.pa", 5) && none("script.r", 8), "ids a script refuses aren't among them");
        assert!(none("nothing", 7), "no match");
        assert!(none("map", 99), "a cursor past the text");
    }

    #[test]
    fn char_indexes_with_multibyte_text_and_crlf() {
        let s = "# Bogotá 🌎\r\nlabels.pl";
        let at = s.chars().count();
        let o = offer(s, at, &ids()).unwrap();
        assert_eq!(o.items, ["labels.placement"]);
        let start: String = s.chars().skip(o.start).take(o.end - o.start).collect();
        assert_eq!(start, "labels.pl");
    }

    #[test]
    fn the_list_is_capped() {
        let many: Vec<String> = (0..30).map(|i| format!("map.cmd{i:02}")).collect();
        assert_eq!(items(offer("map.cm", 6, &many)).len(), super::MAX_ITEMS);
    }

    /// The pane with an empty script, its box focused.
    fn pane() -> egui_kittest::Harness<'static, crate::RustGisApp> {
        use egui_kittest::kittest::Queryable;
        let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(500.0, 700.0)).build_ui_state(
            |ui, app: &mut crate::RustGisApp| crate::analysispane::script_pane(app, ui, &crate::theme::Tokens::light()),
            crate::RustGisApp::with_sample(),
        );
        h.run();
        h.get_by_role(egui::accesskit::Role::MultilineTextInput).focus();
        h.run();
        h
    }

    fn type_in(h: &mut egui_kittest::Harness<'static, crate::RustGisApp>, text: &str) {
        h.event(egui::Event::Text(text.into()));
        h.run();
    }

    /// Typing a line's first word opens the list; Enter accepts the highlighted id, with no line
    /// break, and the list closes.
    #[test]
    fn enter_accepts_the_highlighted_id() {
        use egui_kittest::kittest::Queryable;
        let mut h = pane();
        type_in(&mut h, "labels.sty");
        h.get_by_label("labels.style");
        h.key_press(egui::Key::Enter);
        h.run();
        assert_eq!(h.state().workflow.script, "labels.style {}", "with its parameters' braces");
        assert!(h.query_by_label("labels.style").is_none(), "the list closed");
    }

    /// The arrows move the highlight; Tab accepts it too, inserting no tab.
    #[test]
    fn the_arrows_move_and_tab_accepts() {
        let mut h = pane();
        type_in(&mut h, "labels.");
        h.key_press(egui::Key::ArrowDown);
        h.run();
        h.key_press(egui::Key::Tab);
        h.run();
        let session = &h.state().session;
        let second = super::offer("labels.", 7, &super::script_ids(session)).unwrap().items[1].clone();
        let (template, _) = super::template(&super::params_of(session, &second).unwrap());
        assert_eq!(h.state().workflow.script, format!("{second}{template}"), "the second item, and no tab");
    }

    /// Escape closes the list without touching the text, and it stays closed for that word.
    #[test]
    fn escape_closes_the_list_for_that_word() {
        use egui_kittest::kittest::Queryable;
        let mut h = pane();
        type_in(&mut h, "labels.sty");
        h.key_press(egui::Key::Escape);
        h.run();
        assert!(h.query_by_label("labels.style").is_none());
        assert_eq!(h.state().workflow.script, "labels.sty");
        h.run();
        assert!(h.query_by_label("labels.style").is_none(), "not opened again");
        type_in(&mut h, "l");
        h.get_by_label("labels.style");
    }

    /// A click accepts an item, and the box keeps the focus. Pressed and let go in frames of their
    /// own, as a real click is (`click()` does both in one frame, which hid that the press takes
    /// the focus from the box first).
    #[test]
    fn a_click_accepts_an_item() {
        use egui_kittest::kittest::Queryable;
        let mut h = pane();
        type_in(&mut h, "map.zoomF");
        let at = h.get_by_role_and_label(egui::accesskit::Role::Button, "map.zoomFull").rect().center();
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        h.event(egui::Event::PointerMoved(at));
        h.step();
        h.event(button(true));
        h.step();
        h.step();
        h.event(button(false));
        h.run();
        assert_eq!(h.state().workflow.script, "map.zoomFull");
        assert!(h.get_by_role(egui::accesskit::Role::MultilineTextInput).is_focused());
    }

    /// The box's own undo takes a pick back.
    #[test]
    fn a_pick_is_one_undo_step_of_the_box() {
        let mut h = pane();
        type_in(&mut h, "labels.sty");
        h.key_press(egui::Key::Enter);
        h.run();
        let state = egui::widgets::text_edit::TextEditState::load(&h.ctx, egui::Id::new("script_text")).unwrap();
        let now = (state.cursor.char_range().unwrap(), h.state().workflow.script.clone());
        let mut undoer = state.undoer();
        let back = undoer.undo(&now).map(|(_, text)| text.clone());
        assert_eq!(back.as_deref(), Some("labels.sty"), "the id and its template together");
    }

    /// The template: the required keys, the cursor at the first one's value; empty braces when
    /// every key may be left out; nothing with no keys.
    #[test]
    fn the_template_starts_at_the_first_required_key() {
        use super::template;
        let go_to = r#"{"lon" | "x": number, "lat" | "y": number, "scale"?: number}"#;
        assert_eq!(template(go_to), (r#" {"lon": , "lat": }"#.to_string(), 9), "every required key, the first of its alternatives");
        assert_eq!(template(r#"{"size"?: number, "field": string}"#), (r#" {"field": }"#.to_string(), 11), "not the first key");
        assert_eq!(template(r#"{"size"?: number}"#), (" {}".to_string(), 2));
        assert_eq!(template("{}"), (String::new(), 0));
        assert_eq!(template("a path"), (String::new(), 0), "a description that isn't an object");
        assert_eq!(template(r#"{"größe": number}"#), (r#" {"größe": }"#.to_string(), 11), "chars, not bytes");
    }

    /// Accepting a command with a required key puts the cursor where its value goes, and the
    /// help by the cursor shows at once; a line further on after multibyte text.
    #[test]
    fn accepting_leaves_the_cursor_at_the_first_value() {
        use egui_kittest::kittest::Queryable;
        let mut h = pane();
        type_in(&mut h, "# ñandú\nmap.goToX");
        h.key_press(egui::Key::Enter);
        h.run();
        assert_eq!(h.state().workflow.script, "# ñandú\nmap.goToXY {\"lon\": , \"lat\": }");
        let help = super::help(&h.state().session, "map.goToXY").unwrap();
        h.get_by_label(&format!("{}{}", help.id, help.keys));
        type_in(&mut h, "2");
        assert_eq!(h.state().workflow.script, "# ñandú\nmap.goToXY {\"lon\": 2, \"lat\": }");
    }

    /// A command with no parameters gets nothing after its id; a line that already has
    /// parameters keeps them, and only its id changes.
    #[test]
    fn a_line_with_parameters_keeps_them() {
        let mut h = pane();
        type_in(&mut h, "map.zoomF");
        h.key_press(egui::Key::Enter);
        h.run();
        assert_eq!(h.state().workflow.script, "map.zoomFull");
        let mut h = pane();
        type_in(&mut h, r#"map.goTo {"lat": 1}"#);
        for _ in 0..r#" {"lat": 1}"#.len() {
            h.key_press(egui::Key::ArrowLeft);
        }
        type_in(&mut h, "X");
        h.key_press(egui::Key::Enter);
        h.run();
        assert_eq!(h.state().workflow.script, r#"map.goToXY {"lat": 1}"#);
    }

    /// Moving the cursor, with no text changed, works the list out again: into the word it
    /// closes, back to its end it opens.
    #[test]
    fn moving_the_cursor_opens_and_closes_the_list() {
        use egui_kittest::kittest::Queryable;
        let mut h = pane();
        type_in(&mut h, "labels.sty");
        h.get_by_label("labels.style");
        h.key_press(egui::Key::ArrowLeft);
        h.run();
        assert!(h.query_by_label("labels.style").is_none(), "mid-word");
        h.key_press(egui::Key::ArrowRight);
        h.run();
        h.get_by_label("labels.style");
    }

    /// A cursor scrolled out of the box's view keeps no list open: Enter breaks the line rather
    /// than accept a suggestion no one sees.
    #[test]
    fn a_hidden_cursor_keeps_no_list() {
        use egui_kittest::kittest::Queryable;
        let mut h = pane();
        // Pasted at once, the box can't scroll all the way to the cursor on that frame (its scroll
        // is bounded by the content it had before), so one more key is typed, as a person would:
        // that scrolls the box to the cursor with the whole text in it. Scrolls are eased.
        h.event(egui::Event::Text(format!("{}labels.st", "map.zoomFull\n".repeat(200))));
        h.run_steps(60);
        h.event(egui::Event::Text("y".into()));
        h.run_steps(60);
        h.get_by_label("labels.style");
        // Scroll the box back to its top, away from the cursor.
        let at = h.get_by_role(egui::accesskit::Role::MultilineTextInput).rect().center();
        h.event(egui::Event::PointerMoved(egui::pos2(at.x, 200.0)));
        h.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, 100_000.0),
            modifiers: egui::Modifiers::NONE,
            phase: egui::TouchPhase::Move,
        });
        // egui eases a wheel's scroll over frames, asking for a repaint until it's done.
        h.run_steps(60);
        assert!(h.query_by_label("labels.style").is_none(), "no list for a hidden cursor");
        h.key_press(egui::Key::Enter);
        // The box scrolls back to its cursor, eased too.
        h.run_steps(60);
        assert!(h.state().workflow.script.ends_with("labels.sty\n"), "Enter broke the line");
    }

    /// The ids the pane offers leave out what a script refuses (the one rule, `refused_in_script`).
    #[test]
    fn script_ids_leave_out_what_a_script_refuses() {
        let app = crate::RustGisApp::with_sample();
        let ids = super::script_ids(&app.session);
        assert!(ids.iter().any(|i| i == "map.zoomFull"));
        assert!(!ids.iter().any(|i| rustgis_engine::cmd::workflows::refused_in_script(i)), "no script.*, ui.*, notebook.run");
    }

    /// With text selected there's no list: Enter and Tab act on the selection as usual.
    #[test]
    fn a_selection_shows_no_list() {
        use egui_kittest::kittest::Queryable;
        let mut h = pane();
        type_in(&mut h, "labels.sty");
        h.get_by_label("labels.style");
        h.key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::ArrowLeft);
        h.run();
        assert!(h.query_by_label("labels.style").is_none(), "a selection closes it");
        h.key_press(egui::Key::Enter);
        h.run();
        assert_eq!(h.state().workflow.script, "labels.st\n", "Enter replaced the selection");
    }

    /// Leaving the window closes the list, and coming back doesn't open it again until what's
    /// typed changes (the box keeps its focus meanwhile, as egui keeps a widget's).
    #[test]
    fn leaving_the_window_closes_the_list() {
        use egui_kittest::kittest::Queryable;
        let mut h = pane();
        type_in(&mut h, "labels.sty");
        h.get_by_label("labels.style");
        // As eframe tells it: `RawInput::focused`, which `input.focused` comes from (the
        // `WindowFocused` event alone doesn't change it).
        h.input_mut().focused = false;
        h.event(egui::Event::WindowFocused(false));
        h.run();
        assert!(h.query_by_label("labels.style").is_none(), "closed while away");
        h.input_mut().focused = true;
        h.event(egui::Event::WindowFocused(true));
        h.run();
        assert!(h.query_by_label("labels.style").is_none(), "not opened again on return");
        type_in(&mut h, "l");
        h.get_by_label("labels.style");
    }

    /// A script replaced from outside the box by one as long, the cursor where it was, is worked
    /// out again: the offer is kept by the text's hash, not its length.
    #[test]
    fn a_script_replaced_as_long_is_worked_out_again() {
        use egui_kittest::kittest::Queryable;
        let mut h = pane();
        type_in(&mut h, "labels.sty");
        h.get_by_label("labels.style");
        // Same length, same cursor (10 chars): another prefix, as History's "copy to script" could.
        h.state_mut().workflow.script = "map.zoomFu".into();
        h.run();
        assert!(h.query_by_label("labels.style").is_none(), "the old offer isn't reused");
        h.get_by_role_and_label(egui::accesskit::Role::Button, "map.zoomFull");
    }

    /// A list left open over a script changed from outside the box isn't acted on: Enter doesn't
    /// put an id where the old word was.
    #[test]
    fn keys_drop_a_list_for_another_text() {
        let mut h = pane();
        type_in(&mut h, "labels.sty");
        // Changed under the open list, the box still focused (no code does this today); longer
        // than the old word, so a stale pick could still be spliced in.
        h.state_mut().workflow.script = "# a longer note".into();
        h.key_press(egui::Key::Enter);
        h.run();
        assert!(!h.state().workflow.script.contains("labels.style"), "{:?}", h.state().workflow.script);
    }

    /// The help puts the signature first, then the name, the parameters one per line and the
    /// location last.
    #[test]
    fn help_shows_the_signature_first() {
        let app = crate::RustGisApp::with_sample();
        let h = super::help(&app.session, "labels.style").unwrap();
        assert!(h.keys.starts_with(" {layer?, class?, size?"), "{}", h.keys);
        assert_eq!(h.label, "Text Symbol");
        assert!(h.params.iter().any(|p| p == r#""size"?: number"#), "{:?}", h.params);
        assert_eq!(h.location, "Style › Text Symbol");
        // Required keys go bare.
        assert!(super::help(&app.session, "labels.field").unwrap().keys.contains("field}"));
        // No parameters: the id alone.
        let z = super::help(&app.session, "map.zoomFull").unwrap();
        assert_eq!((z.keys.as_str(), z.params.len()), ("", 0));
        assert!(super::help(&app.session, "nope.cmd").is_none());
    }

    /// The help shows beside the list, for the highlighted item, and follows the arrows.
    #[test]
    fn the_help_follows_the_highlight() {
        use egui_kittest::kittest::Queryable;
        let mut h = pane();
        type_in(&mut h, "labels.");
        let first = super::offer("labels.", 7, &super::script_ids(&h.state().session)).unwrap().items;
        let sig = |id: &str| super::help(&h.state().session, id).map(|h| format!("{}{}", h.id, h.keys)).unwrap();
        let (one, two) = (sig(&first[0]), sig(&first[1]));
        h.get_by_label(&one);
        h.key_press(egui::Key::ArrowDown);
        h.run();
        assert!(h.query_by_label(&one).is_none());
        h.get_by_label(&two);
    }

    /// With the pointer resting on the list, the arrows still move the help with the highlight;
    /// moving the pointer gives it back to the item under it.
    #[test]
    fn the_last_to_move_leads_the_help() {
        use egui_kittest::kittest::Queryable;
        let mut h = pane();
        type_in(&mut h, "labels.");
        let items = super::offer("labels.", 7, &super::script_ids(&h.state().session)).unwrap().items;
        let sig = |h: &egui_kittest::Harness<'static, crate::RustGisApp>, id: &str| {
            super::help(&h.state().session, id).map(|x| format!("{}{}", x.id, x.keys)).unwrap()
        };
        // The pointer over the third item: the help follows it.
        let third = h.get_by_role_and_label(egui::accesskit::Role::Button, &items[2]).rect().center();
        // From somewhere first: egui's first pointer position has no delta to move by.
        h.event(egui::Event::PointerMoved(egui::pos2(1.0, 1.0)));
        h.run();
        h.event(egui::Event::PointerMoved(third));
        h.run();
        h.get_by_label(&sig(&h, &items[2]));
        // ↓ with the pointer still: the help follows the highlight (the second item).
        h.key_press(egui::Key::ArrowDown);
        h.run();
        h.get_by_label(&sig(&h, &items[1]));
    }

    /// In a window too narrow for the help beside the list, the help goes under it: neither is
    /// cut off at the edge.
    #[test]
    fn a_narrow_window_puts_the_help_under_the_list() {
        use egui_kittest::kittest::Queryable;
        let width = 380.0;
        let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(width, 700.0)).build_ui_state(
            |ui, app: &mut crate::RustGisApp| crate::analysispane::script_pane(app, ui, &crate::theme::Tokens::light()),
            crate::RustGisApp::with_sample(),
        );
        h.run();
        h.get_by_role(egui::accesskit::Role::MultilineTextInput).focus();
        h.run();
        h.event(egui::Event::Text("labels.sty".into()));
        h.run();
        let item = h.get_by_role_and_label(egui::accesskit::Role::Button, "labels.style").rect();
        let help = super::help(&h.state().session, "labels.style").unwrap();
        let sig = h.get_by_label(&format!("{}{}", help.id, help.keys)).rect();
        assert!(sig.min.y > item.max.y, "under the list: {sig:?} {item:?}");
        assert!(sig.max.x <= width + 0.5, "within the window: {sig:?}");
    }

    /// In a short window the list and help scroll within the room by the cursor: the popup stays
    /// on screen, and ↓ to the last item scrolls it into view.
    #[test]
    fn a_short_window_scrolls_the_list() {
        use egui_kittest::kittest::Queryable;
        let (width, height) = (380.0, 300.0);
        let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(width, height)).build_ui_state(
            |ui, app: &mut crate::RustGisApp| crate::analysispane::script_pane(app, ui, &crate::theme::Tokens::light()),
            crate::RustGisApp::with_sample(),
        );
        h.run();
        h.get_by_role(egui::accesskit::Role::MultilineTextInput).focus();
        h.run();
        h.event(egui::Event::Text("labels.".into()));
        h.run();
        let items = super::offer("labels.", 7, &super::script_ids(&h.state().session)).unwrap().items;
        for _ in 1..items.len() {
            h.key_press(egui::Key::ArrowDown);
            h.run_steps(10);
        }
        let last = h.get_by_role_and_label(egui::accesskit::Role::Button, items.last().unwrap()).rect();
        assert!(last.min.y >= 0.0 && last.max.y <= height, "the last item scrolled into view: {last:?}");
    }

    /// A list that was short once (a single item) grows back for a longer one: egui lays an area
    /// out in last frame's size, and the list's scroll would keep it short, scrolling.
    #[test]
    fn a_short_list_grows_back() {
        use egui_kittest::kittest::Queryable;
        let mut h = pane();
        type_in(&mut h, "labels.sty");
        h.get_by_label("labels.style");
        for _ in 0.."sty".len() {
            h.key_press(egui::Key::Backspace);
            h.run();
        }
        let items = super::offer("labels.", 7, &super::script_ids(&h.state().session)).unwrap().items;
        assert!(items.len() > 3, "a longer list: {items:?}");
        let list = h.ctx.data(|d| d.get_temp::<super::List>(egui::Id::new("script_text").with("complete"))).unwrap();
        let (shown, _) = list.drawn.unwrap();
        let last = h.get_by_role_and_label(egui::accesskit::Role::Button, items.last().unwrap()).rect();
        assert!(last.max.y <= shown.max.y, "every item in view, no scroll: {last:?} in {shown:?}");
    }

    /// The cursor is in a command's parameters inside its line's `{…}`: not before the brace, not
    /// past the closing one, and a `}` in a string doesn't close it.
    #[test]
    fn hint_at_finds_the_command_whose_braces_hold_the_cursor() {
        use super::hint_at;
        let at = |s: &str| hint_at(s, s.chars().count()).map(|h| (h.line, h.id));
        assert_eq!(at(r#"labels.style {"size": 1"#), Some((0, "labels.style".into())));
        assert_eq!(at("map.zoomFull\nlabels.style {"), Some((13, "labels.style".into())), "its line's start");
        assert_eq!(hint_at("map.zoomFull\nlabels.style {", 27).map(|h| h.brace), Some(26), "its brace");
        assert_eq!(at(r#"labels.style {"x": "a}b", "#), Some((0, "labels.style".into())), "a brace in a string");
        assert_eq!(at(r#"labels.style {"size": 1}"#), None, "past the closing brace");
        assert_eq!(at("labels.style "), None, "before the brace");
        assert_eq!(at("# labels.style {"), None, "a comment");
        assert_eq!(at(r#"labels.style {"a": {"b": 1"#), Some((0, "labels.style".into())), "nested");
        assert!(hint_at("x", 99).is_none(), "a cursor past the text");
    }

    /// Typing a command's parameters keeps its help by the cursor; past the closing brace it's
    /// gone, and Escape closes it until the cursor leaves the braces.
    #[test]
    fn the_help_stays_by_the_cursor_in_the_braces() {
        use egui_kittest::kittest::Queryable;
        let mut h = pane();
        let help = super::help(&h.state().session, "labels.style").unwrap();
        let sig = format!("{}{}", help.id, help.keys);
        type_in(&mut h, "labels.style {");
        h.get_by_label(&sig);
        type_in(&mut h, r#""size": 12"#);
        h.get_by_label(&sig);
        h.key_press(egui::Key::Escape);
        h.run();
        assert!(h.query_by_label(&sig).is_none(), "Escape closes it");
        assert!(h.get_by_role(egui::accesskit::Role::MultilineTextInput).is_focused(), "and the box keeps the focus");
        type_in(&mut h, ", ");
        assert!(h.query_by_label(&sig).is_none(), "not again in the same braces");
        type_in(&mut h, "}");
        assert!(h.query_by_label(&sig).is_none(), "past the closing brace");
        type_in(&mut h, "\nlabels.style {");
        h.get_by_label(&sig);
    }

    /// The help by the cursor stays put sideways while the parameters are typed: it's anchored at
    /// the line's `{`, not at the moving cursor.
    #[test]
    fn the_help_by_the_cursor_doesnt_slide() {
        use egui_kittest::kittest::Queryable;
        let mut h = pane();
        let help = super::help(&h.state().session, "labels.style").unwrap();
        let sig = format!("{}{}", help.id, help.keys);
        type_in(&mut h, "labels.style {");
        let first = h.get_by_label(&sig).rect().left();
        type_in(&mut h, r#""size": 12, "bold": true"#);
        assert!((h.get_by_label(&sig).rect().left() - first).abs() < 0.5, "it didn't move with the cursor");
    }
}
