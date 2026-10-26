//! The project log's entries (#797): which commands go in, and what of them is kept. See
//! [`crate::project::ProjectLog`].

use serde_json::Value;

use crate::project::{LOG_ERROR_CHARS, LOG_LIST_ITEMS, LOG_PARAMS_BYTES, LogEntry};
use crate::{CmdResult, Session};

/// Now, in milliseconds since 1970 (UTC).
pub(crate) fn now_ms() -> u64 {
    web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Whether command `id`, run by itself with `params`, goes into the log: one that changes the
/// project (all but those built with `.view()`; a plugin's when its manifest asks for the Edit
/// permission), undo and redo, a save,
/// the log's own clearing and switching, and a command whose parameters name a file, folder,
/// address or connection (an export), as the plugin permission gate reads them. Reading and
/// looking around (listing, describing, moving the view) is left out: agents, scripts and plugins
/// call those hundreds of times, and they would push the changes out of the log.
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
/// without secrets (as History's Copy as code takes them out), to about [`LOG_PARAMS_BYTES`] of
/// JSON. A secret cut short is still taken out: what is left of it follows its name.
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
    crate::cmd::history::redacted_text(&e)
}

/// `v` without secrets: as History's Copy as code takes them out (`history::redacted`), and,
/// first, inside the JSON a text holds (a script's command lines: `s3.connect {"secret": …}`),
/// whose objects are redacted in place.
fn redacted(v: &Value) -> Value {
    crate::cmd::history::redacted(&embedded_redacted(v, 0))
}

fn embedded_redacted(v: &Value, depth: usize) -> Value {
    match v {
        Value::String(t) if t.contains('{') => Value::String(json_in_text_redacted(t)),
        Value::Array(a) if depth < 8 => Value::Array(a.iter().map(|x| embedded_redacted(x, depth + 1)).collect()),
        Value::Object(o) if depth < 8 => Value::Object(o.iter().map(|(k, x)| (k.clone(), embedded_redacted(x, depth + 1))).collect()),
        other => other.clone(),
    }
}

/// `t` with each JSON object it holds (from a `{` to where the object ends) redacted. Texts are
/// clipped before this, so trying each `{` stays cheap; the secret names are looked for once.
fn json_in_text_redacted(t: &str) -> String {
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
            // Two keys alike in their first 200 characters: the first stays.
            let mut out = serde_json::Map::new();
            for (k, x) in o {
                out.entry(k.chars().take(200).collect::<String>()).or_insert_with(|| clip(x, chars, items, depth + 1));
            }
            Value::Object(out)
        }
        Value::Array(_) | Value::Object(_) => Value::String("…".into()),
        other => other.clone(),
    }
}

/// The entry of command `id`, which started at `at` and took `took`; its error without secrets,
/// clipped to [`LOG_ERROR_CHARS`].
pub(crate) fn entry(id: &str, params: Value, res: &CmdResult, at: u64, took: std::time::Duration) -> LogEntry {
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
