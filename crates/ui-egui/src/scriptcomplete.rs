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
    // The byte the cursor is at (None past the end).
    let byte = if at == 0 { Some(0) } else { script.char_indices().nth(at - 1).map(|(b, c)| b + c.len_utf8()) }?;
    let (before, after) = script.split_at(byte);
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
    worked: Option<(Option<usize>, u64, u64, Option<Offer>)>,
    /// The list shown, and its highlighted item: what the keys act on.
    open: Option<(Offer, usize)>,
    /// The offer Escape closed: not shown again until what's typed changes.
    dismissed: Option<Offer>,
    /// Where the list was drawn last frame: a press on it takes the focus from the box (a click
    /// elsewhere), so the list stays while the pointer is on it, for the press to reach it.
    rect: Option<egui::Rect>,
}

fn load(ui: &Ui, box_id: Id) -> List {
    ui.data(|d| d.get_temp::<List>(box_id.with("complete"))).unwrap_or_default()
}

fn store(ui: &Ui, box_id: Id, list: List) {
    ui.data_mut(|d| d.insert_temp(box_id.with("complete"), list));
}

/// The keys the box keeps while it has focus: code_editor's (the arrows, Tab to indent) and, with
/// the list open, Escape, which egui would otherwise take as "drop the focus" before `keys` can
/// read it as "close the list".
pub fn filter(ui: &Ui, box_id: Id) -> egui::EventFilter {
    egui::EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: load(ui, box_id).open.is_some() }
}

/// Before the box sees the keys: with the list open and the box focused, ↑/↓ move the highlight,
/// Enter or Tab accept it (so Enter doesn't break the line, nor Tab indent), Escape closes the list
/// (and the box keeps the focus). Returns the item to accept, if any. A list worked out for
/// another text than `script` (changed from outside the box since) is dropped, not acted on: its
/// word could be elsewhere now.
pub fn keys(ui: &Ui, box_id: Id, script: &str) -> Option<(Offer, String)> {
    let mut list = load(ui, box_id);
    let (offer, selected) = list.open.clone()?;
    if !ui.memory(|m| m.has_focus(box_id)) {
        return None;
    }
    if list.worked.as_ref().is_none_or(|w| w.1 != egui::util::hash(script)) {
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
        list.rect = None;
        store(ui, box_id, list);
        return None;
    }
    // A press on the list: only then does it outlive the box's focus.
    let on_list = list.rect.is_some_and(|r| ui.input(|i| i.pointer.any_down() && i.pointer.hover_pos().is_some_and(|p| r.contains(p))));
    let offer = if out.response.response.has_focus() {
        // A cursor, not a selection: with text selected, Enter and Tab act on it as usual.
        let at = out.cursor_range.filter(|r| r.is_empty()).map(|r| r.primary.index.0);
        let (text, plugins) = (egui::util::hash(script), rustgis_engine::plugins::revision());
        let kept = list.worked.as_ref().filter(|w| (w.0, w.1, w.2) == (at, text, plugins)).map(|w| w.3.clone());
        match kept {
            Some(offer) => offer,
            None => {
                // The ids only once there's a word to complete.
                let offer = at
                    .and_then(|at| word_at(script, at).map(|w| (at, w)))
                    .and_then(|(at, (start, word))| complete(start, at, &word, &script_ids_cached(ui, session)));
                list.worked = Some((at, text, plugins, offer.clone()));
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
        list.rect = None;
        store(ui, box_id, list);
        return None;
    }
    list.dismissed = None;
    // A cursor scrolled out of the box's view shows no list, and keeps none open: Enter mustn't
    // accept a suggestion no one sees. It comes back with the cursor, as `worked` is kept.
    let cursor = offer.as_ref().map(|o| out.galley.pos_from_cursor(CCursor::new(o.end)).translate(out.galley_pos.to_vec2()));
    let (Some(offer), Some(cursor)) = (offer, cursor.filter(|c| visible.intersects(*c))) else {
        list.open = None;
        list.rect = None;
        store(ui, box_id, list);
        return None;
    };
    let kept = list.open.as_ref().and_then(|(o, s)| o.items.get(*s));
    let selected = kept.and_then(|item| offer.items.iter().position(|i| i == item)).unwrap_or(0);
    // Just under the cursor, or above it when the list would run off the bottom; kept on screen
    // sideways. Its size is last frame's (a long plugin id, another font), estimated only on the
    // first frame it shows.
    let row = ui.text_style_height(&egui::TextStyle::Monospace) + ui.spacing().item_spacing.y + 4.0;
    let size = list.rect.map_or(egui::vec2(MIN_WIDTH + 16.0, row * offer.items.len() as f32 + 12.0), |r| r.size());
    let screen = ui.ctx().content_rect();
    let y = if cursor.bottom() + size.y > screen.bottom() { cursor.top() - size.y } else { cursor.bottom() };
    let x = cursor.left().min(screen.right() - size.x).max(screen.left());
    let mut clicked = None;
    let area =
        egui::Area::new(box_id.with("complete_list")).order(egui::Order::Foreground).fixed_pos(egui::pos2(x, y)).show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_min_width(MIN_WIDTH);
                for (i, item) in offer.items.iter().enumerate() {
                    let r = ui.selectable_label(i == selected, egui::RichText::new(item).monospace());
                    // Taken as the button goes down, as in a code editor: the press already takes the
                    // focus from the box, and a click (down, then up a frame or more later) could find
                    // the list gone by then. A click still counts (assistive technology).
                    if r.clicked() || (r.hovered() && ui.input(|i| i.pointer.primary_pressed())) {
                        clicked = Some((offer.clone(), item.clone()));
                    }
                }
            });
        });
    if clicked.is_some() {
        list.open = None;
        list.rect = None;
    } else {
        list.open = Some((offer, selected));
        list.rect = Some(area.response.rect);
    }
    store(ui, box_id, list);
    clicked
}

/// Puts `item` in place of the offer's word in `script`, the cursor after it, as one undo step of
/// the box (its own undo takes the pick back), and gives the box the focus again.
pub fn accept(ui: &Ui, box_id: Id, script: &mut String, offer: &Offer, item: &str) {
    let chars: Vec<char> = script.chars().collect();
    let (Some(before), Some(after)) = (chars.get(..offer.start), chars.get(offer.end..)) else { return };
    let mut new: String = before.iter().collect();
    new.push_str(item);
    new.extend(after);
    let mut state = TextEditState::load(ui.ctx(), box_id).unwrap_or_default();
    let mut undoer = state.undoer();
    let was = state.cursor.char_range().unwrap_or(CCursorRange::one(CCursor::new(offer.end)));
    undoer.add_undo(&(was, script.clone()));
    state.set_undoer(undoer);
    *script = new;
    state.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(offer.start + item.chars().count()))));
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
        assert_eq!(h.state().workflow.script, "labels.style");
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
        let second = super::offer("labels.", 7, &super::script_ids(&h.state().session)).unwrap().items[1].clone();
        assert_eq!(h.state().workflow.script, second, "the second item, and no tab");
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
        let at = h.get_by_label("map.zoomFull").rect().center();
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
        assert_eq!(back.as_deref(), Some("labels.sty"));
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
        h.get_by_label("map.zoomFull");
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
}
