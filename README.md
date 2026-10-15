# slides

A small tool for presentations. An AI agent sends a short JSON request, and `slides` does the work
deterministically and returns a short JSON result.

It can:

- **create** a PPTX or PDF from Markdown or a JSON slide list,
- **read** a PPTX (titles, text, notes, tables),
- **edit** an existing PPTX (replace text, change slides, delete slides, add slides),
- **convert** between PPTX, PDF and Markdown,
- **check** numeric consistency, optionally against an XLSX model.

PPTX is written directly as an Office Open XML package, so it opens in PowerPoint and LibreOffice.
PDF is rendered locally by **LibreOffice (`soffice`)** in an isolated temporary profile. Both
exports use the same slide layout and native chart renderer. PPTX → PDF renders the original
package, so it keeps its appearance instead of reconstructing a text-only deck. PDF creation also
requires Fontconfig (`fc-scan`). No remote services or accounts are used. PNG/JPEG decoding and font
validation use `printpdf` with its `png` and `jpeg` features enabled.

## Acceptance scenarios

The supported authoring contract is the eight-slide quarterly business review exercised in
`tests/acceptance.rs`: title, two-level agenda, image/caption, regional quarterly revenue table with
totals, editable bar and line charts, two-column comparison, and closing speaker notes.

| Scenario | Contract |
| --- | --- |
| S1 | JSON `create.slides` → PPTX: all content and exact table/chart values, native editable charts, proportional images, bounded text. |
| S2 | The same review in Markdown → PPTX, including fenced `chart` data blocks. |
| S3 | S1 PPTX → PDF: eight pages with every title and number in the text layer. |
| S4 | S1 with a company PPTX template: content retained and company masters, fonts and colors used. |

Tests inspect XML and embedded workbooks, render through LibreOffice to PNG, and check PDF text
bounds. Seeded invariant generators also exercise 200 numeric decks through PPTX and PDF, repeated
byte/JSON determinism, and malformed requests with deadlines. Failures print their seed. Determinism
means identical requests **and input-file bytes**, with the same renderer, fonts and version. PDF
export canonicalizes the Info/XMP dates and document IDs that the renderer generates, without
changing content streams or cross-reference offsets. Creation dates are intentionally not wall-clock
times.

## Operations

| `op` | Fields | What it does |
| --- | --- | --- |
| `create` | `output`, `markdown` **or** `slides`, `theme?`, `template?` | Write a new `.pptx` or `.pdf`. |
| `read` | `input`, `max_chars?` (default 20000), `representation?`, `offset?`, `limit?`, `layouts?` | Read grouped or ungrouped text, tables, charts and images. Choose `both` (default), `text`, or `data`, and paginate slides with zero-based `offset` and `limit`. `layouts: true` also lists company layouts (PPTX/POTX). |
| `edit` | `input`, `output`, `replace?`, `slides?`, `delete?`, `add?` | Change an existing `.pptx`. Parts the tool does not understand are copied byte for byte. |
| `convert` | `input`, `output`, `template?` | `pptx → pdf`, `pptx → md`, `md → pptx`, `md → pdf`. Templates apply only to `md → pptx`. |
| `check` | `input` (PPTX), `model?` (XLSX), `tolerance?` | Return findings in `data`, using one-based slide numbers. Does not modify either file. |

`.markdown` is an alias for `.md`. Other conversion pairs (including PPTX → PPTX and MD → MD) fail
with `convert.unsupported_pair`. Conversion never overwrites its input: identical paths and
hard-link aliases (device + inode on Unix) fail with `convert.same_file` before anything is written.

Markdown mapping: `# ` is a title slide (its first paragraph is the subtitle), `## ` starts a new
slide, and a standalone `---` separates slides. Lists become bullets, Markdown tables become native
tables, and `![](image.png)` inserts a proportionally scaled PNG/JPEG. Nested list levels are kept.
CommonMark inline text is concatenated exactly across emphasis, code and link boundaries, including
formatting inside words and numbers. Only block and line boundaries add separators. Fenced code
blocks keep their indentation and use a monospace font. `> ` is a speaker note, except in a `quote`
layout, where it is visible quotation text.

Layouts: `title`, `title_content`, `two_content`, `section`, `image`, `table`, `quote`, `chart`. To
select a layout, put `<!-- layout: two_content -->` immediately after a slide heading. Two-column
slides distribute content blocks across the columns, and a single bullet list is split in half.
Image layouts put the image above its caption. Quote slides use `text` for the quotation and `title`
for the attribution. JSON title and section slides accept `subtitle`.

Themes: `light` (blue/white), `dark` (charcoal/electric blue), `corporate` (white/navy), and
`creative` (warm ivory/plum with coral and teal accents). Title slides reserve at least two lines at
40–44 pt, including explicit JSON title line breaks. Title and section slides use a large centered
heading, while content slides have 34 pt titles, 24 pt body text, an accent bar and slide numbers.
Ordinary content starts at a fixed gap below the title, including short lists, small tables and
charts. It is not vertically centered. Slides containing only 1–3 bullets automatically get a tinted
theme panel and accent rail. To disable this deterministic treatment, set
`"short_slide_accent": false` on a JSON slide. Dense lists and mixed-content slides keep the plain
grid. For example:

```json
{"op":"create","output":"plan.pptx","slides":[{"title":"Priorities","short_slide_accent":false,"bullets":["Ship the release","Measure adoption"]}]}
```

Dense text first shrinks to 14 pt. Content that still doesn't fit is split onto continuation slides
marked `(cont.)`, with a `slide.split` warning. This applies to text, lists, tables and code in
newly generated slides, including `edit` additions. Added slides use the existing presentation as a
company template: actual slide size, selected layouts/placeholders, inherited fonts and logos,
without adding the default accent panel. Their `layout` may also be a company layout name. Existing
slides are kept, and the template review and fallback warnings described below apply to additions
too. Table headers repeat on continuations, and speaker notes stay on the first fragment. The
summary counts the slides actually written. Tables have colored headers and alternating rows.
Conservative wrapping measurements reserve room for local font substitutions. Visual soft wrapping
is left to Office, and only source line breaks are serialized as semantic breaks. If content can't
be split safely (for example, hundreds of table columns), `slide.cannot_fit` is returned instead of
writing a clipped file.

Editing existing foreign shapes keeps their geometry instead of rebuilding them. Both `edit.replace`
and `edit.slides` refit only changed text bodies, resolving placeholder dimensions from the
layout/master when necessary. Font and paragraph/run properties (including inherited placeholder
styles) stay unchanged when the new text fits, and setting identical text leaves the original slide
part untouched. Only actual estimated overflow triggers smaller type, down to 14 pt (text that is
already smaller is not enlarged). A successful reduction uses `normAutofit` and emits
`edit.text_fitted`. If the text still can't fit, `edit.text_overflow` reports the one-based output
slide number and `hidden_words`, a conservative estimate of potentially hidden words. The full text
remains in the package, and text shapes request clipping to the original field. In that case
`noAutofit` freezes the minimum font, because LibreOffice otherwise recomputes `normAutofit` and
shrinks below the minimum. Split the text manually or enlarge the field in an Office editor.
Unresolved geometry or unsupported vertical/rotated text bodies warn conservatively for all words
rather than claiming success. Estimates depend on fonts and are not exact renderer counts.

Table cells are fitted without changing stored row heights. LibreOffice can still expand an
overflowing row during rendering and let it extend beyond the slide. Such cells always warn. Shorten
or split them manually before presenting. Notes are not treated as visible slide text. Untouched
package parts are copied byte for byte.

Reading and replacement share the same ordered paragraph text: formatted runs, field display text,
literal tabs and `a:br` line breaks (`\n`). Replacements can cross inline boundaries and use the
style of the first inline they touch, while surrounding text keeps its own style. `edit.no_match`
reports a replacement with no matches. `edit.field_missing` reports an unapplied `title` or
`bullets` edit with its zero-based `index` and `field` when the slide has no matching placeholder.
It never guesses that an arbitrary text box is a title.

Markdown export writes body line breaks as hard breaks and table-cell breaks as `<br>` instead of
gluing words together. Export escapes active CommonMark syntax in literal titles, body text and
cells, and the parser reads exported `<br>` breaks. Code extraction keeps blank paragraphs,
including leading and trailing empty lines. JSON slides also accept `code` (a literal string),
including in `edit.add`. Generated code, tables and charts carry private OOXML block metadata that
doesn't depend on mutable shape or placeholder names. Company composition and additions preserve it,
and code keeps a monospace font even in company placeholders. Markdown is a content round trip, not
a promise to keep the original slide geometry or inline styling.

PPTX → Markdown writes embedded images beside the document in `<output-stem>_media/`, using
generated unique filenames and relative links rather than internal ZIP paths. Copy the `.md` and its
media directory together. Markdown image links resolve relative to the input document, and importing
an exported document preserves PNG/JPEG illustration bytes. Companion files use the same checked,
atomic writer as other outputs. URI-escaped path delimiters are decoded before path validation.
Unsafe links are rejected, not fetched.

Reading visits nested shape groups and OOXML `mc:AlternateContent` in document order. It selects the
first Choice whose required namespaces are supported (core PresentationML, DrawingML and charts),
otherwise Fallback, never both. Missing usable branches or unsupported shape objects emit
`read.compatibility_unsupported`, and conversion forwards these warnings.

Slide deletion resolves relative and absolute part addresses, removes incoming slide links,
custom-show entries and notes, and prunes unreachable parts and content-type overrides. Before the
atomic write, final edited packages are checked for missing relationship targets and slide-list
references. Malformed packages fail rather than succeeding with missing slides.

Charts are native, editable PowerPoint objects with cached values and an embedded XLSX workbook:

```json
{"op":"create","output":"results.pptx","slides":[{"title":"Revenue","chart":{"type":"bar","labels":["Q1","Q2","Q3"],"series":[{"name":"2027","values":[24,38,47]}]}}]}
```

Supported chart types are `bar` (horizontal), `column` (vertical), `line`, `pie`, `area` and
`scatter`. Each series must match the labels. For scatter plots, labels are finite numeric strings
representing the shared X coordinates (for example `["1.5", "3", "7"]`), and each series supplies Y
values. A pie chart requires exactly one nonnegative series with a finite positive total. Bare
numeric arrays are also accepted as series. Charts render as vector marks in PDF and as data tables
in Markdown. Generated charts include literal value labels, so PDF exports don't silently round
their source values. Markdown authoring can include a native chart:

````markdown
## Revenue trend
```chart
{"type":"line","labels":["Q1","Q2"],"series":[{"name":"Revenue","values":[24.125,38.5]}]}
```
````

A `chart` fence must contain one valid chart object. Invalid chart JSON fails rather than becoming
an ordinary code block, and other fenced languages remain literal code. Charts are limited to 10,000
points per series and 100 series (`chart.limit`). Earlier releases drew `bar` vertically. Use
`column` for that orientation. Reading an existing chart preserves the orientation encoded in its
XML.

Read, Markdown export and numeric QC use the same indexed cache reader for categories/X and
values/Y, not XML point order. Missing category/value pairs or inconsistent series axes fail
read/export with `read.chart_incomplete` rather than inventing labels or zeroes. QC keeps bare
values but does not guess their cross-slide identity. Unsupported chart types (for example
`doughnutChart`) emit `read.chart_unsupported` with one-based `slide` and `type` parameters in read,
convert and check. Combination charts keep **every supported group**, in XML order, as separate
chart blocks/data tables. `read.chart_combination` explicitly reports this loss of combined styling,
not missing series. Numeric QC reads all groups directly from the same indexed caches. Conversion to
PDF still renders the original chart.

## Company templates and source citations

```json
{"op":"read","input":"company.potx","layouts":true}
```

```json
{"op":"create","template":"company.potx","output":"board.pptx","slides":[{"layout":"title_content","title":"Performance","text":"Revenue: $25 mln","sources":["Source: model.xlsx, DCF!C25"]}]}
```

A template can be PPTX or POTX. Its masters, layouts, themes, logo assets and slide size are
preserved, and its sample slides and their notes are removed. `layout` can be a company layout name
or a standard type (`title`, `title_content`, `two_content`, `section`, `blank`). Duplicate company
names are disambiguated by the inferred slide type. Missing layouts select the nearest available one
with a warning. Text uses company placeholders and inherited or explicit layout fonts and colors.
Content is top-aligned unless the company placeholder explicitly sets an anchor in its layout or
master, and that company setting takes precedence. Tables and charts start at the same placeholder
top as text. Objects use placeholder geometry, or scaled positions with a warning when it's
unavailable. Images are contained proportionally in the destination area, and image layouts reserve
space below for captions before fitting the picture. Template rendering fits text down to 14 pt and
paginates using the selected company placeholders, including narrower body fields. Native tables are
sized to their destination frame. Review the exported PDF with the intended company fonts. `theme`
controls non-template exports, and the company template takes precedence. Native chart fills, text
and series use scheme colors and theme fonts inherited from the selected company layout/master,
rather than the literal palette of the requested non-template theme. Templates are intentionally
rejected for direct PDF creation: create a PPTX first, then convert it.

`sources` is a list of visible 9 pt citations at the bottom, using the theme's minor font and text
color (or explicit company footer styling when present). It is separate from speaker notes and is
returned by `read`. Citations reserve space before pagination, reducing the available content area
when necessary, including in company placeholders. Each continuation repeats the citations, while
speaker notes remain on the first fragment. Excessive citations that leave no usable content area
return `slide.cannot_fit`. Markdown supports `<!-- source: Source: model.xlsx, DCF!C25 -->` on the
current slide, and repeated comments add citations. Company layout names can also be selected with a
`<!-- layout: Company layout name -->` comment immediately after the heading.

## Number checks

```json
{"op":"check","input":"board.pptx","model":"model.xlsx","tolerance":0.000001}
```

`data` is an array of `{slide, text, value, issue, expected?}`. `issue` is a stable code:
`check.metric_mismatch`, `check.table_total`, `check.table_percent`, `check.model_mismatch`, or
`check.source_unresolved`. Values are normalized: `25%` is `0.25`, `4.2x` is `4.2`, `$25 mln` is
`25000000`. Currency symbols and codes, English thousands separators, `k`, `m/mm/mn/mln`, `bn/b`,
negative parentheses and scientific notation are supported. Table column units such as `Revenue
($m)` and `Share (%)` apply to otherwise bare numbers. `tolerance` is an **absolute**
difference in normalized units, default `0.000001`, and it must be finite and nonnegative.
`summary_params.numbers` reports the number of extracted numeric occurrences.

Checks inspect slide text (including grouped text), native table cells and chart caches/custom
labels, not images, speaker notes, slide numbers or source-reference addresses. Cross-slide matching
uses explicit text/row/column/series labels and preserves units and labeled periods. It does not
equate unlabeled figures or perform currency conversion. All three sources produce facts with the
same normalized label, period/segment/unit dimensions, value, source and slide. For example,
`Revenue: 25` and `Metric | Value` / `Revenue | 26` identify the same metric, and generic
`Value`/`Amount` headers do not create separate metrics. A table's first column is a label axis only
when its contents are nonnumeric labels (or explicitly labeled numeric periods). Otherwise its
numeric cells are measures too, including units inherited from column headers. Multiple numeric rows
without a label axis, multiple values in one cell, missing chart categories and other unclassified
figures remain in `summary_params.numbers`. `check.unclassified_numbers` reports their `count`,
`slide` and `sources` instead of guessing cross-slide matches.

Numeric period labels before a value separator (`:`, `=`, `–`, or a tab) are dimensions in the
metric key, not extracted measures. Examples are `Revenue 2027 = $25 mln`,
`Revenue Q1 2027: $25 mln`, month names or dates. Chart values are joined to category labels by
cache `idx`, not cache order, and the complete series and category/period label are part of the
metric identity. Scatter values join X/Y caches by `idx`, using the series name and finite numeric X
coordinate (so `1` and `1.0` identify the same X). Missing categories or X coordinates are not
guessed, and numeric category labels themselves are dimensions rather than contradictory
measurements.

Table totals use rows or columns labeled `Total`, `Sum`, `Suma` or `Grand total`, regardless of
position. Both axes select components from headers, excluding every labeled total. Arithmetic
requires matching units/currencies for all components and totals, and no exchange rate is assumed.
Percentage partitions are checked against 100%, excluding labeled total rows and columns. Column
period headers select vertical partitions, and row period labels select horizontal partitions.
Without periods, a unique `Share`/`Percent`/`%` column header selects that column. Conflicting or
ambiguous axes are not guessed or checked on both axes. Numeric table values and chart value caches
are always measures, even in the 1900–2200 range. Period classification applies only to labels, not
raw values. Line breaks, tabs and cell paragraph boundaries separate numeric tokens. Generated
tables keep a logical ID and row offsets in OOXML extension metadata, so totals and percentage
columns are checked over the whole table, not individual continuation slides. Repeated headers are
counted once for table sums. Foreign tables without this metadata are checked independently.
Subtotals and financial ratios need human interpretation.

Model linking uses source citations `Source: model.xlsx, DCF!C25` (quoted sheet names and `$`
addresses are accepted) or workbook defined names, including rectangular named ranges. References
are separated by commas or semicolons outside quoted sheet names, and doubled apostrophes escape an
apostrophe (e.g. `'O''Brien, US'!B2`). The same tokenizer handles rectangular ranges and
defined-name unions. Every missing reference or cell yields its own unresolved finding, even beside
valid cells or inside a partially resolved defined name. Workbook filename tokens are citation
metadata, not references. Cells are matched by their row label, and a single reference with a single
slide number can also be compared directly. Without citations, unique model row labels and defined
names are matched to text before a number, e.g. `Revenue: $25 mln`. Ambiguous links are not guessed.
`check.model_unlinked` warns when no number on a slide could be compared.

Numeric XLSX cells and cached formula values are read, but formulas are **not evaluated**.
Unresolved references produce findings. Sheet names keep spaces and punctuation (`DCF`, `D CF`, and
`D-CF` are distinct), and cell addresses and defined names have separate identities. Findings longer
than 512 characters have `text_truncated: true` and a `response.truncated` warning. Missing caches
cannot establish agreement with the model. A check always emits `check.scope` to make these
limitations explicit: no findings is not a financial audit.

## Example

Request:

```json
{
  "op": "create",
  "output": "talk.pptx",
  "theme": "corporate",
  "markdown": "# Roadmap\n\n## This quarter\n- Ship v1\n- Write docs\n\n> Mention the demo\n"
}
```

Result:

```json
{
  "ok": true,
  "output": "talk.pptx",
  "bytes": 17364,
  "summary": "created 2 slide(s) as pptx",
  "summary_code": "create.done",
  "summary_params": { "count": "2", "format": "pptx" },
  "warnings": []
}
```

Reading it back:

```json
{ "op": "read", "input": "talk.pptx" }
```

```json
{
  "ok": true,
  "summary": "2 slide(s)",
  "summary_code": "read.done",
  "summary_params": { "count": "2" },
  "text": "# Roadmap\n\n## This quarter\n- Ship v1\n- Write docs\n> Mention the demo\n\n",
  "data": [
    { "index": 0, "layout": "title", "title": "Roadmap", "text": "", "notes": "" },
    { "index": 1, "layout": "title_content", "title": "This quarter", "text": "Ship v1\nWrite docs", "notes": "Mention the demo" }
  ],
  "warnings": []
}
```

## CLI

```sh
slides '{"op":"read","input":"talk.pptx"}'
slides < request.json
slides --schema
```

The result is written to stdout as JSON. The exit code is `0` on success and `1` on error. Every
error uses the flat envelope `{ok:false,summary,summary_code,summary_params,warnings:[]}` on stdout,
including malformed JSON, unknown fields and non-UTF-8 arguments. There is no nested `error` object.
Unknown or mistyped fields use a JSON Pointer in `summary_params.field`, e.g.
`/slides/0/chart/series/0/unknown`, that selects the actual source branch rather than an unused
Markdown alternative. Missing fields also use JSON Pointers (e.g. `/slides/0/table/rows`), and the
`pointer` alias is kept on parser errors. Enum errors supply `allowed` as an array, not a display
string. Malformed JSON includes `line` and `column`. Long message parameters are limited to 256
characters with `truncated_fields` metadata. Arguments and stdin are bounded at 100 MB before
parsing. All paths in the request are relative to the current directory. Absolute paths and `..` are
rejected. Create requires exactly one non-null source, `markdown` or `slides`.

`read.max_chars` bounds the Markdown text and, separately, the serialized slide records in `data`.
Data records are returned whole, never with silently omitted fields. A record larger than the budget
requires a larger `max_chars`. JSON envelope and metadata overhead is not counted. A zero budget
returns empty content. `read.truncated` warns about omitted content, and `truncation` reports
text/data/layout truncation, total slides, offset, page size, returned data records and
`next_offset`. For text-only reads the cursor advances by the slide page size, independently of text
character truncation, and `truncation.text` still reports clipped text. With `data` or `both`, it
advances by the number of complete returned data records (increase `max_chars` if none fit). A null
cursor means the end of the presentation, not that every representation was untruncated. Pagination
applies to both representations. To avoid duplicating content, use:

```json
{"op":"read","input":"talk.pptx","representation":"data","offset":0,"limit":10,"max_chars":12000}
```

## Integration

- `slides --messages` (also `slides docs --messages`) returns
  `{tool,version,messages:[{code,kind,template,params}]}` directly from the runtime message
  catalogue. `kind` distinguishes summaries, warnings, errors, findings and help, and `params` lists
  each template's required parameter names.
- `slides --schema` returns a form-friendly schema derived from documented Rust request types. The
  only union is top-level `oneOf`, discriminated by `op`. Nested definitions are inlined, and
  optional properties use their non-null canonical form. Named chart series are advertised, while
  legacy bare numeric arrays remain accepted by the parser.
- Every property and operation variant has a title and description. File fields, including images in
  slide/add arrays, carry `x-file: input` with `x-accept`, or `x-file: output` with `x-formats`
  (preferred format first). Format lists contain filename extensions without leading dots.
- Exactly one create source and cross-field rules are checked at runtime, not through nested schema
  conditionals. Validation failures use RFC 6901 `summary_params.field`, where the empty string
  denotes the complete malformed request. I/O failures keep their underlying cause in the library.
- Argument and stdin requests use the same parser and runner and return identical results. Success
  and error envelopes always include `warnings`, even when empty. CLI errors are JSON on stdout with
  exit status 1. Responses and diagnostic parameters are bounded.

## Library

```rust
use slides::{run, parse_input, Options};

let input = parse_input(r#"{"op":"create","output":"out.pptx","markdown":"# Hi"}"#)?;
let options = Options::new(".");
let output = run(&input, &options)?;
println!("{}", output.summary);
```

`Options` carries `base_dir`, `max_input_bytes` (default 100 MB), optional `font_path`, `locale`
(default `en`, and unknown locales fall back to English), and `render_timeout` (default 120 seconds
per `soffice`/`fc-scan` command). Expired processes are killed and reaped. Unix launchers use an
isolated process group, terminated with `/bin/kill`, and return `pdf.timeout`. Direct PDF creation
uses `font_path`, or the first available Noto Sans/DejaVu Sans fallback. The selected font is made
available only to that renderer process. PPTX → PDF keeps the original document's fonts and system
substitutions unless `font_path` explicitly requests a replacement. Generated code blocks keep their
monospace font during font substitution.

Every input, output and image path is relative to `base_dir`. Absolute paths, `..` and all symlinks
(including dangling links and internal links) are rejected. Inputs must be regular files, checked
before opening and again on the descriptor. Linux uses nonblocking, no-follow opens and anchored
parent directory descriptors opened component by component with `openat`. Temporary files are
created through a pinned procfs descriptor and committed with `renameat` against that same directory
descriptor, so replacing a parent pathname during a long write cannot redirect it.

The size limit covers inline Markdown/JSON slides, assets and file reads, including bounded reads if
a file grows. All OOXML consumers share limits of 50,000 entries, `max_input_bytes` inflated bytes
and a maximum per-entry compression ratio of 1000:1. XML parts, including ancillary parts, are
limited to 256 nested elements (`xml.depth_limit`), and tree walks are iterative. Plain text, CDATA
and mixed XML text are read and replaced with the same semantics. Element and qualified attribute
recognition resolves namespace URIs and local names in `xmldom`. Input prefixes, default namespaces
and scoped aliases do not change the result. Transitional and Strict OOXML URIs share the same
internal vocabulary, including charts, relationships and XLSX model reads. Unrecognized presentation
roots, slide lists or slide shape trees fail with `package.invalid` instead of reporting a
successful empty presentation. Unchanged names keep their original prefixes when edited, and newly
inserted nodes declare any required namespaces. Some LibreOffice versions reject prefixed OPC
manifest roots. PDF conversion normalizes only those package manifests/relationships in its
temporary render copy, never in the source PPTX.

All request strings and Markdown file content must contain only XML 1.0 characters. Forbidden
controls (including NUL) return `input.xml_character` with the `field` path before any write.
Characters are never silently removed, and existing output files remain unchanged.

Writes use a synced temporary file in the destination directory and an atomic rename. Reads finish
in memory before writes. Editing safely supports identical paths and hard-link aliases and reports
identity-based `summary_params.in_place` (device + inode on Unix), not a path-string comparison.
Conversion rejects that identity, and successful conversions report `in_place: false`. On Linux,
writers lock the base directory inode before reading current state. Concurrent in-place edits do not
lose updates. Non-Linux targets keep component validation and atomic writes, but Linux procfs
anchoring and directory locking are not provided there. This is not a LibreOffice sandbox.

Keep LibreOffice updated when processing untrusted Office documents. Rendering needs installed fonts
and can vary with LibreOffice and font versions. Converting a PPTX preserves charts and image
placement. Extracting Markdown is a content export, not a full-fidelity round trip.

## Warnings

Warnings are structured entries, and `warnings` is an empty array when there are none:

```json
{"code":"slide.text_overflow","message":"slide 2: text does not fit even at 14 pt","params":{"slide":"2"}}
```

The emitted warning codes are:

- `slide.text_overflow`: the source slide's body/table text does not fit at 14 pt.
- `slide.title_overflow`: the source title exceeds its area (hero titles retain at least 40 pt).
- `slide.split`: that source slide was divided into `count` output slides (parameter `slide` is the
  one-based source index).
- `image.unreadable`: an image became unreadable after preflight. An initially missing/unreadable
  image fails with `file.read` and its cause before writing.
- `image.unsupported_format`: not PNG/JPEG.
- `image.not_embedded`: corrupt image or decoding failure.
- `image.no_size`: image has invalid dimensions.
- `edit.slide_missing`: a requested slide/edit/delete index does not exist.
- `edit.no_match`: a replacement found no matching semantic paragraph text.
- `edit.field_missing`: no placeholder matched a requested `title`/`bullets` field (`index`,
  `field`).
- `edit.text_fitted`: a changed field required smaller type and autofit. Review its appearance.
- `edit.text_overflow`: a changed existing field cannot be verified to fit at the minimum font.
  `slide` is its one-based output slide number and `hidden_words` conservatively estimates the words
  that may be hidden (all words when geometry cannot be measured). One warning is emitted per
  affected text body. The original text is retained, not silently discarded.
- `template.content_review`: always emitted for templates and nonempty edit additions. Review
  conservative fitting with company fonts.
- `template.layout_fallback`: a requested company name/type was unavailable, and the nearest layout
  was used.
- `template.placeholder_fallback`: some content had no matching company placeholder/geometry.
- `source.overflow`: citations may exceed the footer area (more than three estimated lines).
- `check.scope`: always emitted by `check`, explaining the scope of the deterministic checks.
- `check.model_unlinked`: with `model`, a slide had no unambiguous numeric link.
- `check.unclassified_numbers`: extracted numbers had no unambiguous metric identity (`slide`,
  `count`, `sources`).
- `read.compatibility_unsupported`: no usable compatibility branch or an unsupported shape object.
- `read.truncated`: a requested text/data/layout representation exceeded its budget.
- `read.chart_unsupported`: a detected chart type is not supported (`slide`, `type`).
- `read.chart_combination`: all groups were extracted separately, without combined styling (`slide`,
  `groups`).
- `response.truncated`: QC finding text exceeded 512 characters (individual findings are marked).
- `chart.labels_review`: more than 20 value labels or numeric labels longer than 16 characters may
  overlap or clip. Names the one-based slide and chart element. Exact editable caches remain.

Unsafe image paths fail with `path.unsafe`, not a success warning. All application messages live in
`src/messages.rs`: stable codes, English templates and named parameters. `Error::code()` and
`Error::params()` expose the same identity. Error display is English, and underlying I/O/XML/ZIP
causes remain available through `std::error::Error::source`. `Error` serializes to the same flat
response as the CLI, and `Error::output()` returns that `Output` explicitly. Use `parse_input` to
keep JSON syntax locations and field pointers when decoding library requests.

## Known limits

Reading arbitrary third-party PPTX, `check`, and `edit` remain available but are outside S1–S4
acceptance. They do not promise complete Office feature support or financial interpretation:

| Limit | Coded warning / behavior |
| --- | --- |
| Unsupported shapes, compatibility branches or chart types | `read.compatibility_unsupported`, `read.chart_unsupported`. Inspect the original. Incomplete chart caches fail with `read.chart_incomplete`. |
| Combined chart styling cannot be represented by the extraction model | `read.chart_combination`. Every supported group's cached data is extracted separately. |
| Financial meaning, unlabeled numbers, uncached formulas and ambiguous model links | `check.scope`, `check.unclassified_numbers`, `check.model_unlinked`. No inferred exchange rates or formula evaluation. |
| Foreign text geometry and overflowing table cells during edits | `edit.text_fitted`, `edit.text_overflow`. Review the actual Office render. Missing targets use `edit.no_match`, `edit.field_missing`, `edit.slide_missing`. |
| Company layout coverage and font-dependent fitting | `template.content_review`, `template.layout_fallback`, `template.placeholder_fallback`. Use installed company fonts and review the export. |
| Dense/very long chart labels | `chart.labels_review` identifies the slide and element. Exact values stay editable even if labels overlap. Split the data across charts for readable slides. |
| Invalid image bytes or unsupported image types | `image.not_embedded`, `image.unsupported_format`, `image.no_size`. Preflight rejects missing images as errors. |
| Content budgets and long citations | `read.truncated`, `response.truncated`, `source.overflow`. Raise budgets or shorten citations. |

Markdown extraction is a content export, not a geometry/style round trip, and extracted charts
become data tables. PDF appearance depends on the installed LibreOffice and fonts. Linux provides
the anchored file-I/O guarantees described above. Non-Linux path-race resistance is not equivalent.

## Verification

`cargo test` covers all operations, OOXML/XML structure, font/layout styling, image embedding, all
six chart workbooks and PDF vector marks, actual dense-text PDF word bounds, overflow pagination and
data preservation, foreign LibreOffice packages, company templates generated from ODP, numeric QC
with injected inconsistencies and XLSX links, message catalogs and invalid inputs. Alignment tests
inspect slide XML and measure title-to-content gaps in LibreOffice PNGs at 800 px width, for both
plain and template exports. Test inputs are generated locally. PDF tests require `soffice`,
`fc-scan`, `qpdf`, `pdftoppm` and `pdftotext`. Run `cargo clippy --all-targets -- -D warnings` for
strict lint checks. A reproducible differential test generates 200 mixed text/table/chart decks,
logs its seed, checks every injected disagreement and rejects findings for agreeing pairs.

## Development

Developed with AI assistance (Claude, GPT agents) under the maintainer's review.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT)
at your option.
