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
/// project (all but those built with `.view()`), a plugin's, undo and redo, a save, the log's own
/// clearing and switching, and a view command whose parameters name a file, folder, address or
/// connection (an export), as the plugin permission gate reads them. Reading and looking around
/// (listing, describing, moving the view) is left out: agents and scripts call those hundreds of
/// times.
pub(crate) fn logs(s: &Session, id: &str, params: &Value) -> bool {
    if id.starts_with(rustgis_plugins::PREFIX)
        || matches!(id, "edit.undo" | "edit.redo" | "project.save" | "project.clearLog" | "project.logCommands")
    {
        return true;
    }
    let Some(spec) = s.registry.get(id) else { return false };
    spec.mutates || crate::cmd::items::names_place(params, crate::cmd::plugins::PLACE_KEYS, 0)
}

/// `params` as the log keeps them: a document given as text (a map, a project, a capabilities
/// answer, GeoJSON) as its size only, since one may hold secrets no redaction finds in it (an S3
/// source's token inside a map's JSON); the rest clipped (so a long script costs little), then
/// without secrets (as History's Copy as code takes them out), to about [`LOG_PARAMS_BYTES`] of
/// JSON. A secret cut short is still taken out: what is left of it follows its name.
pub(crate) fn kept_params(params: &Value) -> Value {
    let v = crate::cmd::history::redacted(&clip(params, LOG_PARAMS_BYTES, 0));
    if serde_json::to_vec(&v).map_or(0, |b| b.len()) <= LOG_PARAMS_BYTES {
        return v;
    }
    let clipped = clip(&v, 512, 0);
    if serde_json::to_vec(&clipped).map_or(0, |b| b.len()) <= LOG_PARAMS_BYTES {
        return clipped;
    }
    // Still too large (many keys, deep nesting): its start, as text.
    let text = serde_json::to_string(&clipped).unwrap_or_default();
    Value::String(format!("{}…", text.chars().take(LOG_PARAMS_BYTES / 2).collect::<String>()))
}

/// The longest text kept as it is that reads as a document (it starts with `{`, `[` or `<`).
const DOCUMENT_CHARS: usize = 200;

/// `v` with each text cut to `chars` characters (a key to 200), a text that reads as a document
/// (JSON or XML past [`DOCUMENT_CHARS`]) given as its size, and each list cut to its first
/// [`LOG_LIST_ITEMS`] (and how many there were); nesting past eight levels is left out.
fn clip(v: &Value, chars: usize, depth: usize) -> Value {
    match v {
        Value::String(t) if t.trim_start().starts_with(['{', '[', '<']) && t.chars().nth(DOCUMENT_CHARS).is_some() => {
            Value::String(format!("(a document of {} characters)", t.chars().count()))
        }
        Value::String(t) if t.chars().count() > chars => Value::String(format!("{}…", t.chars().take(chars).collect::<String>())),
        Value::Array(a) if depth < 8 => {
            let mut out: Vec<Value> = a.iter().take(LOG_LIST_ITEMS).map(|x| clip(x, chars, depth + 1)).collect();
            if a.len() > LOG_LIST_ITEMS {
                out.push(Value::String(format!("… {} items in all", a.len())));
            }
            Value::Array(out)
        }
        Value::Object(o) if depth < 8 => {
            Value::Object(o.iter().map(|(k, x)| (k.chars().take(200).collect(), clip(x, chars, depth + 1))).collect())
        }
        Value::Array(_) | Value::Object(_) => Value::String("…".into()),
        other => other.clone(),
    }
}

/// The entry of command `id`, which started at `at` and took `took`; its error without secrets,
/// clipped to [`LOG_ERROR_CHARS`].
pub(crate) fn entry(id: &str, params: Value, res: &CmdResult, at: u64, took: std::time::Duration) -> LogEntry {
    // Clipped before its secrets are taken out, so a long error costs little; a secret cut short
    // still follows its name.
    let error = res.as_ref().err().map(|e| {
        let e = if e.chars().count() > LOG_ERROR_CHARS {
            format!("{}…", e.chars().take(LOG_ERROR_CHARS).collect::<String>())
        } else {
            e.clone()
        };
        crate::cmd::history::redacted_text(&e)
    });
    LogEntry {
        at,
        command: id.chars().take(200).collect(),
        params,
        ok: res.is_ok(),
        error,
        millis: u64::try_from(took.as_millis()).unwrap_or(u64::MAX),
    }
}
