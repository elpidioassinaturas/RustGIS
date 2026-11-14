//! The JSON Schema of the project file (`.rgis`), generated from the types it is saved from
//! (#1853 piece 2). `cargo xtask schema` writes it to [`PATH`], and `cargo xtask schema --check`
//! fails when the committed file and the types disagree, so a change to what is saved updates both.
//!
//! The schema says what a reader accepts: a field the reader fills in when it is absent isn't
//! required. Parts not described yet ([`Open`]) are any JSON object; they are described in later
//! pull requests, crate by crate.

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};

use crate::project::{CAPABILITIES, FORMAT, Project, READ_FORMAT};

/// Where the schema is kept, from the repository's root.
pub const PATH: &str = "schema/project.schema.json";

/// A part of the project file this schema doesn't describe: any JSON object (described in later
/// pull requests of #1853 piece 2, crate by crate).
pub struct Open;

impl JsonSchema for Open {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Open".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "object",
            "description": "Any object: this schema doesn't describe its fields."
        })
    }
}

/// The schema as [`PATH`] holds it: JSON Schema 2020-12, with the formats it covers (the one this
/// RustGIS writes and the newest it reads) and the capabilities it knows, so changing any of them
/// changes the file too.
pub fn project() -> serde_json::Value {
    let mut schema = schemars::generate::SchemaSettings::draft2020_12().into_generator().into_root_schema_for::<Project>();
    schema.insert("$id".into(), format!("https://raw.githubusercontent.com/opengeos/RustGIS/main/{PATH}").into());
    schema.insert("title".into(), "RustGIS project (.rgis)".into());
    schema.insert(
        "description".into(),
        format!(
            "A RustGIS project file, as RustGIS reads it. Generated from the Rust types by `cargo xtask schema`; don't edit it by hand. It writes format {FORMAT} and reads formats up to {READ_FORMAT}."
        )
        .into(),
    );
    schema.insert("x-rustgis-writes-format".into(), FORMAT.into());
    schema.insert("x-rustgis-reads-format".into(), READ_FORMAT.into());
    schema.insert("x-rustgis-capabilities".into(), CAPABILITIES.into());
    schema.to_value()
}

/// [`project`] as the file holds it: pretty-printed, with a final newline.
pub fn project_text() -> Result<String, String> {
    let mut text = serde_json::to_string_pretty(&project()).map_err(|e| format!("the project schema: {e}"))?;
    text.push('\n');
    Ok(text)
}
