//! slides: one small tool for presentations.
//!
//! Create, read, edit and convert PPTX/PDF presentations from a small JSON request. The public
//! entry point is [`run`]; [`schema`] returns the JSON Schema of the request.

mod charts;
mod checks;
mod deck;
mod edited_text;
#[cfg(target_os = "linux")]
mod file_io;
mod markdown;
pub mod messages;
mod ooxml;
mod paragraph;
mod pdf;
mod pptx;
mod request_schema;
mod templates;
mod xmldom;
mod xmlw;

pub use deck::{Block, Chart, ChartKind, Deck, Layout, Series, Slide, Table, Theme};
pub use messages::{MessageCode, Msg, Warning};

use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Tool name.
pub const NAME: &str = "slides";
/// One-line description.
pub const DESCRIPTION: &str = messages::DESCRIPTION;

/// Default input size limit (100 MB).
pub const DEFAULT_MAX_INPUT_BYTES: u64 = 100 * 1024 * 1024;

/// Errors returned by [`run`]. [`Error::code`] and [`Error::params`] expose the stable
/// message-catalog identity of the error; `Display` renders it in English.
#[derive(Debug, thiserror::Error)]
#[error("{}", self.msg().render("en"))]
pub enum Error {
    AtField {
        #[source]
        source: Box<Error>,
        field: String,
    },
    Input(Msg),
    RequestJson {
        #[source]
        source: serde_json::Error,
        field: Option<String>,
        allowed: Vec<String>,
    },
    UnsafePath {
        path: String,
    },
    TooLarge {
        path: String,
        size: u64,
        limit: u64,
        field: String,
    },
    Read {
        path: String,
        source: std::io::Error,
    },
    Write {
        path: String,
        source: std::io::Error,
    },
    Zip(#[source] Box<dyn std::error::Error + Send + Sync>),
    PdfIo {
        source: std::io::Error,
    },
    FontRead {
        path: String,
        source: std::io::Error,
    },
    ChartData {
        source: std::num::ParseFloatError,
    },
    ModelNumber {
        part: String,
        source: std::num::ParseFloatError,
    },
    ModelIndex {
        part: String,
        source: std::num::ParseIntError,
    },
    CheckJson {
        source: serde_json::Error,
    },
    MarkdownUtf8 {
        source: std::string::FromUtf8Error,
    },
    FontNameUtf8 {
        source: std::string::FromUtf8Error,
    },
    Xml(Msg),
    XmlSource {
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    Pptx(Msg),
    Pdf(Msg),
    Unsupported(Msg),
}

impl Error {
    pub(crate) fn zip(source: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::Zip(Box::new(source))
    }

    fn msg(&self) -> Msg {
        let mut message = self.raw_msg();
        for (key, value) in &mut message.params {
            if key == "field" || key == "pointer" {
                *value = field_pointer(value);
            }
        }
        if !message.params.iter().any(|(key, _)| key == "field") {
            message.params.push(("field".into(), String::new()));
        }
        message
    }

    fn raw_msg(&self) -> Msg {
        match self {
            Error::AtField { source, field } => {
                let mut message = source.raw_msg();
                message.params.retain(|(key, _)| key != "field");
                message.params.push(("field".into(), field.clone()));
                message
            }
            Error::RequestJson {
                source,
                field,
                allowed,
            } => {
                let mut parameters = messages::params(&[
                    ("detail", &source.to_string()),
                    ("line", &source.line().to_string()),
                    ("column", &source.column().to_string()),
                ]);
                if let Some(field) = field {
                    parameters.push(("field".into(), field.clone()));
                    // The pointer alias is retained for existing clients.
                    let pointer = if field.is_empty() || field.starts_with('/') {
                        field.clone()
                    } else {
                        format!("/{}", field.replace('.', "/"))
                    };
                    parameters.push(("pointer".into(), pointer));
                }
                if !allowed.is_empty() {
                    parameters.push(("allowed".into(), allowed.join(",")));
                }
                Msg::new(MessageCode::CliInvalidJson, parameters)
            }
            Error::Input(m)
            | Error::Xml(m)
            | Error::Pptx(m)
            | Error::Pdf(m)
            | Error::Unsupported(m) => m.clone(),
            Error::UnsafePath { path } => Msg::new(
                MessageCode::UnsafePath,
                messages::params(&[("path", &format!("{path:?}"))]),
            ),
            Error::TooLarge {
                path,
                size,
                limit,
                field,
            } => {
                let mut parameters = messages::params(&[
                    ("path", &format!("{path:?}")),
                    ("size", &size.to_string()),
                    ("limit", &limit.to_string()),
                ]);
                if !field.is_empty() {
                    parameters.push(("field".into(), field.clone()));
                }
                Msg::new(MessageCode::TooLarge, parameters)
            }
            Error::Read { path, source } => Msg::new(
                MessageCode::FileRead,
                messages::params(&[
                    ("path", &format!("{path:?}")),
                    ("kind", &format!("{:?}", source.kind())),
                ]),
            ),
            Error::Write { path, source } => Msg::new(
                MessageCode::FileWrite,
                messages::params(&[
                    ("path", &format!("{path:?}")),
                    ("kind", &format!("{:?}", source.kind())),
                ]),
            ),
            Error::FontRead { path, source } => Msg::new(
                MessageCode::PdfFontRead,
                messages::params(&[("path", path), ("detail", &source.to_string())]),
            ),
            Error::XmlSource { source } => Msg::new(
                MessageCode::XmlInvalid,
                messages::params(&[("detail", &source.to_string())]),
            ),
            Error::ChartData { .. } => Msg::new(MessageCode::ChartInvalidData, Vec::new()),
            Error::CheckJson { .. } => Msg::new(
                MessageCode::PackageInvalid,
                messages::params(&[("part", "check findings")]),
            ),
            Error::ModelNumber { part, .. } | Error::ModelIndex { part, .. } => Msg::new(
                MessageCode::ModelInvalid,
                messages::params(&[("part", part)]),
            ),
            Error::MarkdownUtf8 { source } => Msg::new(
                MessageCode::MarkdownNotUtf8,
                messages::params(&[("detail", &source.to_string())]),
            ),
            Error::FontNameUtf8 { .. } => Msg::new(MessageCode::PdfFontParse, Vec::new()),
            Error::PdfIo { .. } => Msg::new(MessageCode::PdfIo, Vec::new()),
            Error::Zip(detail) => Msg::new(
                MessageCode::ZipInvalid,
                messages::params(&[("detail", &detail.to_string())]),
            ),
        }
    }

    /// The same flat response envelope as a successful operation.
    pub fn output(&self) -> Output {
        let mut result = output(None, None, self.msg(), None, None, Vec::new(), "en");
        result.ok = false;
        result
    }

    /// The stable message code of this error.
    pub fn code(&self) -> &'static str {
        self.msg().code.as_str()
    }

    /// The message parameters of this error, as a JSON object.
    pub fn params(&self) -> serde_json::Value {
        messages::params_json(&self.msg().params)
    }
}

impl Serialize for Error {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.output().serialize(serializer)
    }
}

/// Parse a request, retaining JSON locations and schema paths for missing, unknown or mistyped fields.
/// Both CLI and library consumers can serialize the resulting Error as an Output envelope.
pub fn parse_input(json: &str) -> Result<Input, Error> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|source| Error::RequestJson {
            source,
            field: None,
            allowed: Vec::new(),
        })?;
    serde_json::from_value(value.clone()).map_err(|source| {
        let schema = parser_schema();
        let field = missing_field(&value, &schema, &schema, "");
        let allowed = if field.as_deref() == Some("/op") {
            vec!["create", "read", "edit", "convert", "check"]
                .into_iter()
                .map(str::to_owned)
                .collect()
        } else if field.as_deref() == Some("/representation") {
            vec!["text", "data", "both"]
                .into_iter()
                .map(str::to_owned)
                .collect()
        } else {
            Vec::new()
        };
        Error::RequestJson {
            source,
            field,
            allowed,
        }
    })
}

fn missing_field(
    value: &serde_json::Value,
    node: &serde_json::Value,
    root: &serde_json::Value,
    path: &str,
) -> Option<String> {
    if let Some(reference) = node["$ref"].as_str() {
        return missing_field(
            value,
            root.pointer(reference.strip_prefix('#')?)?,
            root,
            path,
        );
    }
    let child_path = |name: &str| format!("{path}/{}", name.replace('~', "~0").replace('/', "~1"));
    let outside_bounds = value.as_f64().is_some_and(|number| {
        node["minimum"].as_f64().is_some_and(|min| number < min)
            || node["maximum"].as_f64().is_some_and(|max| number > max)
    });
    if !schema_type_matches(value, node, root)
        || node.get("const").is_some_and(|v| v != value)
        || node["enum"]
            .as_array()
            .is_some_and(|values| !values.contains(value))
        || outside_bounds
    {
        return Some(path.into());
    }
    if let Some(required) = node["required"].as_array()
        && let Some(object) = value.as_object()
    {
        for name in required.iter().filter_map(serde_json::Value::as_str) {
            if !object.contains_key(name) {
                // Preserve the established missing-field contract; pointer is supplied too.
                return Some(child_path(name).trim_start_matches('/').replace('/', "."));
            }
        }
    }
    if let Some(properties) = node["properties"].as_object() {
        if node["additionalProperties"] == false {
            for name in value.as_object().into_iter().flat_map(|o| o.keys()) {
                if !properties.contains_key(name) {
                    return Some(child_path(name));
                }
            }
        }
        for (name, schema) in properties {
            if let Some(child) = value.get(name)
                && let Some(field) = missing_field(child, schema, root, &child_path(name))
            {
                return Some(field);
            }
        }
    }
    if let Some(array) = value.as_array() {
        for (index, child) in array.iter().enumerate() {
            if let Some(field) =
                missing_field(child, &node["items"], root, &child_path(&index.to_string()))
            {
                return Some(field);
            }
        }
    }
    for alternatives in ["oneOf", "anyOf"] {
        if let Some(variants) = node[alternatives].as_array() {
            let candidates: Vec<_> = variants
                .iter()
                .filter(|variant| {
                    schema_type_matches(value, variant, root)
                        && variant
                            .pointer("/properties/op/const")
                            .is_none_or(|op| value.get("op") == Some(op))
                })
                .collect();
            // Prefer the source branch whose required fields are actually present. This
            // prevents a slides error from being attributed to the unused markdown branch.
            if candidates.is_empty() {
                return Some(
                    if value.is_object()
                        && variants
                            .iter()
                            .any(|v| v.pointer("/properties/op/const").is_some())
                    {
                        child_path("op")
                    } else {
                        path.into()
                    },
                );
            }
            let required_present = |variant: &&serde_json::Value| {
                variant["required"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(serde_json::Value::as_str)
                    .all(|name| value.get(name).is_some())
            };
            let selected: Vec<_> = candidates
                .iter()
                .copied()
                .filter(required_present)
                .collect();
            let selected = if selected.is_empty() {
                candidates
            } else {
                selected
            };
            let mut first_error = None;
            for variant in selected {
                match missing_field(value, variant, root, path) {
                    None => {
                        first_error = None;
                        break;
                    }
                    Some(field) => {
                        if first_error.is_none() {
                            first_error = Some(field);
                        }
                    }
                }
            }
            if first_error.is_some() {
                return first_error;
            }
        }
    }
    None
}
fn schema_type_matches(
    value: &serde_json::Value,
    node: &serde_json::Value,
    root: &serde_json::Value,
) -> bool {
    if let Some(reference) = node["$ref"].as_str() {
        return reference
            .strip_prefix('#')
            .and_then(|r| root.pointer(r))
            .is_none_or(|node| schema_type_matches(value, node, root));
    }
    let matches = |kind: &str| match kind {
        "null" => value.is_null(),
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "number" => value.is_number(),
        "integer" => value.is_i64() || value.is_u64(),
        _ => true,
    };
    match &node["type"] {
        serde_json::Value::String(kind) => matches(kind),
        serde_json::Value::Array(kinds) => kinds
            .iter()
            .filter_map(serde_json::Value::as_str)
            .any(matches),
        _ => true,
    }
}

fn field_pointer(field: &str) -> String {
    if field.is_empty() || field.starts_with('/') {
        return field.to_owned();
    }
    let path = field.replace('[', ".").replace(']', "");
    format!(
        "/{}",
        path.split('.')
            .map(|part| part.replace('~', "~0").replace('/', "~1"))
            .collect::<Vec<_>>()
            .join("/")
    )
}

/// Runtime options. Every path in the request is relative to `base_dir`.
#[derive(Debug, Clone)]
pub struct Options {
    pub base_dir: PathBuf,
    pub max_input_bytes: u64,
    pub font_path: Option<PathBuf>,
    /// Deadline for each local rendering command.
    pub render_timeout: std::time::Duration,
    /// Message locale; only `en` is built in, unknown locales fall back to it.
    pub locale: String,
}

impl Options {
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Options {
            base_dir: base_dir.into(),
            max_input_bytes: DEFAULT_MAX_INPUT_BYTES,
            font_path: None,
            render_timeout: std::time::Duration::from_secs(120),
            locale: "en".to_string(),
        }
    }
}

impl Default for Options {
    fn default() -> Self {
        Options::new(".")
    }
}

/// Result returned by [`run`].
#[derive(Debug, Serialize)]
pub struct Output {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
    pub summary: String,
    pub summary_code: &'static str,
    pub summary_params: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    pub warnings: Vec<Warning>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncation: Option<serde_json::Value>,
}

fn output(
    output: Option<String>,
    bytes: Option<u64>,
    summary: Msg,
    text: Option<String>,
    data: Option<serde_json::Value>,
    warnings: Vec<Warning>,
    locale: &str,
) -> Output {
    Output {
        ok: true,
        output,
        bytes,
        summary: summary.render(locale),
        summary_code: summary.code.as_str(),
        summary_params: messages::params_json(&summary.params),
        text,
        data,
        warnings,
        truncation: None,
    }
}

/// A request. `op` selects the operation.
#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Input {
    /// Create a presentation from Markdown or structured slides.
    Create(CreateInput),
    /// Extract presentation content and optional template layouts.
    Read(ReadInput),
    /// Edit an existing presentation without rebuilding untouched parts.
    Edit(EditInput),
    /// Convert a presentation or Markdown document to another format.
    Convert(ConvertInput),
    /// Check explicitly labeled numeric facts and optional model references.
    Check(CheckInput),
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateInput {
    /// Output presentation path relative to the working directory.
    pub output: String,
    #[serde(default)]
    /// Markdown source; provide exactly one of markdown and slides.
    pub markdown: Option<String>,
    #[serde(default)]
    /// Structured slide source; provide exactly one of slides and markdown.
    pub slides: Option<Vec<SlideInput>>,
    #[serde(default)]
    /// Color and font theme: light, dark, corporate or creative.
    pub theme: Option<String>,
    /// Company PPTX or POTX template path.
    #[serde(default)]
    pub template: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SlideInput {
    /// Disable the automatic themed panel on slides with one to three bullets.
    #[serde(default)]
    pub short_slide_accent: Option<bool>,
    /// Standard layout type or company layout name when using a template.
    #[serde(default)]
    pub layout: Option<String>,
    /// Slide heading, preserved as supplied.
    #[serde(default)]
    pub title: Option<String>,
    /// Subtitle for a title or section slide.
    #[serde(default)]
    pub subtitle: Option<String>,
    /// Bullet text; two leading spaces per nested level.
    #[serde(default)]
    pub bullets: Option<Vec<String>>,
    /// Body text or image caption.
    #[serde(default)]
    pub text: Option<String>,
    /// A literal code block, including blank lines. Also available to edit.add.
    #[serde(default)]
    pub code: Option<String>,
    /// PNG or JPEG image path relative to the working directory.
    #[serde(default)]
    pub image: Option<String>,
    /// Native editable table with literal cell text.
    #[serde(default)]
    pub table: Option<TableInput>,
    /// Native editable chart and its source data.
    #[serde(default)]
    pub chart: Option<ChartInput>,
    /// Speaker notes, not visible slide content.
    #[serde(default)]
    pub notes: Option<String>,
    /// Visible source citations rendered in the footer.
    #[serde(default)]
    pub sources: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TableInput {
    /// Column headings, repeated on continuation slides.
    pub headers: Vec<String>,
    /// Rows of literal cell text in column order.
    pub rows: Vec<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChartInput {
    /// Chart type: bar, column, line, pie, area or scatter.
    #[serde(rename = "type")]
    pub kind: String,
    /// Category labels, or finite numeric X coordinates for scatter charts.
    pub labels: Vec<String>,
    /// Named data series; each value list must match the labels.
    pub series: Vec<SeriesInput>,
}

/// A chart series: either a named object or a bare array of numbers.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(untagged, deny_unknown_fields)]
pub enum SeriesInput {
    /// A named data series.
    Named {
        /// Series legend label.
        name: String,
        /// Finite numeric Y values in label order.
        values: Vec<f64>,
    },
    /// Legacy unnamed series accepted by the parser only.
    Bare(Vec<f64>),
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadInput {
    /// Input PPTX or POTX path.
    pub input: String,
    /// Maximum characters for each returned content representation.
    #[serde(default)]
    pub max_chars: Option<usize>,
    /// Select text, data, or both. max_chars bounds each representation's content.
    #[serde(default)]
    pub representation: ReadRepresentation,
    /// Zero-based first slide to return.
    #[serde(default)]
    pub offset: usize,
    /// Maximum number of slides in the page.
    #[serde(default)]
    pub limit: Option<usize>,
    /// Include company template layout descriptions.
    #[serde(default)]
    pub layouts: bool,
}

#[derive(Debug, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReadRepresentation {
    Text,
    Data,
    #[default]
    Both,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReplaceInput {
    /// Literal paragraph text to find.
    pub find: String,
    /// Replacement text.
    pub replace: String,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SlideEdit {
    /// Zero-based slide index.
    pub index: usize,
    /// Replacement heading text.
    #[serde(default)]
    pub title: Option<String>,
    /// Replacement body bullets.
    #[serde(default)]
    pub bullets: Option<Vec<String>>,
    /// Replacement speaker notes.
    #[serde(default)]
    pub notes: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EditInput {
    /// Existing PPTX path.
    pub input: String,
    /// Destination PPTX path; input identity is supported.
    pub output: String,
    /// Literal text replacement across paragraph runs.
    #[serde(default)]
    pub replace: Option<ReplaceInput>,
    /// Targeted edits by slide index.
    #[serde(default)]
    pub slides: Option<Vec<SlideEdit>>,
    /// Zero-based slide indices to remove.
    #[serde(default)]
    pub delete: Option<Vec<usize>>,
    /// New slides using the existing company layouts.
    #[serde(default)]
    pub add: Option<Vec<SlideInput>>,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConvertInput {
    /// Source PPTX or Markdown path.
    pub input: String,
    /// Destination PDF, Markdown or PPTX path, depending on the source.
    pub output: String,
    /// Company PPTX or POTX path, for Markdown to PPTX only.
    #[serde(default)]
    pub template: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CheckInput {
    /// Presentation PPTX path to inspect.
    pub input: String,
    /// Optional XLSX model path for cached-value comparisons.
    #[serde(default)]
    pub model: Option<String>,
    /// Absolute tolerance in normalized units (percentages use fractions). Default: 0.000001.
    #[serde(default)]
    pub tolerance: Option<f64>,
}

/// JSON Schema of [`Input`].
pub fn schema() -> serde_json::Value {
    request_schema::form_schema()
}

fn parser_schema() -> serde_json::Value {
    let mut schema =
        serde_json::to_value(schemars::schema_for!(Input)).unwrap_or(serde_json::Value::Null);
    if let Some(variants) = schema
        .get_mut("oneOf")
        .and_then(serde_json::Value::as_array_mut)
    {
        for variant in variants {
            if variant
                .pointer("/properties/op/const")
                .and_then(serde_json::Value::as_str)
                == Some("create")
            {
                variant["oneOf"] = serde_json::json!([
                    {"required":["markdown"], "properties":{"markdown":{"type":"string"},"slides":{"type":"null"}}},
                    {"required":["slides"], "properties":{"slides":{"type":"array"},"markdown":{"type":"null"}}}
                ]);
            }
        }
    }
    schema
}

/// Run one request.
pub fn run(input: &Input, opts: &Options) -> Result<Output, Error> {
    xmlw::validate_input(
        &serde_json::to_value(input).map_err(|source| Error::CheckJson { source })?,
        "",
    )?;
    // A directory inode is stable across atomic file replacements. Serialize writers before
    // reading the current input, including aliases and separate CLI processes in this base.
    #[cfg(target_os = "linux")]
    let _writer_lock = if matches!(input, Input::Create(_) | Input::Edit(_) | Input::Convert(_)) {
        let directory = std::fs::File::open(&opts.base_dir).map_err(|source| Error::Write {
            path: ".".into(),
            source,
        })?;
        directory.lock().map_err(|source| Error::Write {
            path: ".".into(),
            source,
        })?;
        Some(directory)
    } else {
        None
    };
    let result = match input {
        Input::Create(req) => create(req, opts),
        Input::Read(req) => read(req, opts),
        Input::Edit(req) => edit(req, opts),
        Input::Convert(req) => convert(req, opts),
        Input::Check(req) => checks::run(req, opts),
    };
    result.map_err(|error| contextual_error(error, input))
}

// -------------------------------------------------------------------------------------------
// Helpers

/// Resolve a request path against the base directory, rejecting absolute paths and `..`.
pub fn resolve_path(base: &Path, rel: &str) -> Result<PathBuf, Error> {
    if rel.is_empty() {
        return Err(Error::UnsafePath {
            path: rel.to_string(),
        });
    }
    let path = Path::new(rel);
    if path.is_absolute() {
        return Err(Error::UnsafePath {
            path: rel.to_string(),
        });
    }
    let mut out = base.to_path_buf();
    for component in path.components() {
        match component {
            Component::Normal(part) => {
                out.push(part);
                match std::fs::symlink_metadata(&out) {
                    Ok(meta) if meta.file_type().is_symlink() => {
                        return Err(input_error(
                            MessageCode::PathSymlink,
                            messages::params(&[("path", rel)]),
                        ));
                    }
                    Ok(_) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(source) => {
                        return Err(Error::Read {
                            path: rel.into(),
                            source,
                        });
                    }
                }
            }
            Component::CurDir => {}
            _ => {
                return Err(Error::UnsafePath {
                    path: rel.to_string(),
                });
            }
        }
    }
    // Existing symlinks, including a symlinked output parent, must not escape the base.
    let base_real = std::fs::canonicalize(base).map_err(|source| Error::Read {
        path: ".".to_string(),
        source,
    })?;
    let mut existing = out.as_path();
    while std::fs::symlink_metadata(existing).is_err() {
        existing = existing.parent().ok_or_else(|| Error::UnsafePath {
            path: rel.to_string(),
        })?;
    }
    let actual = std::fs::canonicalize(existing).map_err(|source| Error::Read {
        path: rel.to_string(),
        source,
    })?;
    if !actual.starts_with(base_real) {
        return Err(Error::UnsafePath {
            path: rel.to_string(),
        });
    }
    Ok(out)
}

// Anchor every parent directory on Linux. Renaming a checked parent cannot redirect later I/O.
#[cfg(target_os = "linux")]
fn anchored_path(base: &Path, rel: &str, create: bool) -> Result<(std::fs::File, PathBuf), Error> {
    file_io::parent(base, rel, create).map_err(|source| Error::Read {
        path: rel.into(),
        source,
    })
}

fn read_request(base: &Path, rel: &str, opts: &Options) -> Result<Vec<u8>, Error> {
    let path = resolve_path(base, rel)?;
    #[cfg(target_os = "linux")]
    let (_parent, path) = {
        let _ = path;
        anchored_path(base, rel, false)?
    };
    let meta = std::fs::symlink_metadata(&path).map_err(|source| Error::Read {
        path: rel.to_string(),
        source,
    })?;
    if meta.len() > opts.max_input_bytes {
        return Err(Error::TooLarge {
            path: rel.to_string(),
            size: meta.len(),
            limit: opts.max_input_bytes,
            field: String::new(),
        });
    }
    if !meta.is_file() {
        return Err(input_error(
            MessageCode::FileNotRegular,
            messages::params(&[("path", rel)]),
        ));
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // Linux O_NONBLOCK | O_NOFOLLOW: never wait for a swapped-in FIFO or follow a link.
        options.custom_flags(0x800 | 0x20000);
    }
    let mut file = options.open(&path).map_err(|source| Error::Read {
        path: rel.into(),
        source,
    })?;
    let opened = file.metadata().map_err(|source| Error::Read {
        path: rel.into(),
        source,
    })?;
    if !opened.is_file() {
        return Err(input_error(
            MessageCode::FileNotRegular,
            messages::params(&[("path", rel)]),
        ));
    }
    if opened.len() > opts.max_input_bytes {
        return Err(Error::TooLarge {
            path: rel.into(),
            size: opened.len(),
            limit: opts.max_input_bytes,
            field: String::new(),
        });
    }
    use std::io::Read;
    let mut bytes = Vec::new();
    (&mut file)
        .take(opts.max_input_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|source| Error::Read {
            path: rel.into(),
            source,
        })?;
    if bytes.len() as u64 > opts.max_input_bytes {
        return Err(Error::TooLarge {
            path: rel.into(),
            size: bytes.len() as u64,
            limit: opts.max_input_bytes,
            field: String::new(),
        });
    }
    Ok(bytes)
}

fn write_request(base: &Path, rel: &str, bytes: &[u8]) -> Result<(String, u64), Error> {
    let path = resolve_path(base, rel)?;
    #[cfg(target_os = "linux")]
    let (_parent, path) = {
        let _ = path;
        anchored_path(base, rel, true)?
    };
    #[cfg(not(target_os = "linux"))]
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| Error::Write {
            path: rel.into(),
            source,
        })?;
    }
    let parent = path
        .parent()
        .ok_or_else(|| Error::UnsafePath { path: rel.into() })?;
    let write_error = |source| Error::Write {
        path: rel.into(),
        source,
    };
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(write_error)?;
    use std::io::Write;
    for chunk in bytes.chunks(64 * 1024) {
        temporary.write_all(chunk).map_err(write_error)?;
    }
    temporary.as_file().sync_all().map_err(write_error)?;
    // Check the pinned destination, not the original pathname: a parent rename must not
    // redirect the write or turn a safe in-progress write into a symlink traversal.
    if std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(input_error(
            MessageCode::PathSymlink,
            messages::params(&[("path", rel)]),
        ));
    }
    #[cfg(target_os = "linux")]
    file_io::persist(&_parent, temporary.path(), &path).map_err(write_error)?;
    #[cfg(not(target_os = "linux"))]
    temporary
        .persist(&path)
        .map_err(|error| write_error(error.error))?;
    Ok((rel.to_string(), bytes.len() as u64))
}

fn same_file(base: &Path, input: &str, output: &str) -> Result<bool, Error> {
    let metadata = |rel: &str| -> Result<Option<std::fs::Metadata>, Error> {
        match std::fs::symlink_metadata(resolve_path(base, rel)?) {
            Ok(meta) => Ok(Some(meta)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(source) => Err(Error::Read {
                path: rel.into(),
                source,
            }),
        }
    };
    if let (Some(input), Some(output)) = (metadata(input)?, metadata(output)?) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            return Ok(input.is_file()
                && output.is_file()
                && input.dev() == output.dev()
                && input.ino() == output.ino());
        }
        #[cfg(not(unix))]
        {
            let _ = (input, output);
        }
    }
    Ok(false)
}

fn extension(path: &str) -> String {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

fn input_error(code: MessageCode, mut params: messages::Params) -> Error {
    let field_and_allowed = match code {
        MessageCode::UnknownTheme => Some(("theme", "light,dark,corporate,creative")),
        MessageCode::UnknownChartType => Some(("chart.type", "bar,column,line,pie,area,scatter")),
        MessageCode::UnknownLayout => Some((
            "layout",
            "title,title_content,two_content,section,image,table,quote,chart",
        )),
        MessageCode::TemplateOutput => Some(("template", "create PPTX,convert Markdown to PPTX")),
        MessageCode::CreateNoSource | MessageCode::CreateSourceConflict => {
            Some(("/slides", "exactly one non-null source"))
        }
        _ => None,
    };
    if let Some((field, allowed)) = field_and_allowed {
        params.extend(messages::params(&[("field", field), ("allowed", allowed)]));
    }
    Error::Input(Msg::new(code, params))
}

fn contextual_error(mut error: Error, input: &Input) -> Error {
    if let Error::Input(message) = &mut error {
        let field = match message.code {
            MessageCode::UnknownTheme => Some("/theme".to_owned()),
            MessageCode::TemplateOutput | MessageCode::TemplateFormat => {
                Some("/template".to_owned())
            }
            MessageCode::CreateNoSource | MessageCode::CreateSourceConflict => {
                Some("/slides".to_owned())
            }
            MessageCode::NoSlides => Some(
                match input {
                    Input::Create(req) if req.markdown.is_some() => "/markdown",
                    Input::Create(_) => "/slides",
                    _ => "/input",
                }
                .to_owned(),
            ),
            MessageCode::CheckTolerance => Some("/tolerance".to_owned()),
            MessageCode::ChartLimit
            | MessageCode::UnknownChartType
            | MessageCode::ChartEmpty
            | MessageCode::ChartScatterData
            | MessageCode::ChartPieData
            | MessageCode::ChartSeriesLength
            | MessageCode::ChartValueNotFinite
            | MessageCode::UnknownLayout => {
                let slides = match input {
                    Input::Create(req) if req.markdown.is_some() => {
                        message.params.retain(|(key, _)| key != "field");
                        message.params.push(("field".into(), "/markdown".into()));
                        return error;
                    }
                    Input::Convert(_) => {
                        message.params.retain(|(key, _)| key != "field");
                        message.params.push(("field".into(), "/input".into()));
                        return error;
                    }
                    Input::Create(req) => req.slides.as_deref(),
                    Input::Edit(req) => req.add.as_deref(),
                    _ => None,
                };
                let index = slides
                    .and_then(|slides| {
                        slides.iter().position(|slide| {
                            if message.code == MessageCode::UnknownLayout {
                                slide
                                    .layout
                                    .as_deref()
                                    .is_some_and(|name| Layout::parse(name).is_none())
                            } else {
                                slide
                                    .chart
                                    .as_ref()
                                    .is_some_and(|chart| chart_from(chart).is_err())
                            }
                        })
                    })
                    .unwrap_or(0);
                let source = if matches!(input, Input::Edit(_)) {
                    "add"
                } else {
                    "slides"
                };
                let suffix = match message.code {
                    MessageCode::UnknownLayout => "layout",
                    MessageCode::UnknownChartType => "chart/type",
                    _ => "chart/series",
                };
                Some(format!("/{source}/{index}/{suffix}"))
            }
            _ => None,
        };
        if let Some(field) = field {
            message.params.retain(|(key, _)| key != "field");
            message.params.push(("field".into(), field));
        }
    }
    if error.raw_msg().params.iter().any(|(key, _)| key == "field") {
        return error;
    }
    let path = match &error {
        Error::UnsafePath { path }
        | Error::TooLarge { path, .. }
        | Error::Read { path, .. }
        | Error::Write { path, .. } => Some(path.clone()),
        _ => error
            .raw_msg()
            .params
            .into_iter()
            .find(|(key, _)| key == "path")
            .map(|(_, value)| value),
    };
    fn path_field(value: &serde_json::Value, target: &str, prefix: &str) -> Option<String> {
        match value {
            serde_json::Value::Object(object) => object.iter().find_map(|(key, value)| {
                let field = format!("{prefix}/{}", key.replace('~', "~0").replace('/', "~1"));
                if matches!(
                    key.as_str(),
                    "input" | "output" | "image" | "template" | "model"
                ) && value.as_str() == Some(target)
                {
                    Some(field)
                } else {
                    path_field(value, target, &field)
                }
            }),
            serde_json::Value::Array(array) => array
                .iter()
                .enumerate()
                .find_map(|(i, value)| path_field(value, target, &format!("{prefix}/{i}"))),
            _ => None,
        }
    }
    let field = path
        .and_then(|target| {
            serde_json::to_value(input)
                .ok()
                .and_then(|value| path_field(&value, &target, ""))
        })
        .unwrap_or_else(|| {
            match input {
                Input::Create(req) if req.template.is_some() => "/template",
                Input::Create(req) if req.markdown.is_some() => "/markdown",
                Input::Create(_) => "/slides",
                Input::Read(_) | Input::Edit(_) | Input::Convert(_) | Input::Check(_) => "/input",
            }
            .to_owned()
        });
    if let Error::TooLarge { field: target, .. } = &mut error {
        *target = field;
        return error;
    }
    Error::AtField {
        source: Box::new(error),
        field,
    }
}

fn theme_from(name: Option<&str>) -> Result<Theme, Error> {
    match name.unwrap_or("light") {
        "light" => Ok(deck::theme("light")),
        "dark" => Ok(deck::theme("dark")),
        "corporate" => Ok(deck::theme("corporate")),
        "creative" => Ok(deck::theme("creative")),
        other => Err(input_error(
            MessageCode::UnknownTheme,
            messages::params(&[("theme", &format!("{other:?}"))]),
        )),
    }
}

fn chart_from(req: &ChartInput) -> Result<Chart, Error> {
    if req.labels.len() > 10_000
        || req.series.len() > 100
        || req.series.iter().any(|s| match s {
            SeriesInput::Named { values, .. } | SeriesInput::Bare(values) => values.len() > 10_000,
        })
    {
        return Err(input_error(
            MessageCode::ChartLimit,
            messages::params(&[("field", "chart.series")]),
        ));
    }
    let kind = ChartKind::parse(&req.kind).ok_or_else(|| {
        input_error(
            MessageCode::UnknownChartType,
            messages::params(&[("kind", &format!("{:?}", req.kind))]),
        )
    })?;
    if req.labels.is_empty() || req.series.is_empty() {
        return Err(input_error(MessageCode::ChartEmpty, Vec::new()));
    }
    if kind == ChartKind::Scatter
        && req
            .labels
            .iter()
            .any(|v| v.parse::<f64>().map_or(true, |n| !n.is_finite()))
    {
        return Err(input_error(MessageCode::ChartScatterData, Vec::new()));
    }
    let mut series = Vec::new();
    for (i, item) in req.series.iter().enumerate() {
        let (name, values) = match item {
            SeriesInput::Named { name, values } => (name.clone(), values.clone()),
            SeriesInput::Bare(values) => ((i + 1).to_string(), values.clone()),
        };
        if values.len() != req.labels.len() {
            return Err(input_error(
                MessageCode::ChartSeriesLength,
                messages::params(&[
                    ("series", &name),
                    ("values", &values.len().to_string()),
                    ("labels", &req.labels.len().to_string()),
                ]),
            ));
        }
        if values.iter().any(|v| !v.is_finite()) {
            return Err(input_error(
                MessageCode::ChartValueNotFinite,
                messages::params(&[("series", &name)]),
            ));
        }
        series.push(Series { name, values });
    }
    if kind == ChartKind::Pie {
        let total: f64 = series.iter().flat_map(|s| &s.values).sum();
        if series.len() != 1
            || series.iter().flat_map(|s| &s.values).any(|v| *v < 0.0)
            || !total.is_finite()
            || total <= 0.0
        {
            return Err(input_error(MessageCode::ChartPieData, Vec::new()));
        }
    }
    Ok(Chart {
        kind,
        labels: req.labels.clone(),
        series,
    })
}

/// Convert one JSON slide into the internal model. Shared by `create` and `edit` (`add`).
pub(crate) fn slide_from_input(req: &SlideInput) -> Result<Slide, Error> {
    let mut slide = Slide {
        title: req.title.clone().unwrap_or_default(),
        subtitle: req.subtitle.clone().unwrap_or_default(),
        notes: req.notes.clone().unwrap_or_default(),
        short_slide_accent: req.short_slide_accent.unwrap_or(true),
        sources: req.sources.clone(),
        ..Slide::default()
    };
    if let Some(bullets) = &req.bullets
        && !bullets.is_empty()
    {
        slide.blocks.push(Block::Bullets(bullets.clone()));
    }
    if let Some(text) = &req.text
        && !text.is_empty()
    {
        slide.blocks.push(Block::Text(text.clone()));
    }
    if let Some(code) = &req.code {
        slide.blocks.push(Block::Code(code.clone()));
    }
    if let Some(table) = &req.table {
        slide.blocks.push(Block::Table(Table {
            headers: table.headers.clone(),
            rows: table.rows.clone(),
            ..Table::default()
        }));
    }
    if let Some(chart) = &req.chart {
        slide.blocks.push(Block::Chart(chart_from(chart)?));
    }
    if let Some(image) = &req.image {
        slide.blocks.push(Block::Image(image.clone()));
    }
    slide.layout = match &req.layout {
        Some(name) => Layout::parse(name).ok_or_else(|| {
            input_error(
                MessageCode::UnknownLayout,
                messages::params(&[("layout", &format!("{name:?}"))]),
            )
        })?,
        None => Deck::infer_layout(&slide),
    };
    Ok(slide)
}

fn deck_from_slides(slides: &[SlideInput]) -> Result<Deck, Error> {
    let mut deck = Deck::default();
    for req in slides {
        deck.slides.push(slide_from_input(req)?);
    }
    Ok(deck)
}

fn chart_text(chart: &Chart) -> String {
    let mut rows = vec![chart.labels.join("\t")];
    for series in &chart.series {
        let values: Vec<String> = series
            .values
            .iter()
            .map(|v| {
                if *v == v.trunc() && v.abs() < 1e15 {
                    format!("{}", *v as i64)
                } else {
                    format!("{v}")
                }
            })
            .collect();
        rows.push(format!("{}\t{}", series.name, values.join("\t")));
    }
    rows.join("\n")
}

fn block_text(block: &Block) -> String {
    match block {
        Block::Bullets(items) => items.join("\n"),
        Block::Text(text) | Block::Code(text) => text.clone(),
        Block::Image(path) => path.clone(),
        Block::Chart(chart) => chart_text(chart),
        Block::Table(table) => {
            let mut rows = Vec::new();
            if !table.headers.is_empty() {
                rows.push(table.headers.join("\t"));
            }
            for row in &table.rows {
                rows.push(row.join("\t"));
            }
            rows.join("\n")
        }
    }
}

fn slide_json(deck: &Deck) -> serde_json::Value {
    let slides: Vec<serde_json::Value> = deck
        .slides
        .iter()
        .enumerate()
        .map(|(index, slide)| {
            let text = slide
                .blocks
                .iter()
                .map(block_text)
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join("\n");
            serde_json::json!({
                "index": index,
                "layout": slide.layout.as_str(),
                "title": slide.title,
                "subtitle": slide.subtitle,
                "text": text,
                "notes": slide.notes,
                "sources": slide.sources,
            })
        })
        .collect();
    serde_json::Value::Array(slides)
}

fn truncate(text: String, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text;
    }
    text.chars().take(max_chars).collect()
}

// -------------------------------------------------------------------------------------------
// Operations

fn validate_assets(deck: &Deck, opts: &Options) -> Result<(), Error> {
    for slide in &deck.slides {
        for block in &slide.blocks {
            if let Block::Image(path) = block {
                read_request(&opts.base_dir, path, opts)?;
            }
        }
    }
    Ok(())
}

fn create(req: &CreateInput, opts: &Options) -> Result<Output, Error> {
    if req.markdown.is_some() && req.slides.is_some() {
        return Err(input_error(MessageCode::CreateSourceConflict, Vec::new()));
    }
    let size = if let Some(markdown) = &req.markdown {
        markdown.len() as u64
    } else if let Some(slides) = &req.slides {
        serde_json::to_vec(slides)
            .map_err(|source| Error::CheckJson { source })?
            .len() as u64
    } else {
        0
    };
    if size > opts.max_input_bytes {
        return Err(Error::TooLarge {
            path: "create".into(),
            size,
            limit: opts.max_input_bytes,
            field: if req.markdown.is_some() {
                "/markdown"
            } else {
                "/slides"
            }
            .into(),
        });
    }
    resolve_path(&opts.base_dir, &req.output)?;
    let theme = theme_from(req.theme.as_deref())?;
    let deck = if let Some(markdown) = &req.markdown {
        markdown::parse(markdown)?
    } else if let Some(slides) = &req.slides {
        if req.template.is_some() {
            templates::deck_from_slides(slides)?
        } else {
            deck_from_slides(slides)?
        }
    } else {
        return Err(input_error(MessageCode::CreateNoSource, Vec::new()));
    };
    if deck.slides.is_empty() {
        return Err(input_error(MessageCode::NoSlides, Vec::new()));
    }
    validate_assets(&deck, opts)?;
    let format = extension(&req.output);
    let template = templates::load(req.template.as_deref(), opts)?;
    if format == "pdf" && template.is_some() {
        return Err(input_error(MessageCode::TemplateOutput, Vec::new()));
    }
    let (deck, mut warnings) = templates::paginate(deck, theme, template.as_deref(), opts)?;
    let (bytes, export_warnings) = match format.as_str() {
        "pptx" => templates::write(&deck, theme, template.as_deref(), opts)?,
        "pdf" => pdf::write(&deck, theme, opts)?,
        _ => {
            return Err(Error::Unsupported(Msg::new(
                MessageCode::UnsupportedCreateOutput,
                messages::params(&[("path", &req.output)]),
            )));
        }
    };
    warnings.extend(export_warnings);
    let (out, size) = write_request(&opts.base_dir, &req.output, &bytes)?;
    Ok(output(
        Some(out),
        Some(size),
        Msg::new(
            MessageCode::CreateDone,
            messages::params(&[
                ("count", &deck.slides.len().to_string()),
                ("format", &format),
            ]),
        ),
        None,
        None,
        warnings,
        &opts.locale,
    ))
}

fn read(req: &ReadInput, opts: &Options) -> Result<Output, Error> {
    let bytes = read_request(&opts.base_dir, &req.input, opts)?;
    if extension(&req.input) != "pptx" && !(req.layouts && extension(&req.input) == "potx") {
        return Err(Error::Unsupported(Msg::new(
            MessageCode::UnsupportedReadInput,
            messages::params(&[("path", &req.input)]),
        )));
    }
    let layouts = req
        .layouts
        .then(|| templates::list(&bytes, opts.max_input_bytes))
        .transpose()?;
    let (deck, mut warnings) =
        pptx::read_with_warnings(&bytes, opts.max_input_bytes, &opts.locale)?;
    let maximum = req.max_chars.unwrap_or(20_000);
    let total = deck.slides.len();
    let mut page = deck;
    page.slides = page
        .slides
        .into_iter()
        .skip(req.offset)
        .take(req.limit.unwrap_or(usize::MAX))
        .collect();
    let include_text = !matches!(req.representation, ReadRepresentation::Data);
    let include_data = !matches!(req.representation, ReadRepresentation::Text);
    let full_text = markdown::render(&page);
    let text_truncated = include_text && full_text.chars().count() > maximum;
    let text = include_text.then(|| truncate(full_text, maximum));
    // Whole data records only: never silently drop individual fields from a slide.
    let all = slide_json(&page);
    let mut records = Vec::new();
    let mut used = 2usize;
    if include_data {
        for record in all.as_array().into_iter().flatten() {
            let size = record.to_string().chars().count().saturating_add(1);
            if used.saturating_add(size) > maximum {
                break;
            }
            used += size;
            let mut record = record.clone();
            record["index"] = serde_json::json!(req.offset.saturating_add(records.len()));
            records.push(record);
        }
    }
    let returned = records.len();
    // Text represents the requested slide page even when its character budget truncates it.
    // When data is requested, advance only past complete records actually returned.
    let consumed = if include_data {
        returned
    } else {
        page.slides.len()
    };
    let data_truncated = include_data && returned < page.slides.len();
    let mut data = include_data.then_some(serde_json::Value::Array(records));
    let mut layouts_truncated = false;
    if let Some(layouts) = layouts
        && include_data
    {
        let value = serde_json::json!({"slides":data, "layouts":layouts});
        if value.to_string().chars().count() <= maximum {
            data = Some(value);
        } else {
            layouts_truncated = true;
            data = Some(serde_json::json!({"slides":data, "layouts":[]}));
        }
    }
    for (representation, truncated) in [
        ("text", text_truncated),
        ("data", data_truncated),
        ("layouts", layouts_truncated),
    ] {
        if truncated {
            warnings.push(Warning::new(
                MessageCode::ReadTruncated,
                messages::params(&[("representation", representation)]),
                &opts.locale,
            ));
        }
    }
    let mut result = output(
        None,
        Some(bytes.len() as u64),
        Msg::new(
            MessageCode::ReadDone,
            messages::params(&[("count", &total.to_string())]),
        ),
        text,
        data,
        warnings,
        &opts.locale,
    );
    result.truncation = Some(serde_json::json!({
        "text":text_truncated, "data":data_truncated, "layouts":layouts_truncated,
        "total_slides":total, "offset":req.offset, "page_slides":page.slides.len(),
        "returned_data_slides":returned,
        "next_offset": if req.offset.saturating_add(consumed) < total { Some(req.offset.saturating_add(consumed)) } else { None }
    }));
    Ok(result)
}

fn edit(req: &EditInput, opts: &Options) -> Result<Output, Error> {
    resolve_path(&opts.base_dir, &req.output)?;
    let in_place = same_file(&opts.base_dir, &req.input, &req.output)?;
    if let Some(add) = &req.add {
        validate_assets(&templates::deck_from_slides(add)?, opts)?;
    }
    let bytes = read_request(&opts.base_dir, &req.input, opts)?;
    if extension(&req.input) != "pptx" {
        return Err(Error::Unsupported(Msg::new(
            MessageCode::UnsupportedEditInput,
            messages::params(&[("path", &req.input)]),
        )));
    }
    let (out_bytes, mut warnings) = pptx::edit(&bytes, req, &opts.locale, opts.max_input_bytes)?;
    let out_bytes = if let Some(add) = &req.add {
        let (bytes, added_warnings) = templates::append(&out_bytes, add, opts)?;
        warnings.extend(added_warnings);
        bytes
    } else {
        out_bytes
    };
    let mut package = ooxml::Package::open(&out_bytes, opts.max_input_bytes)?;
    if req
        .delete
        .as_ref()
        .is_some_and(|indices| !indices.is_empty())
    {
        package.prune_unreachable()?;
    }
    package.validate_relationships()?;
    let out_bytes = package.finish()?;
    let (out, size) = write_request(&opts.base_dir, &req.output, &out_bytes)?;
    Ok(output(
        Some(out),
        Some(size),
        Msg::new(
            MessageCode::EditDone,
            messages::params(&[("in_place", if in_place { "true" } else { "false" })]),
        ),
        None,
        None,
        warnings,
        &opts.locale,
    ))
}

/// Export package assets with generated names, never ZIP paths. Every companion file uses
/// the same checked, directory-pinned atomic writer as the Markdown document itself.
fn export_markdown(deck: &Deck, output: &str, opts: &Options) -> Result<Vec<u8>, Error> {
    let output = Path::new(output);
    let stem = output
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| Error::UnsafePath {
            path: output.to_string_lossy().into_owned(),
        })?;
    let media = format!("{stem}_media");
    let parent = output.parent().unwrap_or(Path::new(""));
    let mut exported = deck.clone();
    let mut paths = std::collections::BTreeMap::new();
    for (index, (source, bytes)) in deck.assets.iter().enumerate() {
        let extension = extension(source);
        let extension = if extension.len() <= 12
            && !extension.is_empty()
            && extension.bytes().all(|b| b.is_ascii_alphanumeric())
        {
            &extension
        } else {
            "bin"
        };
        let link = format!("{media}/image{}.{}", index + 1, extension);
        let destination = parent.join(&link).to_string_lossy().into_owned();
        // Check every destination before publishing any companion file.
        resolve_path(&opts.base_dir, &destination)?;
        paths.insert(source.clone(), (link, destination, bytes));
    }
    for slide in &mut exported.slides {
        for block in &mut slide.blocks {
            if let Block::Image(path) = block {
                let (link, _, _) = paths.get(path).ok_or_else(|| {
                    input_error(
                        MessageCode::ImageNotFound,
                        messages::params(&[("slide", "export"), ("path", path)]),
                    )
                })?;
                *path = link.clone();
            }
        }
    }
    for (_, destination, bytes) in paths.values() {
        write_request(&opts.base_dir, destination, bytes)?;
    }
    Ok(markdown::render(&exported).into_bytes())
}

fn convert(req: &ConvertInput, opts: &Options) -> Result<Output, Error> {
    if req.template.is_some()
        && (!matches!(extension(&req.input).as_str(), "md" | "markdown")
            || extension(&req.output) != "pptx")
    {
        return Err(input_error(MessageCode::TemplateOutput, Vec::new()));
    }
    let template = templates::load(req.template.as_deref(), opts)?;
    resolve_path(&opts.base_dir, &req.output)?;
    let bytes = read_request(&opts.base_dir, &req.input, opts)?;
    let input_ext = extension(&req.input);
    let in_place = same_file(&opts.base_dir, &req.input, &req.output)?;
    if in_place {
        return Err(input_error(MessageCode::ConvertSameFile, Vec::new()));
    }
    if input_ext == "pptx" && extension(&req.output) == "pdf" {
        let (deck, warnings) =
            pptx::read_with_warnings(&bytes, opts.max_input_bytes, &opts.locale)?;
        let count = deck.slides.len();
        let pdf = pdf::from_pptx(&bytes, opts)?;
        let (out, size) = write_request(&opts.base_dir, &req.output, &pdf)?;
        return Ok(output(
            Some(out),
            Some(size),
            Msg::new(
                MessageCode::ConvertDone,
                messages::params(&[
                    ("from", "pptx"),
                    ("to", "pdf"),
                    ("count", &count.to_string()),
                    ("in_place", if in_place { "true" } else { "false" }),
                ]),
            ),
            None,
            None,
            warnings,
            &opts.locale,
        ));
    }
    let mut read_warnings = Vec::new();
    let mut deck = match input_ext.as_str() {
        "md" | "markdown" => {
            let text = String::from_utf8(bytes).map_err(|source| Error::MarkdownUtf8 { source })?;
            xmlw::validate_text(&text, "input.markdown")?;
            markdown::parse(&text)?
        }
        "pptx" => {
            let (deck, warnings) =
                pptx::read_with_warnings(&bytes, opts.max_input_bytes, &opts.locale)?;
            read_warnings = warnings;
            deck
        }
        _ => {
            return Err(Error::Unsupported(Msg::new(
                MessageCode::UnsupportedConvertInput,
                messages::params(&[("path", &req.input)]),
            )));
        }
    };
    if matches!(input_ext.as_str(), "md" | "markdown") {
        // Markdown links are relative to their document, not the process working directory.
        let parent = Path::new(&req.input).parent().unwrap_or(Path::new(""));
        for slide in &mut deck.slides {
            for block in &mut slide.blocks {
                if let Block::Image(path) = block {
                    // Validate the untrusted link before joining, including absolute paths.
                    resolve_path(&opts.base_dir, path)?;
                    *path = parent.join(&*path).to_string_lossy().into_owned();
                }
            }
        }
        validate_assets(&deck, opts)?;
    }
    let theme = deck::theme("light");
    let output_ext = extension(&req.output);
    if !matches!(output_ext.as_str(), "pptx" | "pdf" | "md" | "markdown") {
        return Err(Error::Unsupported(Msg::new(
            MessageCode::UnsupportedConvertOutput,
            messages::params(&[("path", &req.output)]),
        )));
    }
    if !matches!(
        (input_ext.as_str(), output_ext.as_str()),
        ("md" | "markdown", "pptx" | "pdf") | ("pptx", "md" | "markdown" | "pdf")
    ) {
        return Err(input_error(
            MessageCode::ConvertUnsupportedPair,
            messages::params(&[("from", &input_ext), ("to", &output_ext)]),
        ));
    }
    let (deck, mut warnings) = if matches!(output_ext.as_str(), "pptx" | "pdf") {
        templates::paginate(deck, theme, template.as_deref(), opts)?
    } else {
        (deck, Vec::new())
    };
    let (out_bytes, export_warnings) = match output_ext.as_str() {
        "pptx" => templates::write(&deck, theme, template.as_deref(), opts)?,
        "pdf" => pdf::write(&deck, theme, opts)?,
        "md" | "markdown" => (export_markdown(&deck, &req.output, opts)?, Vec::new()),
        _ => {
            return Err(Error::Unsupported(Msg::new(
                MessageCode::UnsupportedConvertOutput,
                messages::params(&[("path", &req.output)]),
            )));
        }
    };
    warnings.extend(read_warnings);
    warnings.extend(export_warnings);
    let (out, size) = write_request(&opts.base_dir, &req.output, &out_bytes)?;
    Ok(output(
        Some(out),
        Some(size),
        Msg::new(
            MessageCode::ConvertDone,
            messages::params(&[
                ("from", &input_ext),
                ("to", &output_ext),
                ("count", &deck.slides.len().to_string()),
                ("in_place", if in_place { "true" } else { "false" }),
            ]),
        ),
        None,
        None,
        warnings,
        &opts.locale,
    ))
}
