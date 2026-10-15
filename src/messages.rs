//! The message catalog: every user-facing message in the crate lives here.
//!
//! Each message has a stable [`MessageCode`] and an English template with `{name}`
//! placeholders. [`render`] fills a template for a locale; `en` is the only built-in locale,
//! and any unknown locale falls back to it. Adding a language means adding a template table,
//! not touching the logic that raises messages.

use serde::Serialize;

/// The English one-line description of the tool.
pub const DESCRIPTION: &str =
    "Create, read, edit and convert PPTX/PDF presentations from Markdown or a small JSON request.";

/// Ordered message parameters: `(name, value)` pairs.
pub type Params = Vec<(String, String)>;

/// Build [`Params`] from literal pairs.
pub fn params(pairs: &[(&str, &str)]) -> Params {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// Parameters as a JSON object (numbers stay strings; they are display values).
pub fn params_json(params: &Params) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for (k, v) in params {
        let value = if k == "allowed" {
            serde_json::Value::Array(
                v.split(',')
                    .map(|v| serde_json::Value::String(v.to_owned()))
                    .collect(),
            )
        } else {
            serde_json::Value::String(v.clone())
        };
        map.insert(k.clone(), value);
    }
    serde_json::Value::Object(map)
}

fn bounded_params(mut params: Params) -> Params {
    let mut truncated = Vec::new();
    for (name, value) in &mut params {
        if value.chars().count() > 256 {
            *value = value.chars().take(256).collect();
            if matches!(name.as_str(), "field" | "pointer") && value.starts_with('/') {
                // Point to the containing object rather than cut an RFC 6901 escape/token.
                value.truncate(value.rfind('/').unwrap_or(0));
            }
            truncated.push(name.clone());
        }
    }
    if !truncated.is_empty() {
        params.push(("truncated_fields".into(), truncated.join(",")));
    }
    params
}

/// A message ready to render: a code plus its parameters.
#[derive(Clone, Debug)]
pub struct Msg {
    pub code: MessageCode,
    pub params: Params,
}

impl Msg {
    pub fn new(code: MessageCode, params: Params) -> Self {
        Msg {
            code,
            params: bounded_params(params),
        }
    }

    pub fn render(&self, locale: &str) -> String {
        render(self.code, &self.params, locale)
    }
}

/// One warning entry of a result: stable code, rendered message, and raw parameters.
#[derive(Clone, Debug, Serialize)]
pub struct Warning {
    pub code: &'static str,
    pub message: String,
    pub params: serde_json::Value,
}

impl Warning {
    pub fn new(code: MessageCode, params: Params, locale: &str) -> Self {
        let params = bounded_params(params);
        Warning {
            code: code.as_str(),
            message: render(code, &params, locale),
            params: params_json(&params),
        }
    }
}

/// Stable codes for every message the crate can produce.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageCode {
    ChartLimit,
    ChartLabelReview,
    ConvertSameFile,
    ConvertUnsupportedPair,
    XmlDepthLimit,
    XmlCharacter,
    CreateSourceConflict,
    PdfTimeout,
    CliArgumentEncoding,
    CliArguments,
    PathSymlink,
    FileNotRegular,
    ReadTruncated,
    ReadCompatibilityUnsupported,
    ReadChartUnsupported,
    ReadChartIncomplete,
    ReadChartCombination,
    CheckUnclassifiedNumbers,
    ResponseTruncated,
    TemplateContentReview,
    TemplatePlaceholderFallback,
    SourceOverflow,
    CheckDone,
    CheckTolerance,
    CheckScope,
    ModelInvalid,
    ModelUnlinked,
    ModelMismatch,
    MetricMismatch,
    TableTotal,
    TablePercent,
    SourceUnresolved,
    TemplateOutput,
    TemplateFormat,
    TemplateFallback,
    PackageInvalid,
    CreateDone,
    ReadDone,
    EditDone,
    ConvertDone,
    SlideTextOverflow,
    SlideSplit,
    ChartScatterData,
    SlideCannotFit,
    SlideTitleOverflow,
    ImageUnsafePath,
    ImageUnreadable,
    ImageUnsupported,
    ImageNotFound,
    ImageNoSize,
    ImageNotEmbedded,
    FontWarning,
    EditSlideMissing,
    EditFieldMissing,
    EditTextOverflow,
    EditNoMatch,
    EditTextFitted,
    InputBadRequest,
    UnknownTheme,
    UnknownLayout,
    UnknownChartType,
    CreateNoSource,
    NoSlides,
    MarkdownNotUtf8,
    ChartEmpty,
    ChartInvalidData,
    ChartPieData,
    ChartSeriesLength,
    ChartValueNotFinite,
    UnsafePath,
    TooLarge,
    FileRead,
    FileWrite,
    ZipInvalid,
    XmlInvalid,
    XmlMultipleRoots,
    XmlEmpty,
    PptxMissingPresentation,
    PptxNoShapeTree,
    PptxBadRelationships,
    PptxNoLayout,
    PptxNoSlideList,
    PdfIo,
    PdfRender,
    PdfFontRead,
    PdfFontMissing,
    PdfFontParse,
    UnsupportedCreateOutput,
    UnsupportedReadInput,
    UnsupportedEditInput,
    UnsupportedConvertInput,
    UnsupportedConvertOutput,
    CliHelp,
    CliCurrentDir,
    CliSchemaRender,
    CliStdinRead,
    CliInvalidJson,
    CliResultRender,
}

impl MessageCode {
    /// Every code, for the catalog completeness test.
    pub const ALL: &'static [MessageCode] = &[
        MessageCode::ChartLimit,
        MessageCode::ChartLabelReview,
        MessageCode::ConvertSameFile,
        MessageCode::ConvertUnsupportedPair,
        MessageCode::XmlDepthLimit,
        MessageCode::XmlCharacter,
        MessageCode::CreateSourceConflict,
        MessageCode::PdfTimeout,
        MessageCode::CliArgumentEncoding,
        MessageCode::CliArguments,
        MessageCode::PathSymlink,
        MessageCode::FileNotRegular,
        MessageCode::ReadTruncated,
        MessageCode::ReadCompatibilityUnsupported,
        MessageCode::ReadChartUnsupported,
        MessageCode::ReadChartIncomplete,
        MessageCode::ReadChartCombination,
        MessageCode::CheckUnclassifiedNumbers,
        MessageCode::ResponseTruncated,
        MessageCode::TemplateContentReview,
        MessageCode::TemplatePlaceholderFallback,
        MessageCode::SourceOverflow,
        MessageCode::CheckDone,
        MessageCode::CheckTolerance,
        MessageCode::CheckScope,
        MessageCode::ModelInvalid,
        MessageCode::ModelUnlinked,
        MessageCode::ModelMismatch,
        MessageCode::MetricMismatch,
        MessageCode::TableTotal,
        MessageCode::TablePercent,
        MessageCode::SourceUnresolved,
        MessageCode::TemplateOutput,
        MessageCode::TemplateFormat,
        MessageCode::TemplateFallback,
        MessageCode::PackageInvalid,
        MessageCode::CreateDone,
        MessageCode::ReadDone,
        MessageCode::EditDone,
        MessageCode::ConvertDone,
        MessageCode::SlideTextOverflow,
        MessageCode::SlideSplit,
        MessageCode::ChartScatterData,
        MessageCode::SlideCannotFit,
        MessageCode::SlideTitleOverflow,
        MessageCode::ImageUnsafePath,
        MessageCode::ImageUnreadable,
        MessageCode::ImageUnsupported,
        MessageCode::ImageNotFound,
        MessageCode::ImageNoSize,
        MessageCode::ImageNotEmbedded,
        MessageCode::FontWarning,
        MessageCode::EditSlideMissing,
        MessageCode::EditFieldMissing,
        MessageCode::EditTextOverflow,
        MessageCode::EditNoMatch,
        MessageCode::EditTextFitted,
        MessageCode::InputBadRequest,
        MessageCode::UnknownTheme,
        MessageCode::UnknownLayout,
        MessageCode::UnknownChartType,
        MessageCode::CreateNoSource,
        MessageCode::NoSlides,
        MessageCode::MarkdownNotUtf8,
        MessageCode::ChartEmpty,
        MessageCode::ChartInvalidData,
        MessageCode::ChartPieData,
        MessageCode::ChartSeriesLength,
        MessageCode::ChartValueNotFinite,
        MessageCode::UnsafePath,
        MessageCode::TooLarge,
        MessageCode::FileRead,
        MessageCode::FileWrite,
        MessageCode::ZipInvalid,
        MessageCode::XmlInvalid,
        MessageCode::XmlMultipleRoots,
        MessageCode::XmlEmpty,
        MessageCode::PptxMissingPresentation,
        MessageCode::PptxNoShapeTree,
        MessageCode::PptxBadRelationships,
        MessageCode::PptxNoLayout,
        MessageCode::PptxNoSlideList,
        MessageCode::PdfIo,
        MessageCode::PdfRender,
        MessageCode::PdfFontRead,
        MessageCode::PdfFontMissing,
        MessageCode::PdfFontParse,
        MessageCode::UnsupportedCreateOutput,
        MessageCode::UnsupportedReadInput,
        MessageCode::UnsupportedEditInput,
        MessageCode::UnsupportedConvertInput,
        MessageCode::UnsupportedConvertOutput,
        MessageCode::CliHelp,
        MessageCode::CliCurrentDir,
        MessageCode::CliSchemaRender,
        MessageCode::CliStdinRead,
        MessageCode::CliInvalidJson,
        MessageCode::CliResultRender,
    ];

    /// The stable dotted code, as reported in JSON output.
    pub fn as_str(self) -> &'static str {
        match self {
            MessageCode::ChartLimit => "chart.limit",
            MessageCode::ChartLabelReview => "chart.labels_review",
            MessageCode::ConvertSameFile => "convert.same_file",
            MessageCode::ConvertUnsupportedPair => "convert.unsupported_pair",
            MessageCode::XmlDepthLimit => "xml.depth_limit",
            MessageCode::XmlCharacter => "input.xml_character",
            MessageCode::CreateSourceConflict => "input.create_source_conflict",
            MessageCode::PdfTimeout => "pdf.timeout",
            MessageCode::CliArgumentEncoding => "cli.argument_encoding",
            MessageCode::CliArguments => "cli.arguments",
            MessageCode::PathSymlink => "path.symlink",
            MessageCode::FileNotRegular => "file.not_regular",
            MessageCode::ReadTruncated => "read.truncated",
            MessageCode::ReadCompatibilityUnsupported => "read.compatibility_unsupported",
            MessageCode::ReadChartUnsupported => "read.chart_unsupported",
            MessageCode::ReadChartIncomplete => "read.chart_incomplete",
            MessageCode::ReadChartCombination => "read.chart_combination",
            MessageCode::CheckUnclassifiedNumbers => "check.unclassified_numbers",
            MessageCode::ResponseTruncated => "response.truncated",
            MessageCode::TemplateContentReview => "template.content_review",
            MessageCode::TemplatePlaceholderFallback => "template.placeholder_fallback",
            MessageCode::SourceOverflow => "source.overflow",
            MessageCode::CheckDone => "check.done",
            MessageCode::CheckTolerance => "check.invalid_tolerance",
            MessageCode::CheckScope => "check.scope",
            MessageCode::ModelInvalid => "check.invalid_model",
            MessageCode::ModelUnlinked => "check.model_unlinked",
            MessageCode::ModelMismatch => "check.model_mismatch",
            MessageCode::MetricMismatch => "check.metric_mismatch",
            MessageCode::TableTotal => "check.table_total",
            MessageCode::TablePercent => "check.table_percent",
            MessageCode::SourceUnresolved => "check.source_unresolved",
            MessageCode::TemplateOutput => "template.output",
            MessageCode::TemplateFormat => "template.format",
            MessageCode::TemplateFallback => "template.layout_fallback",
            MessageCode::PackageInvalid => "package.invalid",
            MessageCode::CreateDone => "create.done",
            MessageCode::ReadDone => "read.done",
            MessageCode::EditDone => "edit.done",
            MessageCode::ConvertDone => "convert.done",
            MessageCode::SlideTextOverflow => "slide.text_overflow",
            MessageCode::SlideSplit => "slide.split",
            MessageCode::ChartScatterData => "input.chart_scatter_data",
            MessageCode::SlideCannotFit => "slide.cannot_fit",
            MessageCode::SlideTitleOverflow => "slide.title_overflow",
            MessageCode::ImageUnsafePath => "image.unsafe_path",
            MessageCode::ImageUnreadable => "image.unreadable",
            MessageCode::ImageUnsupported => "image.unsupported_format",
            MessageCode::ImageNotFound => "image.not_found",
            MessageCode::ImageNoSize => "image.no_size",
            MessageCode::ImageNotEmbedded => "image.not_embedded",
            MessageCode::FontWarning => "pdf.font_warning",
            MessageCode::EditFieldMissing => "edit.field_missing",
            MessageCode::EditSlideMissing => "edit.slide_missing",
            MessageCode::EditTextOverflow => "edit.text_overflow",
            MessageCode::EditNoMatch => "edit.no_match",
            MessageCode::EditTextFitted => "edit.text_fitted",
            MessageCode::InputBadRequest => "input.bad_request",
            MessageCode::UnknownTheme => "input.unknown_theme",
            MessageCode::UnknownLayout => "input.unknown_layout",
            MessageCode::UnknownChartType => "input.unknown_chart_type",
            MessageCode::CreateNoSource => "input.create_no_source",
            MessageCode::NoSlides => "input.no_slides",
            MessageCode::MarkdownNotUtf8 => "input.markdown_not_utf8",
            MessageCode::ChartEmpty => "input.chart_empty",
            MessageCode::ChartInvalidData => "chart.invalid_data",
            MessageCode::ChartPieData => "input.chart_pie_data",
            MessageCode::ChartSeriesLength => "input.chart_series_length",
            MessageCode::ChartValueNotFinite => "input.chart_value_not_finite",
            MessageCode::UnsafePath => "path.unsafe",
            MessageCode::TooLarge => "input.too_large",
            MessageCode::FileRead => "file.read",
            MessageCode::FileWrite => "file.write",
            MessageCode::ZipInvalid => "zip.invalid",
            MessageCode::XmlInvalid => "xml.invalid",
            MessageCode::XmlMultipleRoots => "xml.multiple_roots",
            MessageCode::XmlEmpty => "xml.empty",
            MessageCode::PptxMissingPresentation => "pptx.missing_presentation",
            MessageCode::PptxNoShapeTree => "pptx.no_shape_tree",
            MessageCode::PptxBadRelationships => "pptx.bad_relationships",
            MessageCode::PptxNoLayout => "pptx.no_layout",
            MessageCode::PptxNoSlideList => "pptx.no_slide_list",
            MessageCode::PdfIo => "pdf.io",
            MessageCode::PdfRender => "pdf.render",
            MessageCode::PdfFontRead => "pdf.font_read",
            MessageCode::PdfFontMissing => "pdf.font_missing",
            MessageCode::PdfFontParse => "pdf.font_parse",
            MessageCode::UnsupportedCreateOutput => "unsupported.create_output",
            MessageCode::UnsupportedReadInput => "unsupported.read_input",
            MessageCode::UnsupportedEditInput => "unsupported.edit_input",
            MessageCode::UnsupportedConvertInput => "unsupported.convert_input",
            MessageCode::UnsupportedConvertOutput => "unsupported.convert_output",
            MessageCode::CliHelp => "cli.help",
            MessageCode::CliCurrentDir => "cli.current_dir",
            MessageCode::CliSchemaRender => "cli.schema_render",
            MessageCode::CliStdinRead => "cli.stdin_read",
            MessageCode::CliInvalidJson => "cli.invalid_json",
            MessageCode::CliResultRender => "cli.result_render",
        }
    }
}

/// English templates, one per code. `{name}` marks a parameter.
const EN: &[(MessageCode, &str)] = &[
    (
        MessageCode::ChartLimit,
        "chart exceeds the limit of 10000 points or 100 series",
    ),
    (
        MessageCode::ChartLabelReview,
        "slide {slide}, element {element}: dense or long chart labels may overlap or be clipped; exact values remain in the editable caches and workbook",
    ),
    (
        MessageCode::XmlDepthLimit,
        "XML nesting exceeds the limit of {limit} elements",
    ),
    (
        MessageCode::XmlCharacter,
        "field {field} contains a character forbidden by XML 1.0; remove invalid control characters",
    ),
    (
        MessageCode::CreateSourceConflict,
        "create requires exactly one of markdown or slides; remove one source",
    ),
    (
        MessageCode::PdfTimeout,
        "local renderer {program} exceeded {seconds} seconds and was terminated",
    ),
    (
        MessageCode::CliArgumentEncoding,
        "CLI arguments must be valid UTF-8",
    ),
    (
        MessageCode::CliArguments,
        "provide one JSON argument or JSON on stdin; use --help or --schema",
    ),
    (
        MessageCode::PathSymlink,
        "symlink in path {path}; use a regular path inside the working directory",
    ),
    (
        MessageCode::FileNotRegular,
        "input {path} must be a regular file",
    ),
    (
        MessageCode::ReadTruncated,
        "read representation {representation} was truncated; adjust max_chars, offset or limit",
    ),
    (
        MessageCode::ReadCompatibilityUnsupported,
        "slide {slide}: unsupported shape or compatibility content was not imported; review the original presentation",
    ),
    (
        MessageCode::ReadChartUnsupported,
        "slide {slide}: unsupported chart type {type} was detected; extraction may be incomplete",
    ),
    (
        MessageCode::ReadChartIncomplete,
        "chart cache has missing or inconsistent indexed category/X and value/Y points; dense extraction is not possible",
    ),
    (
        MessageCode::ReadChartCombination,
        "slide {slide}: combination chart extracted as {groups} separate chart blocks; all cached series are retained, combined styling is not represented",
    ),
    (
        MessageCode::CheckUnclassifiedNumbers,
        "slide {slide}: {count} unclassified numbers ({sources}); no cross-slide identity was guessed",
    ),
    (
        MessageCode::ResponseTruncated,
        "long response values were truncated to {limit} characters",
    ),
    (
        MessageCode::EditNoMatch,
        "replacement found no matching paragraph text",
    ),
    (
        MessageCode::EditTextFitted,
        "slide {slide}: edited text required a smaller font and autofit; review the field before presenting",
    ),
    (
        MessageCode::EditTextOverflow,
        "slide {slide}: edited text cannot fit at the minimum font size; {hidden_words} word(s) may be hidden (conservative estimate); full text is retained; review the field before presenting",
    ),
    (
        MessageCode::TemplateContentReview,
        "Company placeholders use conservative text fitting and pagination; visually review the exported slides with the installed company fonts.",
    ),
    (
        MessageCode::TemplatePlaceholderFallback,
        "slide {slide}: no matching company placeholder for some content; using its scaled position",
    ),
    (
        MessageCode::SourceOverflow,
        "slide {slide}: source footnotes may exceed the footer area; shorten the citations",
    ),
    (
        MessageCode::CheckDone,
        "checked presentation: {count} finding(s)",
    ),
    (
        MessageCode::CheckTolerance,
        "tolerance must be finite and nonnegative",
    ),
    (
        MessageCode::CheckScope,
        "Checks use explicit metric labels, table totals and percentage columns; they do not infer financial meaning or read numbers embedded in images. Model formulas require cached numeric results.",
    ),
    (
        MessageCode::ModelInvalid,
        "invalid XLSX model or nonnumeric value: {part}",
    ),
    (
        MessageCode::ModelUnlinked,
        "slide {slide}: no numeric value could be linked to the model",
    ),
    (MessageCode::ModelMismatch, "value differs from the model"),
    (
        MessageCode::MetricMismatch,
        "the same metric has a different value on another slide",
    ),
    (
        MessageCode::TableTotal,
        "table total does not equal its components",
    ),
    (
        MessageCode::TablePercent,
        "table percentages do not add up to 100%",
    ),
    (
        MessageCode::SourceUnresolved,
        "source reference cannot be resolved in the model",
    ),
    (
        MessageCode::TemplateOutput,
        "template is supported only for create PPTX and Markdown to PPTX conversion",
    ),
    (
        MessageCode::TemplateFormat,
        "template must be a PPTX or POTX file",
    ),
    (
        MessageCode::TemplateFallback,
        "slide {slide}: layout {requested} not found; using {selected}",
    ),
    (
        MessageCode::PackageInvalid,
        "invalid Office package part: {part}",
    ),
    (
        MessageCode::CreateDone,
        "created {count} slide(s) as {format}",
    ),
    (MessageCode::ReadDone, "{count} slide(s)"),
    (MessageCode::EditDone, "edited presentation"),
    (
        MessageCode::ConvertDone,
        "converted {from} to {to} ({count} slide(s))",
    ),
    (
        MessageCode::SlideTextOverflow,
        "slide {slide}: text does not fit even at 14 pt",
    ),
    (
        MessageCode::SlideSplit,
        "slide {slide}: text does not fit at the minimum font size; split into {count} slides",
    ),
    (
        MessageCode::ChartScatterData,
        "a scatter chart needs finite numeric labels for its X coordinates",
    ),
    (
        MessageCode::SlideCannotFit,
        "slide {slide}: content cannot fit without losing data",
    ),
    (
        MessageCode::SlideTitleOverflow,
        "slide {slide}: title exceeds its available area",
    ),
    (
        MessageCode::ImageUnsafePath,
        "slide {slide}: image {path} has an unsafe path",
    ),
    (
        MessageCode::ImageUnreadable,
        "slide {slide}: cannot read image {path}: {detail}",
    ),
    (
        MessageCode::ImageUnsupported,
        "slide {slide}: image {path} is not PNG or JPEG",
    ),
    (
        MessageCode::ImageNotFound,
        "slide {slide}: image {path} not found",
    ),
    (
        MessageCode::ImageNoSize,
        "slide {slide}: image {path} has no size",
    ),
    (
        MessageCode::ImageNotEmbedded,
        "slide {slide}: image {path} not embedded ({detail})",
    ),
    (MessageCode::FontWarning, "font: {detail}"),
    (
        MessageCode::EditSlideMissing,
        "slide {index} does not exist",
    ),
    (
        MessageCode::EditFieldMissing,
        "slide {index}: field {field} has no matching placeholder; edit was not applied",
    ),
    (MessageCode::InputBadRequest, "bad request: {detail}"),
    (
        MessageCode::UnknownTheme,
        "unknown theme {theme}; expected light, dark, corporate or creative",
    ),
    (MessageCode::UnknownLayout, "unknown layout {layout}"),
    (
        MessageCode::UnknownChartType,
        "unknown chart type {kind}; expected bar, column, line, pie, area or scatter",
    ),
    (
        MessageCode::CreateNoSource,
        "create needs `markdown` or `slides`",
    ),
    (MessageCode::NoSlides, "the presentation has no slides"),
    (
        MessageCode::MarkdownNotUtf8,
        "markdown is not UTF-8: {detail}",
    ),
    (
        MessageCode::ChartEmpty,
        "a chart needs at least one label and one series",
    ),
    (
        MessageCode::ChartInvalidData,
        "the chart contains invalid cached values",
    ),
    (
        MessageCode::ChartPieData,
        "a pie chart needs exactly one nonnegative series with a finite positive total",
    ),
    (
        MessageCode::ChartSeriesLength,
        "chart series {series} has {values} value(s) for {labels} label(s)",
    ),
    (
        MessageCode::ChartValueNotFinite,
        "chart series {series} has a non-finite value",
    ),
    (
        MessageCode::UnsafePath,
        "unsafe path {path}: paths must be relative and stay inside the base directory",
    ),
    (
        MessageCode::TooLarge,
        "input {path} is {size} bytes, over the {limit} byte limit",
    ),
    (MessageCode::FileRead, "cannot read {path}"),
    (MessageCode::FileWrite, "cannot write {path}"),
    (MessageCode::ZipInvalid, "invalid zip package: {detail}"),
    (MessageCode::XmlInvalid, "invalid XML: {detail}"),
    (
        MessageCode::XmlMultipleRoots,
        "invalid XML: multiple root elements",
    ),
    (MessageCode::XmlEmpty, "invalid XML: empty document"),
    (
        MessageCode::PptxMissingPresentation,
        "invalid PPTX: missing ppt/presentation.xml",
    ),
    (
        MessageCode::PptxNoShapeTree,
        "invalid PPTX: slide has no shape tree",
    ),
    (
        MessageCode::PptxBadRelationships,
        "invalid PPTX: {part} is not a relationships part",
    ),
    (
        MessageCode::PptxNoLayout,
        "invalid PPTX: the package has no slide layout",
    ),
    (
        MessageCode::PptxNoSlideList,
        "invalid PPTX: cannot add slide list",
    ),
    (
        MessageCode::PdfIo,
        "cannot run the local PDF renderer or access its temporary files; ensure LibreOffice and Fontconfig are installed",
    ),
    (
        MessageCode::PdfRender,
        "LibreOffice could not render this presentation as PDF",
    ),
    (
        MessageCode::PdfFontRead,
        "cannot read font {path}: {detail}",
    ),
    (
        MessageCode::PdfFontMissing,
        "no font found; set Options.font_path to a TTF",
    ),
    (MessageCode::PdfFontParse, "cannot parse the font"),
    (
        MessageCode::UnsupportedCreateOutput,
        "unsupported file type for {path} (use .pptx or .pdf)",
    ),
    (
        MessageCode::UnsupportedReadInput,
        "unsupported file type for {path} (read needs a .pptx)",
    ),
    (
        MessageCode::UnsupportedEditInput,
        "unsupported file type for {path} (edit needs a .pptx)",
    ),
    (
        MessageCode::ConvertSameFile,
        "conversion input and output must be different files; choose a new output file",
    ),
    (
        MessageCode::ConvertUnsupportedPair,
        "unsupported conversion {from} to {to}; use pptx to pdf/md or md to pptx/pdf",
    ),
    (
        MessageCode::UnsupportedConvertInput,
        "unsupported file type for {path} (convert reads .pptx or .md)",
    ),
    (
        MessageCode::UnsupportedConvertOutput,
        "unsupported file type for {path} (convert writes .pptx, .pdf or .md)",
    ),
    (
        MessageCode::CliHelp,
        "Usage: slides '<json>' | slides < request.json\n       slides --schema\n       slides docs --messages\nCreate, read, edit and convert presentations. PDF output requires local LibreOffice; direct PDF creation also uses Fontconfig.",
    ),
    (
        MessageCode::CliCurrentDir,
        "cannot access the current directory: {detail}",
    ),
    (
        MessageCode::CliSchemaRender,
        "cannot render the schema: {detail}",
    ),
    (
        MessageCode::CliStdinRead,
        "cannot read the request from stdin: {detail}",
    ),
    (
        MessageCode::CliInvalidJson,
        "invalid JSON request: {detail}",
    ),
    (
        MessageCode::CliResultRender,
        "cannot render the result: {detail}",
    ),
];

const LOCALES: &[(&str, &[(MessageCode, &str)])] = &[("en", EN)];

/// Machine-readable documentation derived from the same catalogue as runtime messages.
pub fn catalogue() -> serde_json::Value {
    let messages: Vec<_> = MessageCode::ALL
        .iter()
        .map(|code| {
            let text = template(*code, "en");
            let mut params = Vec::new();
            let mut rest = text;
            while let Some(open) = rest.find('{') {
                let after = &rest[open + 1..];
                let Some(close) = after.find('}') else {
                    break;
                };
                params.push(after[..close].to_owned());
                rest = &after[close + 1..];
            }
            let kind = match code {
                MessageCode::CreateDone
                | MessageCode::ReadDone
                | MessageCode::EditDone
                | MessageCode::ConvertDone
                | MessageCode::CheckDone => "summary",
                MessageCode::ChartLabelReview
                | MessageCode::SlideTextOverflow
                | MessageCode::SlideTitleOverflow
                | MessageCode::SlideSplit
                | MessageCode::ImageUnsafePath
                | MessageCode::ImageUnreadable
                | MessageCode::ImageUnsupported
                | MessageCode::ImageNotFound
                | MessageCode::ImageNoSize
                | MessageCode::ImageNotEmbedded
                | MessageCode::FontWarning
                | MessageCode::EditSlideMissing
                | MessageCode::EditFieldMissing
                | MessageCode::EditTextOverflow
                | MessageCode::EditNoMatch
                | MessageCode::EditTextFitted
                | MessageCode::TemplateContentReview
                | MessageCode::TemplatePlaceholderFallback
                | MessageCode::TemplateFallback
                | MessageCode::SourceOverflow
                | MessageCode::CheckScope
                | MessageCode::ModelUnlinked
                | MessageCode::CheckUnclassifiedNumbers
                | MessageCode::ReadTruncated
                | MessageCode::ReadCompatibilityUnsupported
                | MessageCode::ReadChartUnsupported
                | MessageCode::ReadChartCombination
                | MessageCode::ResponseTruncated => "warning",
                MessageCode::MetricMismatch
                | MessageCode::ModelMismatch
                | MessageCode::TableTotal
                | MessageCode::TablePercent
                | MessageCode::SourceUnresolved => "finding",
                MessageCode::CliHelp => "help",
                _ => "error",
            };
            serde_json::json!({"code":code.as_str(),"kind":kind,"template":text,"params":params})
        })
        .collect();
    serde_json::json!({"tool":crate::NAME,"version":env!("CARGO_PKG_VERSION"),"messages":messages})
}

fn template(code: MessageCode, locale: &str) -> &'static str {
    let table = LOCALES
        .iter()
        .find(|(name, _)| *name == locale)
        .map(|(_, table)| *table)
        .unwrap_or(EN);
    table
        .iter()
        .find(|(key, _)| *key == code)
        .map(|(_, text)| *text)
        .unwrap_or("")
}

/// Render a message for `locale`. Unknown locales fall back to English. A placeholder whose
/// parameter is missing is left in the text, so a programming error stays visible.
pub fn render(code: MessageCode, params: &Params, locale: &str) -> String {
    let mut rest = template(code, locale);
    let mut out = String::new();
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else {
            break;
        };
        let name = &after[..close];
        if let Some((_, value)) = params.iter().find(|(key, _)| key == name) {
            out.push_str(value);
        } else {
            out.push_str(&rest[open..open + close + 2]);
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placeholders(template: &str) -> Vec<String> {
        let mut names = Vec::new();
        let mut rest = template;
        while let Some(open) = rest.find('{') {
            let after = &rest[open + 1..];
            match after.find('}') {
                Some(close) => {
                    names.push(after[..close].to_string());
                    rest = &after[close + 1..];
                }
                None => break,
            }
        }
        names
    }

    #[test]
    fn parameter_values_are_not_interpreted_as_templates() {
        let values = params(&[
            ("slide", "1"),
            ("path", "{detail}.png"),
            ("detail", "invalid image"),
        ]);
        let text = render(MessageCode::ImageUnreadable, &values, "en");
        assert!(text.contains("{detail}.png"));
        assert!(text.ends_with("invalid image"));
    }

    /// Every code has an English template and renders fully once its parameters are supplied.
    #[test]
    fn every_code_renders_with_its_template_params() {
        for code in MessageCode::ALL {
            let template = template(*code, "en");
            assert!(!template.is_empty(), "{} has no template", code.as_str());
            let params: Params = placeholders(template)
                .iter()
                .map(|name| (name.clone(), "x".to_string()))
                .collect();
            let rendered = render(*code, &params, "en");
            assert!(
                !rendered.contains('{'),
                "{} leaves a placeholder: {rendered}",
                code.as_str()
            );
            // Unknown locales fall back to English.
            assert_eq!(rendered, render(*code, &params, "zz"));
        }
    }
}
