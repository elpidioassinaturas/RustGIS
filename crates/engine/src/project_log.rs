//! The project log (#797): the log a project keeps ([`ProjectLog`]), which commands go in, what
//! of them is kept, and how its paths are written portable into a project file and read back.

use std::sync::{Mutex, MutexGuard};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{CmdResult, Session};

/// Now, in milliseconds since 1970 (UTC).
pub(crate) fn now_ms() -> u64 {
    web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Whether command `id`, run by itself with `params`, goes into the log: one that changes the
/// project (all but those built with `.view()`; a plugin's when its manifest asks for the Edit
/// permission), undo and redo, a save, the log's own clearing and switching, and a command whose
/// parameters name a file, folder, address or connection (an export), as the plugin permission
/// gate reads them. Reading and looking around (listing, describing, moving the view) is left
/// out: agents, scripts and plugins call those hundreds of times, and they would push the changes
/// out of the log.
pub(crate) fn logs(s: &Session, id: &str, params: &Value) -> bool {
    // Making or opening a project from text names no place: it is logged as the open from a file is.
    if matches!(
        id,
        "edit.undo"
            | "edit.redo"
            | "project.save"
            | "project.clearLog"
            | "project.logCommands"
            | "project.new"
            | "project.sample"
            | "project.openText"
    ) {
        return true;
    }
    let mutates = if id.starts_with(rustgis_plugins::PREFIX) {
        rustgis_plugins::command(id).is_some_and(|c| c.mutates())
    } else {
        match s.registry.get(id) {
            Some(spec) => spec.mutates,
            None => return false,
        }
    };
    mutates || crate::cmd::items::names_place(params, crate::cmd::plugins::PLACE_KEYS, 0)
}

/// `params` as the log keeps them: a document given as text (a map, a project, a capabilities
/// answer, GeoJSON) as its size only, since one may hold secrets no redaction finds in it (an S3
/// source's token inside a map's JSON); the rest clipped (so a long script costs little), then
/// without secrets (as History's Copy as code takes them out). A secret cut short is still taken
/// out: what is left of it follows its name. Within [`LOG_PARAMS_BYTES`] of JSON in three tries,
/// each giving up more: texts at that bound and lists at their first [`LOG_LIST_ITEMS`]; then
/// texts at 512 characters; then the first keys with their short values only.
pub(crate) fn kept_params(params: &Value) -> Value {
    let v = redacted(&clip(params, LOG_PARAMS_BYTES, LOG_LIST_ITEMS, 0));
    if serde_json::to_vec(&v).map_or(0, |b| b.len()) <= LOG_PARAMS_BYTES {
        return v;
    }
    // Texts shorter; lists as they are (already cut, their "… N items in all" kept).
    let clipped = clip(&v, 512, usize::MAX, 0);
    if serde_json::to_vec(&clipped).map_or(0, |b| b.len()) <= LOG_PARAMS_BYTES {
        return clipped;
    }
    // Still too large (many keys, long lists): its first keys with their short values only (a
    // number, a flag, a text of up to 64 characters with no brace), the rest as "…", as many as
    // fit within the bound. Nothing in it is cut JSON, so it reads back as it was written.
    let short = |v: &Value| match v {
        Value::String(t) if t.chars().nth(64).is_none() && !t.contains('{') => v.clone(),
        Value::Number(_) | Value::Bool(_) | Value::Null => v.clone(),
        _ => Value::String("…".into()),
    };
    match clipped {
        Value::Object(o) => {
            // Each key and value is short, but forty of them can still add up (escaped or
            // multi-byte text): each pair counts its bytes, a colon and a comma. Room is kept for
            // the note of how many keys there were, when some are left out.
            let mut room = LOG_PARAMS_BYTES.saturating_sub(2 + 48);
            let mut out = serde_json::Map::new();
            for (k, v) in o.iter().take(40) {
                let (k, v): (String, Value) = (k.chars().take(64).collect(), short(v));
                // Two keys alike in their first 64 characters: the first stays.
                if out.contains_key(&k) {
                    continue;
                }
                let size = serde_json::to_vec(&k)
                    .map_or(usize::MAX, |b| b.len())
                    .saturating_add(serde_json::to_vec(&v).map_or(usize::MAX, |b| b.len()))
                    .saturating_add(2);
                if size > room {
                    break;
                }
                room -= size;
                out.insert(k, v);
            }
            if out.len() < o.len() {
                out.insert("…".into(), Value::String(format!("{} keys in all", o.len())));
            }
            Value::Object(out)
        }
        other => short(&other),
    }
}

/// An entry read from a file, kept as a new one is: its command clipped, its parameters and error
/// clipped and without secrets (a file may hold what RustGIS never wrote, and a plugin may read
/// the log).
pub(crate) fn bounded(mut e: LogEntry) -> LogEntry {
    if e.command.chars().nth(200).is_some() {
        e.command = e.command.chars().take(200).collect();
    }
    // One that fits, as written, keeps its clipped lists' counts: only the document rule applies,
    // then its secrets are taken out.
    let fits = serde_json::to_vec(&e.params).is_ok_and(|b| b.len() <= LOG_PARAMS_BYTES);
    e.params = if fits { redacted(&clip(&e.params, usize::MAX, usize::MAX, 0)) } else { kept_params(&e.params) };
    e.error = e.error.as_deref().map(kept_error);
    // Pointers into the entry, as many as its texts at most.
    e.relative.truncate(1_000);
    e.relative.retain(|p| p.len() <= 1_024);
    e
}

/// An error as the log keeps it: clipped to [`LOG_ERROR_CHARS`] before its secrets are taken out,
/// so a long one costs little (a secret cut short still follows its name).
fn kept_error(e: &str) -> String {
    let e = if e.chars().nth(LOG_ERROR_CHARS).is_some() {
        format!("{}…", e.chars().take(LOG_ERROR_CHARS).collect::<String>())
    } else {
        e.to_string()
    };
    crate::cmd::history::redacted_text(&sql_secrets_removed(&e))
}

/// `v` without secrets: as History's Copy as code takes them out (`history::redacted`), and,
/// first, inside the JSON a text holds (a script's command lines: `s3.connect {"secret": …}`),
/// whose objects are redacted in place.
fn redacted(v: &Value) -> Value {
    crate::cmd::history::redacted(&embedded_redacted(v, 0))
}

fn embedded_redacted(v: &Value, depth: usize) -> Value {
    match v {
        Value::String(t) => {
            let t = if t.contains('{') { json_in_text_redacted(t) } else { t.clone() };
            Value::String(sql_secrets_removed(&t))
        }
        Value::Array(a) if depth < 8 => Value::Array(a.iter().map(|x| embedded_redacted(x, depth + 1)).collect()),
        Value::Object(o) if depth < 8 => Value::Object(o.iter().map(|(k, x)| (k.clone(), embedded_redacted(x, depth + 1))).collect()),
        other => other.clone(),
    }
}

/// `t` with each JSON object it holds (from a `{` to where the object ends) redacted. Texts are
/// clipped before this, so trying each `{` stays cheap; the secret names are looked for once.
pub(crate) fn json_in_text_redacted(t: &str) -> String {
    let mut out = String::with_capacity(t.len());
    let last_secret = last_secret_name(t);
    let mut rest = t;
    while let Some(at) = rest.find('{') {
        // Where this `{` is in `t`: a secret's name after it is one at a later offset.
        let brace = t.len() - rest.len() + at;
        let (before, from) = rest.split_at(at);
        out.push_str(before);
        let mut objects = serde_json::Deserializer::from_str(from).into_iter::<Value>();
        match objects.next() {
            Some(Ok(v @ Value::Object(_))) => {
                let end = objects.byte_offset();
                out.push_str(&serde_json::to_string(&crate::cmd::history::redacted(&v)).unwrap_or_default());
                rest = from.get(end..).unwrap_or("");
            }
            // Cut short by the clip, or an object that doesn't parse (a trailing comma, single
            // quotes) though a key starts it, or whose keys aren't quoted but one is a secret's
            // (`{secretAccessKey: …}`): what follows its `{` may hold a secret, so none of it
            // stays. Text such as `{name}` or `{x:.2f}` stays as it is.
            Some(Err(e))
                if e.is_eof()
                    || from.get(1..).is_some_and(|r| r.trim_start().starts_with(['"', '\'']))
                    || last_secret.is_some_and(|s| s > brace) =>
            {
                out.push('…');
                return out;
            }
            _ => {
                out.push('{');
                rest = from.get(1..).unwrap_or("");
            }
        }
    }
    out.push_str(rest);
    out
}

/// `t` with each SQL `CREATE … SECRET`'s options (`CREATE SECRET s3 (TYPE s3, KEY_ID 'a',
/// SECRET 'b')`, DuckDB's form, `OR REPLACE` and `PERSISTENT` too) as `(…)`: their values are
/// written as plain words, which no name or address form finds. With no `)`, the rest goes.
pub(crate) fn sql_secrets_removed(t: &str) -> String {
    // ASCII lowercase keeps every byte where it was.
    let lower = t.to_ascii_lowercase();
    let mut out = String::with_capacity(t.len());
    let mut from = 0;
    while let Some(at) = lower.get(from..).and_then(|r| r.find("create")).map(|i| i + from) {
        let after = at + "create".len();
        // `create` as a word of its own (not `recreate` or `created`).
        let word = lower.get(..at).and_then(|b| b.chars().next_back()).is_none_or(|c| !(c.is_alphanumeric() || c == '_'))
            && lower.get(after..).is_some_and(|r| r.starts_with(char::is_whitespace));
        if !word {
            out.push_str(t.get(from..after).unwrap_or(""));
            from = after;
            continue;
        }
        // The statement's head, to its `(`: `create [or replace] [persistent] secret [name] (`.
        let Some(open) = lower.get(after..).and_then(|r| r.find(['(', ';'])).map(|i| i + after) else { break };
        let head = lower.get(after..open).unwrap_or("");
        // The object's kind comes first (`SECRET`, after `OR REPLACE`, `PERSISTENT` or
        // `TEMPORARY`): a table named `secret` isn't one.
        let kind = head.split_whitespace().find(|w| !matches!(*w, "or" | "replace" | "persistent" | "temporary" | "temp"));
        if !lower.get(open..).is_some_and(|r| r.starts_with('(')) || kind != Some("secret") {
            out.push_str(t.get(from..after).unwrap_or(""));
            from = after;
            continue;
        }
        out.push_str(t.get(from..open).unwrap_or(""));
        out.push_str("(…");
        // Its `)`, quotes and nested brackets aside.
        let (mut depth, mut quote, mut close) = (0usize, None, None);
        for (i, c) in t.get(open..).unwrap_or("").char_indices() {
            match (quote, c) {
                (Some(q), c) if c == q => quote = None,
                (Some(_), _) => {}
                (None, '\'' | '"') => quote = Some(c),
                (None, '(') => depth += 1,
                (None, ')') => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        close = Some(open + i);
                        break;
                    }
                }
                _ => {}
            }
        }
        match close {
            Some(c) => {
                out.push(')');
                from = c + 1;
            }
            None => return out,
        }
    }
    out.push_str(t.get(from..).unwrap_or(""));
    out
}

/// Where in `t` the last secret's name (`credentials::secret_key`) written as a key, unquoted,
/// before a `:` or `=` (`secretAccessKey: "…"`, `token = …`) starts: one pass over the text.
fn last_secret_name(t: &str) -> Option<usize> {
    let name = |c: char| c.is_alphanumeric() || matches!(c, '_' | '-' | '.');
    let mut last = None;
    let mut rest = t;
    while let Some(at) = rest.find(name) {
        let from = rest.get(at..).unwrap_or("");
        let end = from.find(|c: char| !name(c)).unwrap_or(from.len());
        let (key, after) = from.split_at(end);
        if after.trim_start().starts_with([':', '=']) && rustgis_data::source::credentials::secret_key(key) {
            last = Some(t.len() - from.len());
        }
        rest = after;
    }
    last
}

/// The longest text kept as it is that reads as a document (it starts with `{`, `[` or `<`).
const DOCUMENT_CHARS: usize = 200;

/// `v` with each text cut to `chars` characters (a key to 200), a text that reads as a document
/// (JSON or XML past [`DOCUMENT_CHARS`]) given as its size, and each list cut to its first
/// `items` (and how many there were); nesting past eight levels is left out.
fn clip(v: &Value, chars: usize, items: usize, depth: usize) -> Value {
    match v {
        // A byte-order mark before it is still a document's start.
        Value::String(t)
            if t.trim_start_matches('\u{feff}').trim_start().starts_with(['{', '[', '<']) && t.chars().nth(DOCUMENT_CHARS).is_some() =>
        {
            Value::String(format!("(a document of {} characters)", t.chars().count()))
        }
        Value::String(t) if t.chars().nth(chars).is_some() => Value::String(format!("{}…", t.chars().take(chars).collect::<String>())),
        Value::Array(a) if depth < 8 => {
            let mut out: Vec<Value> = a.iter().take(items).map(|x| clip(x, chars, items, depth + 1)).collect();
            if a.len() > items {
                out.push(Value::String(format!("… {} items in all", a.len())));
            }
            Value::Array(out)
        }
        Value::Object(o) if depth < 8 => {
            // Two keys alike in their first 200 characters: the first stays. A key cut short may
            // hide a secret's name past the cut (redaction reads the names after this): its value
            // goes.
            let mut out = serde_json::Map::new();
            for (k, x) in o {
                let value = || if k.chars().nth(200).is_some() { Value::String("…".into()) } else { clip(x, chars, items, depth + 1) };
                out.entry(k.chars().take(200).collect::<String>()).or_insert_with(value);
            }
            Value::Object(out)
        }
        Value::Array(_) | Value::Object(_) => Value::String("…".into()),
        other => other.clone(),
    }
}

/// The entry of command `id`, which started at `at` and took `took`; its error without secrets,
/// clipped to [`LOG_ERROR_CHARS`].
pub(crate) fn entry(id: &str, params: Value, res: &CmdResult, at: u64, took: web_time::Duration) -> LogEntry {
    let error = res.as_ref().err().map(|e| kept_error(e));
    LogEntry {
        at,
        command: id.chars().take(200).collect(),
        params,
        ok: res.is_ok(),
        error,
        millis: u64::try_from(took.as_millis()).unwrap_or(u64::MAX),
        relative: Vec::new(),
    }
}

/// Most entries a project's log keeps; past it, the oldest go first.
pub const LOG_LIMIT: usize = 10_000;
/// The most entries of plugins' commands the log keeps, their oldest going first past it: a
/// plugin run many times can't push the user's own entries out of the log.
pub const LOG_PLUGIN_LIMIT: usize = 1_000;
/// Most bytes of JSON an entry's parameters keep (a long text is cut, a long list keeps its first
/// [`LOG_LIST_ITEMS`]), and most characters of its error.
pub const LOG_PARAMS_BYTES: usize = 4096;
pub const LOG_LIST_ITEMS: usize = 100;
pub const LOG_ERROR_CHARS: usize = 500;

/// One command run on a project: when it started (UTC, milliseconds since 1970), its id, its
/// parameters (secrets taken out, large values clipped), whether it succeeded, its error, and how
/// long it took.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LogEntry {
    pub at: u64,
    pub command: String,
    #[serde(default)]
    pub params: serde_json::Value,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub millis: u64,
    /// In a saved file, what saving made portable: the parameters' texts it changed, by their
    /// JSON pointers (`/params/path`, always starting with `/`), and each relative path it wrote
    /// (`./My Data/a.tif`, always starting with `.`). Reading makes those very paths absolute
    /// again, in those texts and in the error, and nothing else: a text or a word given as
    /// relative (`./a.tif`, a name such as `./Maps`) reads back as it was run. Taken once read;
    /// never shown.
    #[serde(default, skip_serializing)]
    pub relative: Vec<String>,
}

/// The longest saved log text kept for the next save ([`LogState::written`]).
const WRITTEN_KEPT_BYTES: usize = 1 << 20;

/// A log as it is held, written by [`ProjectLog::write_json`] (its paths made portable) and read
/// back through its own `Deserialize`.
#[derive(Debug, Default)]
struct LogState {
    /// Logging turned off (`project.logCommands`); on by default.
    off: bool,
    /// Oldest first; the oldest go first past [`LOG_LIMIT`], without moving the rest.
    entries: std::collections::VecDeque<LogEntry>,
    /// Moved on by each change, so a text written earlier is known to be the log as it is.
    generation: u64,
    /// The text the last save wrote, with the generation it was written from and the folder it
    /// was written for: a save or autosave with nothing logged since writes it as it is. Up to
    /// [`WRITTEN_KEPT_BYTES`] only.
    written: Option<(u64, Option<String>, std::sync::Arc<str>)>,
}

/// Values a file's log can hold that aren't what it should be: read as nothing (an empty log, no
/// entries), so that nothing in the log keeps a project from opening.
macro_rules! anything_else_is_default {
    () => {
        fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<Self::Value, E> {
            Ok(Default::default())
        }
        fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<Self::Value, E> {
            Ok(Default::default())
        }
        fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<Self::Value, E> {
            Ok(Default::default())
        }
        fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<Self::Value, E> {
            Ok(Default::default())
        }
        fn visit_str<E: serde::de::Error>(self, _: &str) -> Result<Self::Value, E> {
            Ok(Default::default())
        }
        fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
            Ok(Default::default())
        }
        fn visit_none<E: serde::de::Error>(self) -> Result<Self::Value, E> {
            Ok(Default::default())
        }
    };
}

impl<'de> Deserialize<'de> for LogState {
    /// A file's log (a project file is hostile input), read so that nothing in it keeps the
    /// project from opening: its entries one at a time ([`LogEntries`]); anything but an object
    /// is an empty log.
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct State;
        impl<'de> serde::de::Visitor<'de> for State {
            type Value = LogState;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a project log")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<LogState, A::Error> {
                let mut s = LogState::default();
                while let Some(k) = map.next_key::<String>()? {
                    match k.as_str() {
                        "off" => s.off = map.next_value::<serde_json::Value>()?.as_bool().unwrap_or(false),
                        "entries" => s.entries = map.next_value_seed(LogEntries)?,
                        _ => {
                            map.next_value::<serde::de::IgnoredAny>()?;
                        }
                    }
                }
                Ok(s)
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<LogState, A::Error> {
                while seq.next_element::<serde::de::IgnoredAny>()?.is_some() {}
                Ok(LogState::default())
            }
            anything_else_is_default!();
        }
        d.deserialize_any(State)
    }
}

/// A file's log entries, each read in turn as a JSON value (the file is in memory already),
/// bounded and without secrets as a new entry is ([`bounded`]) as it comes, and only the last
/// [`LOG_LIMIT`] kept. One that doesn't read as an entry (a file edited by hand, another
/// version's) is skipped.
struct LogEntries;

impl<'de> serde::de::DeserializeSeed<'de> for LogEntries {
    type Value = std::collections::VecDeque<LogEntry>;
    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        d.deserialize_any(self)
    }
}

impl<'de> serde::de::Visitor<'de> for LogEntries {
    type Value = std::collections::VecDeque<LogEntry>;
    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("a list of log entries")
    }
    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut out = std::collections::VecDeque::new();
        while let Some(v) = seq.next_element::<serde_json::Value>()? {
            let Ok(e) = serde_json::from_value::<LogEntry>(v) else { continue };
            // Bounded at once, so what is held is at most the cap's worth of bounded entries,
            // whatever the file holds.
            out.push_back(bounded(e));
            if out.len() > LOG_LIMIT {
                out.pop_front();
            }
        }
        Ok(out)
    }
    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        while map.next_entry::<serde::de::IgnoredAny, serde::de::IgnoredAny>()?.is_some() {}
        Ok(Default::default())
    }
    anything_else_is_default!();
}

/// A project's log (#797): one entry for each command run on it, from any front end, kept in
/// the `.rgis`. It sits outside the undo history: the project's undo snapshots share it (a clone
/// is the same log), so undoing a command never takes its entry back, and a snapshot copies
/// nothing of it. A project opened, made new or from a template has a log of its own. It is read
/// in place, under its lock; only saving copies its entries, once ([`ProjectLog::write_json`]).
#[derive(Clone, Debug, Default)]
pub struct ProjectLog(std::sync::Arc<Mutex<LogState>>);

impl ProjectLog {
    fn state(&self) -> MutexGuard<'_, LogState> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The state, to change: what a save wrote of it is no longer the log.
    fn changing(&self) -> MutexGuard<'_, LogState> {
        let mut s = self.state();
        s.generation = s.generation.wrapping_add(1);
        s.written = None;
        s
    }

    /// No entries, and logging on: nothing for a project file to keep.
    pub fn nothing_to_save(&self) -> bool {
        let s = self.state();
        s.entries.is_empty() && !s.off
    }

    pub fn on(&self) -> bool {
        !self.state().off
    }

    pub fn set_on(&self, on: bool) {
        self.changing().off = !on;
    }

    /// Adds `e` when logging is on, the oldest entries dropped past [`LOG_LIMIT`], and a plugin's
    /// command's past [`LOG_PLUGIN_LIMIT`] the oldest of those; turning the log off
    /// (`project.logCommands`), and clearing it while off, are recorded too, so the log says why
    /// it stops or is empty.
    pub fn push(&self, e: LogEntry) {
        let mut s = self.changing();
        if s.off && !matches!(e.command.as_str(), "project.logCommands" | "project.clearLog") {
            return;
        }
        let plugin = |e: &LogEntry| e.command.starts_with(rustgis_plugins::PREFIX);
        if plugin(&e)
            && s.entries.iter().filter(|x| plugin(x)).count() >= LOG_PLUGIN_LIMIT
            && let Some(oldest) = s.entries.iter().position(plugin)
        {
            s.entries.remove(oldest);
        }
        s.entries.push_back(e);
        while s.entries.len() > LOG_LIMIT {
            s.entries.pop_front();
        }
    }

    /// Adds `e` as part of the last entry when that is the same command, run well: a step merged
    /// into the last undo step ([`crate::Session::merge_undo`]). The entry keeps when the first
    /// started, takes the last one's parameters, and adds up how long they took.
    pub fn join_last(&self, mut e: LogEntry) {
        {
            let mut s = self.changing();
            if let Some(last) = s.entries.back_mut().filter(|l| l.ok && l.command == e.command) {
                e.at = last.at;
                e.millis = last.millis.saturating_add(e.millis);
                *last = e;
                return;
            }
        }
        self.push(e);
    }

    /// Every entry, oldest first (a copy: for tests and small logs; [`ProjectLog::select`] and
    /// [`ProjectLog::each`] read in place).
    pub fn entries(&self) -> Vec<LogEntry> {
        self.state().entries.iter().cloned().collect()
    }

    /// The last `limit` entries that `keep` keeps, oldest first, with how many it keeps and how
    /// many there are: only those are copied.
    pub fn select(&self, keep: impl Fn(&LogEntry) -> bool, limit: usize) -> (Vec<LogEntry>, usize, usize) {
        let s = self.state();
        let matching: Vec<&LogEntry> = s.entries.iter().filter(|e| keep(e)).collect();
        let tail = matching.get(matching.len().saturating_sub(limit)..).unwrap_or_default().iter().map(|e| (*e).clone()).collect();
        (tail, matching.len(), s.entries.len())
    }

    /// Calls `f` with each entry, oldest first, under the log's lock.
    pub fn each(&self, mut f: impl FnMut(&LogEntry)) {
        for e in &self.state().entries {
            f(e);
        }
    }

    pub fn len(&self) -> usize {
        self.state().entries.len()
    }

    /// No entries (logging may still be off: see [`ProjectLog::nothing_to_save`]).
    pub fn is_empty(&self) -> bool {
        self.state().entries.is_empty()
    }

    /// The paths a file's log was saved with relative to its folder (`./data/a.tif`), made
    /// absolute again from `dir`, the folder it was read from, as the project's sources are; so
    /// a later Save As elsewhere writes them relative to that folder. Only the texts the save
    /// made portable (an entry's `relative`) are read as such: one given as relative stays so.
    pub(crate) fn resolve_paths(&self, dir: &str) {
        // The folder itself is written as `.`.
        let word = |p: &str| {
            if p == "." {
                return Some(dir.to_string());
            }
            (p.starts_with("./") || p.starts_with("../")).then(|| rustgis_map::paths::resolve(dir, p)).flatten()
        };
        for e in &mut self.changing().entries {
            let (pointers, wrote): (Vec<String>, Vec<String>) =
                std::mem::take(&mut e.relative).into_iter().partition(|r| r.starts_with('/'));
            // The paths the save wrote, each back to absolute, found whole (spaces and all).
            let swaps: Vec<(String, String)> = wrote.into_iter().filter_map(|w| word(&w).map(|abs| (w, abs))).collect();
            if swaps.is_empty() {
                continue;
            }
            for at in &pointers {
                if let Some(serde_json::Value::String(t)) = at.strip_prefix("/params").and_then(|p| e.params.pointer_mut(p)) {
                    *t = swapped(t, &swaps);
                }
            }
            if let Some(err) = e.error.as_mut() {
                *err = swapped(err, &swaps);
            }
        }
    }

    /// Empties the log; how many entries it had.
    pub fn clear(&self) -> usize {
        std::mem::take(&mut self.changing().entries).len()
    }

    /// The log as a project file keeps it, written at the end of `out`, each entry in turn; saved
    /// into a folder (`dir`), its local paths made portable ([`make_portable`]).
    fn write_json(&self, out: &mut String, dir: Option<&str>) -> Result<(), String> {
        // Nothing logged since the last save into the same folder: its text as it was.
        let written = {
            let s = self.state();
            s.written.as_ref().filter(|(g, d, _)| *g == s.generation && d.as_deref() == dir).map(|(_, _, t)| t.clone())
        };
        if let Some(text) = written {
            out.push_str(&text);
            return Ok(());
        }
        // Copied under the lock and written after it is let go, so a command logged meanwhile
        // (the interface thread, while an autosave writes) doesn't wait on the writing. That copy
        // is the only one: each entry is made portable in place and written into `out`.
        let (off, entries, generation) = {
            let s = self.state();
            (s.off, s.entries.clone(), s.generation)
        };
        let start = out.len();
        out.push_str(if off { r#"{"off":true,"entries":["# } else { r#"{"entries":["# });
        for (i, mut e) in entries.into_iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            // Where its paths were made portable: those read back as such (an entry read from a
            // file that wasn't resolved keeps its own).
            let mut relative = std::mem::take(&mut e.relative);
            if let Some(d) = dir {
                let kept = relative.len();
                let (params, error) = (e.params.clone(), e.error.clone());
                portable_entry(&mut e, d, &mut relative, false);
                // A text that already held, as a word, a relative path this save wrote would read
                // back with that word made absolute too: the entry's paths are written as their
                // file names instead, which read back as they are.
                if collides(&params, error.as_deref(), relative.get(kept..).unwrap_or_default()) {
                    (e.params, e.error) = (params, error);
                    relative.truncate(kept);
                    portable_entry(&mut e, d, &mut relative, true);
                }
            }
            let text = serde_json::to_string(&e).map_err(|e| e.to_string())?;
            match text.strip_suffix('}').filter(|_| !relative.is_empty()) {
                Some(open) => {
                    out.push_str(open);
                    out.push_str(",\"relative\":");
                    out.push_str(&serde_json::to_string(&relative).map_err(|e| e.to_string())?);
                    out.push('}');
                }
                None => out.push_str(&text),
            }
        }
        out.push_str("]}");
        // Kept for the next save, unless something was logged while this one wrote, or the text
        // is long enough (a log near its limit runs to tens of MB) that holding a second copy of
        // it until the next change costs more than writing it again.
        let mut s = self.state();
        if s.generation == generation && out.len() - start <= WRITTEN_KEPT_BYTES {
            s.written = Some((generation, dir.map(str::to_string), out.get(start..).unwrap_or("").into()));
        }
        Ok(())
    }
}

impl<'de> Deserialize<'de> for ProjectLog {
    /// A file's log, its last [`LOG_LIMIT`] entries ([`LogEntries`]).
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(ProjectLog(std::sync::Arc::new(Mutex::new(LogState::deserialize(d)?))))
    }
}

/// A project's saved JSON with its log (#797) as its last member, when there is one to keep. Saved
/// into a folder (`dir`), the local paths in the log's parameters and errors are written relative
/// to it, as the project's own sources are, or as their file names where they can't be, so a
/// project moved with its data keeps no trace of the folder it was in.
pub(crate) fn with_log(mut text: String, log: &ProjectLog, dir: Option<&str>) -> Result<String, String> {
    if log.nothing_to_save() || !text.ends_with('}') {
        return Ok(text);
    }
    text.pop();
    text.push_str(",\"log\":");
    log.write_json(&mut text, dir)?;
    text.push('}');
    Ok(text)
}

/// `e`'s parameters and error with their local paths made portable, in place, each change noted
/// in `relative` ([`make_portable`]); as file names only with `names`.
fn portable_entry(e: &mut LogEntry, dir: &str, relative: &mut Vec<String>, names: bool) {
    let mut swaps = Vec::new();
    make_portable(&mut e.params, dir, "/params", 0, names, relative, &mut swaps);
    if let Some(err) = e.error.as_mut() {
        // The parameters' paths first, whole (a folder may have spaces in its name), then any
        // other word that is one.
        *err = portable_text(&swapped(err, &swaps), dir, names, relative);
    }
}

/// Whether reading back what a save noted (`noted`: the pointers of the parameters it changed
/// and the relative paths it wrote) would change more than it wrote: one of those paths was
/// already a word of a text read back as such (a changed parameter's, as `params` held it, or
/// the error).
fn collides(params: &serde_json::Value, error: Option<&str>, noted: &[String]) -> bool {
    let (pointers, wrote): (Vec<&String>, Vec<&String>) = noted.iter().partition(|r| r.starts_with('/'));
    let texts = pointers.iter().filter_map(|p| p.strip_prefix("/params").and_then(|p| params.pointer(p))?.as_str()).chain(error);
    // `swapped` changes a text only where the path stands as a word of it.
    texts.into_iter().any(|t| wrote.iter().any(|w| swapped(t, &[((*w).clone(), String::new())]) != t))
}

/// `v`'s texts with their local paths made portable ([`portable_text`]; as file names only with
/// `names`), in place; each one changed noted in `changed` by its JSON pointer, `at` being `v`'s,
/// and each whole-text path in `swaps`, as it was and as it is written.
fn make_portable(
    v: &mut serde_json::Value,
    dir: &str,
    at: &str,
    depth: usize,
    names: bool,
    changed: &mut Vec<String>,
    swaps: &mut Vec<(String, String)>,
) {
    use serde_json::Value;
    match v {
        Value::String(t) => {
            let portable = portable_text(t, dir, names, changed);
            if portable != *t {
                if one_path(t) {
                    swaps.push((t.clone(), portable.clone()));
                }
                *t = portable;
                changed.push(at.to_string());
            }
        }
        Value::Array(a) if depth < 16 => {
            for (i, x) in a.iter_mut().enumerate() {
                make_portable(x, dir, &format!("{at}/{i}"), depth + 1, names, changed, swaps);
            }
        }
        Value::Object(o) if depth < 16 => {
            for (k, x) in o.iter_mut() {
                make_portable(x, dir, &format!("{at}/{}", k.replace('~', "~0").replace('/', "~1")), depth + 1, names, changed, swaps);
            }
        }
        _ => {}
    }
}

/// `t` with each text of `swaps` that is a word of its own (from the text's start or a space,
/// quote or bracket, to its end or a space, quote, bracket or punctuation) as its other half,
/// the longest first: a path with spaces in it, known whole.
fn swapped(t: &str, swaps: &[(String, String)]) -> String {
    let mut swaps: Vec<&(String, String)> = swaps.iter().filter(|(from, _)| !from.is_empty()).collect();
    swaps.sort_by_key(|(from, _)| std::cmp::Reverse(from.len()));
    let mut t = t.to_string();
    for (from, to) in swaps {
        let mut out = String::with_capacity(t.len());
        let mut rest = t.as_str();
        while let Some(at) = rest.find(from.as_str()) {
            out.push_str(rest.get(..at).unwrap_or(""));
            let starts = out.chars().next_back().is_none_or(|c| c.is_whitespace() || matches!(c, '"' | '\'' | '`' | '(' | '['));
            rest = rest.get(at + from.len()..).unwrap_or("");
            let ends = rest.chars().next().is_none_or(|c| c.is_whitespace() || matches!(c, '"' | '\'' | '`' | ')' | ']' | ',' | ';' | ':'));
            out.push_str(if starts && ends { to } else { from });
        }
        out.push_str(rest);
        t = out;
    }
    t
}

/// `t` with each absolute local path in it (the whole text, or a word of it, quotes and trailing
/// punctuation aside) relative to `dir`, or its file name where it can't be or with `names`;
/// each relative path written noted in `wrote` (once), for reading to make absolute again.
fn portable_text(t: &str, dir: &str, names: bool, wrote: &mut Vec<String>) -> String {
    let made = std::cell::RefCell::new(Vec::new());
    let portable = |p: &str| {
        let name = || p.rsplit(['/', '\\']).next().unwrap_or(p).to_string();
        if names {
            return name();
        }
        let r = rustgis_map::paths::relative_to(dir, p).filter(|r| rustgis_map::paths::is_local(r)).unwrap_or_else(name);
        made.borrow_mut().push(r.clone());
        r
    };
    let out = if one_path(t) { portable(t) } else { path_words(t, |p| rustgis_map::paths::is_absolute_local(p).then(|| portable(p))) };
    for r in made.into_inner() {
        if (r == "." || r.starts_with("./") || r.starts_with("../")) && !wrote.contains(&r) {
            wrote.push(r);
        }
    }
    out
}

/// Whether `t` is one absolute local path as a whole (spaces in its folders' names and all), not
/// a text that starts with one: no other word of it is a path, and no `: ` follows the first
/// (`/data/a.tif: not found`). The root alone (`/`) is no file's path.
fn one_path(t: &str) -> bool {
    let path_word = |w: &str| {
        let core = word_core(w);
        core.get(1..).is_some_and(|r| r.contains(['/', '\\'])) && rustgis_map::paths::is_absolute_local(core)
    };
    t.len() > 1
        && rustgis_map::paths::is_absolute_local(t)
        && (!t.contains(char::is_whitespace) || (!t.contains(": ") && !t.split_whitespace().skip(1).any(path_word)))
}

/// A word without the quotes, brackets and trailing `,`, `;` or `:` around it.
fn word_core(word: &str) -> &str {
    let core = word.trim_end().trim_matches(|c: char| matches!(c, '"' | '\'' | '`' | '(' | ')' | '[' | ']' | ',' | ';'));
    core.strip_suffix(':').unwrap_or(core)
}

/// `t` with each word (quotes, brackets, a trailing `,`, `;` or `:` aside) replaced by what `f`
/// makes of it, where `f` gives something: how a path among a text's words is found when it is
/// made portable on saving. Only a word with a separator past its first character is tried (`/data/a.tif`, `C:\a.tif`, `./a.tif`): a division's `/` or a
/// `/home` among other words is left as written.
fn path_words(t: &str, f: impl Fn(&str) -> Option<String>) -> String {
    t.split_inclusive(char::is_whitespace)
        .map(|word| {
            let core = word_core(word);
            let path_like = core.get(1..).is_some_and(|r| r.contains(['/', '\\']));
            match path_like.then(|| f(core)).flatten() {
                Some(new) => word.replacen(core, &new, 1),
                None => word.to_string(),
            }
        })
        .collect()
}
