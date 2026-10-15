//! Deterministic numeric QC, with explicit labels and cached XLSX values.
use std::collections::BTreeMap;

use crate::messages::{MessageCode, Msg, Warning, params};
use crate::ooxml::{Package, descendants};
use crate::xmldom::El;
use crate::{CheckInput, Error, Options, Output};
use serde::Serialize;

#[derive(Clone, Debug)]
struct Number {
    value: f64,
    unit: String,
    start: usize,
    end: usize,
}
fn numbers(text: &str) -> Vec<Number> {
    let mut out = Vec::new();
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut i = 0;
    while i < chars.len() {
        let (start, c) = chars[i];
        if !c.is_ascii_digit() && !matches!(c, '-' | '+' | '(' | '$' | '€' | '£' | '¥' | '.') {
            i += 1;
            continue;
        }
        if i > 0 && (chars[i - 1].1.is_alphanumeric() || chars[i - 1].1 == '$') {
            i += 1;
            continue;
        }
        let mut j = i;
        let mut negative = false;
        let mut currency = "";
        while j < chars.len() && !chars[j].1.is_ascii_digit() {
            match chars[j].1 {
                '(' | '-' => negative = true,
                '$' => currency = "USD",
                '€' => currency = "EUR",
                '£' => currency = "GBP",
                '¥' => currency = "JPY",
                '+' | ' ' => {}
                _ => break,
            }
            j += 1;
        }
        let digit_start = j;
        while j < chars.len() && (chars[j].1.is_ascii_digit() || matches!(chars[j].1, ',' | '.')) {
            j += 1;
        }
        if j == digit_start {
            i += 1;
            continue;
        }
        if j < chars.len() && matches!(chars[j].1, 'e' | 'E') {
            let mut exponent = j + 1;
            if exponent < chars.len() && matches!(chars[exponent].1, '+' | '-') {
                exponent += 1;
            }
            let first = exponent;
            while exponent < chars.len() && chars[exponent].1.is_ascii_digit() {
                exponent += 1;
            }
            if exponent > first {
                j = exponent;
            }
        }
        if currency.is_empty() {
            currency = match text[..start].split_whitespace().last().unwrap_or("") {
                "USD" => "USD",
                "EUR" => "EUR",
                "GBP" => "GBP",
                "PLN" => "PLN",
                _ => "",
            };
        }
        let end_digits = chars.get(j).map(|(b, _)| *b).unwrap_or(text.len());
        let raw = text[chars[digit_start].0..end_digits]
            .trim_end_matches([',', '.'])
            .replace(',', "");
        let Ok(mut value) = raw.parse::<f64>() else {
            i = j;
            continue;
        };
        if negative {
            value = -value;
        }
        let mut k = j;
        if k < chars.len() && chars[k].1 == ')' {
            k += 1;
        }
        while k < chars.len() && chars[k].1.is_whitespace() {
            k += 1;
        }
        let unit_start = chars.get(k).map(|(b, _)| *b).unwrap_or(text.len());
        while k < chars.len() && (chars[k].1.is_ascii_alphabetic() || chars[k].1 == '%') {
            k += 1;
        }
        let suffix_end = chars.get(k).map(|(b, _)| *b).unwrap_or(text.len());
        let suffix = text[unit_start..suffix_end].to_ascii_lowercase();
        let (mult, unit) = match suffix.as_str() {
            "%" => (0.01, "%"),
            "x" => (1., "x"),
            "k" | "thousand" => (1e3, ""),
            "m" | "mm" | "mn" | "mln" | "million" => (1e6, ""),
            "b" | "bn" | "billion" => (1e9, ""),
            "usd" => (1., "USD"),
            "eur" => (1., "EUR"),
            "gbp" => (1., "GBP"),
            "pln" => (1., "PLN"),
            _ => {
                k = j;
                (1., "")
            }
        };
        value *= mult;
        if value.is_finite() {
            out.push(Number {
                value,
                unit: if currency.is_empty() {
                    unit.into()
                } else {
                    currency.into()
                },
                start,
                end: chars.get(k).map(|(b, _)| *b).unwrap_or(text.len()),
            });
        }
        i = k.max(j);
    }
    out
}
fn key(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect()
}
fn content(el: &El) -> String {
    if el.name == "a:p" {
        return crate::paragraph::Paragraph::read(el).text;
    }
    descendants(el, "a:p")
        .iter()
        .map(|p| crate::paragraph::Paragraph::read(p).text)
        .collect::<Vec<_>>()
        .join("\n")
}
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
struct FactKey {
    metric: String,
    period: String,
    segment: String,
    unit: String,
}
#[derive(Clone, Copy)]
enum Source {
    Text,
    Table,
    Chart,
}
impl Source {
    fn name(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Table => "table",
            Self::Chart => "chart",
        }
    }
}
#[derive(Clone)]
struct Atom {
    slide: usize,
    text: String,
    number: Number,
    metric: FactKey,
    source: Source,
}

/// All producers use this identity function. Units/scales belong to the value, periods
/// belong to dimensions, and punctuation/case never depend on the source representation.
fn fact_key(label: &str, unit: &str) -> FactKey {
    let mut period = Vec::new();
    let mut words = Vec::new();
    // Scale letters are units only in explicit header annotations, not in labels such
    // as "Region M" or "Region K". Removing them globally aliases unrelated segments.
    let mut cleaned = String::new();
    let mut remaining = label;
    while let Some(start) = remaining.find(['(', '[']) {
        cleaned.push_str(&remaining[..start]);
        let tail = &remaining[start + 1..];
        if let Some(end) = tail.find([')', ']']) {
            let annotation = &tail[..end];
            if table_cell("1", annotation) == "1" {
                cleaned.push_str(annotation);
            }
            cleaned.push(' ');
            remaining = &tail[end + 1..];
        } else {
            remaining = &remaining[start..];
            break;
        }
    }
    cleaned.push_str(remaining);
    // Generic headings are ignored as a whole, never as words inside a real metric
    // such as "Enterprise Value".
    if matches!(
        key(&cleaned).as_str(),
        "value" | "values" | "amount" | "metric" | "metrics"
    ) {
        return FactKey {
            unit: unit.into(),
            ..FactKey::default()
        };
    }
    for token in cleaned.split(|c: char| !c.is_alphanumeric()) {
        let token = token.to_lowercase();
        if token.is_empty() {
            continue;
        }
        if period_label(&token) {
            period.push(token);
        } else {
            words.push(token);
        }
    }
    FactKey {
        metric: words.first().cloned().unwrap_or_default(),
        // Preserve word boundaries: "D CF" and "DCF" are not the same segment.
        segment: words.into_iter().skip(1).collect::<Vec<_>>().join(" "),
        period: period.join(" "),
        unit: unit.into(),
    }
}
fn fact(text: &str, number: Number, label: &str, slide: usize, source: Source) -> Atom {
    Atom {
        slide,
        text: text.into(),
        metric: fact_key(label, &number.unit),
        number,
        source,
    }
}
#[derive(Serialize)]
struct Finding {
    slide: usize,
    text: String,
    value: f64,
    issue: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    expected: Option<f64>,
}
fn finding(atom: &Atom, issue: MessageCode, expected: Option<f64>) -> Finding {
    Finding {
        slide: atom.slide,
        text: atom.text.clone(),
        value: atom.number.value,
        issue: issue.as_str(),
        expected,
    }
}
#[derive(Clone, Copy)]
enum NumericRole {
    Prose,
    Label,
}

fn label_value_separator(text: &str) -> Option<usize> {
    text.char_indices()
        .find_map(|(idx, c)| matches!(c, ':' | '=' | '–' | '\t').then_some(idx))
}

fn atoms(text: &str, slide: usize, role: NumericRole, source: Source) -> Vec<Atom> {
    let ns = numbers(text);
    // Numbers in a labeled period precede the value separator. Retain their bytes in
    // the metric identity, but never emit them as independent measurements.
    let label_end = label_value_separator(text).filter(|end| ns.iter().any(|n| n.start > *end));
    let dimension = |n: &Number| match role {
        NumericRole::Label => true,
        NumericRole::Prose => label_end.is_some_and(|end| n.end <= end),
    };
    ns.iter()
        .filter(|n| !dimension(n))
        .enumerate()
        .map(|(position, n)| {
            let mut skeleton = String::new();
            let mut end = 0;
            for other in &ns {
                skeleton.push_str(&text[end..other.start]);
                if dimension(other) {
                    skeleton.push_str(&text[other.start..other.end]);
                } else {
                    skeleton.push('#');
                }
                end = other.end;
            }
            skeleton.push_str(&text[end..]);
            let has_label = !fact_key(&skeleton, &n.unit).metric.is_empty();
            let label = if ns.iter().filter(|n| !dimension(n)).count() == 1 {
                label_end
                    .map(|end| text[..end].to_string())
                    .unwrap_or(skeleton)
            } else {
                format!("{skeleton} position {position}")
            };
            let label = if has_label { &label } else { "" };
            fact(text, n.clone(), label, slide, source)
        })
        .collect()
}
fn table_cell(cell: &str, header: &str) -> String {
    if cell.trim().replace(',', "").parse::<f64>().is_err() {
        return cell.into();
    }
    let tokens: Vec<_> = header
        .split(|c: char| c.is_whitespace() || matches!(c, '(' | ')' | '[' | ']' | '/'))
        .collect();
    for token in tokens {
        let token = token.trim_end_matches('.');
        let (currency, unit) = match token.chars().next() {
            Some(c @ ('$' | '€' | '£' | '¥')) => (c.to_string(), &token[c.len_utf8()..]),
            _ => (String::new(), token),
        };
        if matches!(
            unit.to_ascii_lowercase().as_str(),
            "%" | "x"
                | "k"
                | "m"
                | "mm"
                | "mn"
                | "mln"
                | "million"
                | "b"
                | "bn"
                | "billion"
                | "usd"
                | "eur"
                | "gbp"
                | "pln"
        ) || !currency.is_empty()
        {
            return format!("{currency}{cell} {unit}");
        }
    }
    cell.into()
}
fn normalize_table(rows: &mut [Vec<String>]) {
    if let Some(headers) = rows.first().cloned() {
        for row in rows.iter_mut().skip(1) {
            for (i, cell) in row.iter_mut().enumerate() {
                *cell = table_cell(cell, headers.get(i).map(String::as_str).unwrap_or(""));
            }
        }
    }
}

/// Detect a label axis from actual cell content, not the column index. Explicit
/// period headers also identify numeric years as dimensions, never monetary measures.
fn table_facts(rows: &[Vec<String>], slide: usize) -> Vec<Atom> {
    let Some(headers) = rows.first() else {
        return Vec::new();
    };
    let body = &rows[1..];
    let numeric_cell = |cell: &str| {
        let text = cell.trim();
        let ns = numbers(text);
        matches!(ns.as_slice(), [n] if n.start == 0 && text[n.end..].trim().is_empty())
    };
    let label_column = !body.is_empty()
        && body.iter().all(|row| {
            row.first()
                .is_some_and(|cell| !cell.trim().is_empty() && !numeric_cell(cell))
        })
        || headers.first().is_some_and(|h| {
            matches!(key(h).as_str(), "period" | "year" | "quarter" | "date")
                && body
                    .iter()
                    .all(|r| r.first().is_some_and(|v| period_label(v)))
        });
    let mut out = Vec::new();
    for row in body {
        for (col, cell) in row.iter().enumerate() {
            if col == 0 && label_column {
                continue;
            }
            let header = headers.get(col).map(String::as_str).unwrap_or("");
            let row_label = if label_column {
                row.first().map(String::as_str).unwrap_or("")
            } else {
                ""
            };
            let label = if label_column
                && fact_key(header, "").metric.is_empty()
                && !period_label(header)
            {
                row_label.to_string()
            } else if label_column && period_label(header) {
                format!("{row_label} {header}")
            } else {
                format!("{header} {row_label}")
            };
            let ns = numbers(cell);
            // Multiple unlabeled rows or multiple measures in one cell are ambiguous.
            let label = if (!label_column && body.len() > 1) || ns.len() != 1 {
                ""
            } else {
                &label
            };
            for number in ns {
                let text = format!("{header} {row_label}: {cell}");
                // Preserve offsets for existing model-linking consumers.
                let mut number = number;
                number.start += text.len() - cell.len();
                number.end += text.len() - cell.len();
                out.push(fact(&text, number, label, slide, Source::Table));
            }
        }
    }
    out
}

fn period_label(label: &str) -> bool {
    let lower = label.to_ascii_lowercase();
    lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|token| {
            let token = token.strip_prefix("fy").unwrap_or(token);
            token
                .parse::<u16>()
                .is_ok_and(|year| (1900..=2200).contains(&year))
                || matches!(
                    token,
                    "q1" | "q2"
                        | "q3"
                        | "q4"
                        | "january"
                        | "february"
                        | "march"
                        | "april"
                        | "may"
                        | "june"
                        | "july"
                        | "august"
                        | "september"
                        | "october"
                        | "november"
                        | "december"
                )
        })
}

fn total_label(label: &str) -> bool {
    matches!(key(label).as_str(), "total" | "sum" | "suma" | "grandtotal")
}

/// Both axes use the same header-selected components and totals. A missing/non-numeric
/// cell or incompatible unit means that no arithmetic claim can be made.
struct TablePartition {
    parts: Vec<Number>,
    totals: Vec<Number>,
}
impl TablePartition {
    fn read<'a>(cells: impl Iterator<Item = (&'a str, Option<&'a str>)>) -> Option<Self> {
        let mut partition = Self {
            parts: Vec::new(),
            totals: Vec::new(),
        };
        for (label, cell) in cells {
            let ns = numbers(cell?);
            let [number] = ns.as_slice() else { return None };
            if total_label(label) {
                partition.totals.push(number.clone());
            } else {
                partition.parts.push(number.clone());
            }
        }
        let unit = &partition.parts.first()?.unit;
        partition
            .parts
            .iter()
            .chain(&partition.totals)
            .all(|n| &n.unit == unit)
            .then_some(partition)
    }
    fn check(
        &self,
        slide: usize,
        text: String,
        percent: bool,
        tolerance: f64,
        findings: &mut Vec<Finding>,
    ) {
        let expected: f64 = self.parts.iter().map(|n| n.value).sum();
        for actual in &self.totals {
            if (actual.value - expected).abs() > tolerance {
                findings.push(Finding {
                    slide,
                    text: text.clone(),
                    value: actual.value,
                    issue: MessageCode::TableTotal.as_str(),
                    expected: Some(expected),
                });
            }
        }
        if percent
            && self.parts.len() > 1
            && self.parts.iter().all(|n| n.unit == "%")
            && (expected - 1.).abs() > tolerance
        {
            findings.push(Finding {
                slide,
                text,
                value: expected,
                issue: MessageCode::TablePercent.as_str(),
                expected: Some(1.),
            });
        }
    }
}

fn table_checks(rows: &[Vec<String>], slide: usize, tolerance: f64, findings: &mut Vec<Finding>) {
    if rows.len() < 2 {
        return;
    }
    let total = total_label;
    let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
    let column_labels: Vec<_> = rows[0]
        .iter()
        .skip(1)
        .filter(|label| !total(label))
        .collect();
    let row_labels: Vec<_> = rows
        .iter()
        .skip(1)
        .filter_map(|row| row.first())
        .filter(|label| !total(label))
        .collect();
    let column_periods =
        !column_labels.is_empty() && column_labels.iter().all(|label| period_label(label));
    let row_periods = !row_labels.is_empty() && row_labels.iter().all(|label| period_label(label));
    let partial_periods = column_labels.iter().any(|label| period_label(label)) && !column_periods
        || row_labels.iter().any(|label| period_label(label)) && !row_periods;
    let share_columns: Vec<_> = rows[0]
        .iter()
        .enumerate()
        .skip(1)
        .filter_map(|(col, label)| {
            let label_key = key(label);
            (label.contains('%') || label_key.contains("share") || label_key.contains("percent"))
                .then_some(col)
        })
        .collect();
    // A period is a separate partition, not a component of the partition. With no
    // period or unique share header there is not enough information to choose an axis.
    let vertical = !partial_periods
        && (column_periods && !row_periods
            || (!column_periods && !row_periods && share_columns.len() == 1));
    let horizontal = !partial_periods && row_periods && !column_periods;
    for col in 1..cols {
        let label = rows[0].get(col).map(String::as_str).unwrap_or("");
        if total(label) {
            continue;
        }
        if let Some(partition) = TablePartition::read(rows.iter().skip(1).map(|row| {
            (
                row.first().map(String::as_str).unwrap_or(""),
                row.get(col).map(String::as_str),
            )
        })) {
            partition.check(
                slide,
                label.into(),
                vertical && (column_periods || share_columns.contains(&col)),
                tolerance,
                findings,
            );
        }
    }
    for row in rows.iter().skip(1) {
        if row.first().is_some_and(|label| total(label)) {
            continue;
        }
        if let Some(partition) = TablePartition::read(
            rows[0]
                .iter()
                .enumerate()
                .skip(1)
                .map(|(col, label)| (label.as_str(), row.get(col).map(String::as_str))),
        ) {
            partition.check(slide, row.join(" | "), horizontal, tolerance, findings);
        }
    }
}

#[derive(Clone)]
struct Cell {
    value: f64,
    label: String,
}
#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum Reference {
    Cell { sheet: String, address: String },
    Name(String),
}
impl Reference {
    fn parse(raw: &str) -> Option<Self> {
        let raw = raw.trim().trim_start_matches('=');
        if let Some((sheet, address)) = raw.rsplit_once('!') {
            let sheet = if sheet.starts_with('\'') && sheet.ends_with('\'') && sheet.len() >= 2 {
                sheet[1..sheet.len() - 1].replace("''", "'")
            } else {
                sheet.to_string()
            };
            let address = address.replace('$', "").to_ascii_uppercase();
            let split = address.find(|c: char| c.is_ascii_digit())?;
            if sheet.is_empty()
                || split == 0
                || !address[..split].bytes().all(|b| b.is_ascii_uppercase())
                || !address[split..].bytes().all(|b| b.is_ascii_digit())
                || address[split..].parse::<u32>().ok()? == 0
            {
                return None;
            }
            Some(Self::Cell {
                sheet: sheet.to_lowercase(),
                address,
            })
        } else if !raw.is_empty() {
            Some(Self::Name(raw.to_lowercase()))
        } else {
            None
        }
    }
}
#[derive(Clone, Default)]
struct Resolution {
    cells: Vec<Cell>,
    unresolved: Vec<String>,
}
struct Model {
    refs: BTreeMap<Reference, Resolution>,
    labels: BTreeMap<String, Vec<Cell>>,
}
fn model_error(part: &str) -> Error {
    crate::input_error(MessageCode::ModelInvalid, params(&[("part", part)]))
}
fn xlsx(bytes: &[u8], limit: u64) -> Result<Model, Error> {
    let pkg = Package::open(bytes, limit)?;
    let book = pkg.xml("xl/workbook.xml")?;
    let sheets = pkg.related("xl/workbook.xml", "worksheet")?;
    let shared = if pkg.0.contains_key("xl/sharedStrings.xml") {
        descendants(&pkg.xml("xl/sharedStrings.xml")?, "si")
            .iter()
            .map(|s| {
                descendants(s, "t")
                    .iter()
                    .map(|t| t.text())
                    .collect::<String>()
            })
            .collect()
    } else {
        Vec::<String>::new()
    };
    let mut model = Model {
        refs: BTreeMap::new(),
        labels: BTreeMap::new(),
    };
    for sheet in descendants(&book, "sheet") {
        let name = sheet
            .attr("name")
            .ok_or_else(|| model_error("sheet name"))?;
        let part = &sheets
            .iter()
            .find(|(id, _)| Some(id.as_str()) == sheet.attr("r:id"))
            .ok_or_else(|| model_error(name))?
            .1;
        let dom = pkg.xml(part)?;
        for row in descendants(&dom, "row") {
            let mut label = String::new();
            for c in row.children_named("c") {
                let addr = c.attr("r").ok_or_else(|| model_error(part))?;
                let raw = c.child("v").map(|v| v.text()).unwrap_or_default();
                match c.attr("t") {
                    Some("s") => {
                        let i = raw.parse::<usize>().map_err(|source| Error::ModelIndex {
                            part: addr.into(),
                            source,
                        })?;
                        label = shared.get(i).ok_or_else(|| model_error(addr))?.clone();
                    }
                    Some("inlineStr") => {
                        label = descendants(c, "t").iter().map(|t| t.text()).collect()
                    }
                    Some("str") => label = raw,
                    Some("e") | Some("b") => {}
                    _ => {
                        if raw.is_empty() {
                            continue;
                        }
                        let value = raw.parse::<f64>().map_err(|source| Error::ModelNumber {
                            part: addr.into(),
                            source,
                        })?;
                        if !value.is_finite() {
                            return Err(model_error(addr));
                        }
                        let cell = Cell {
                            value,
                            label: label.clone(),
                        };
                        let reference = Reference::parse(&format!("{name}!{addr}"))
                            .ok_or_else(|| model_error(addr))?;
                        model.refs.insert(
                            reference,
                            Resolution {
                                cells: vec![cell.clone()],
                                unresolved: Vec::new(),
                            },
                        );
                        if !label.is_empty() {
                            model.labels.entry(key(&label)).or_default().push(cell);
                        }
                    }
                }
            }
        }
    }
    for named in descendants(&book, "definedName") {
        let name = named
            .attr("name")
            .ok_or_else(|| model_error("definedName"))?;
        let mut resolution = Resolution::default();
        for reference in reference_tokens(named.text().trim_start_matches('='), &[',', ';']) {
            let values = resolve_reference(reference, &model)?;
            resolution.cells.extend(values.cells);
            resolution.unresolved.extend(values.unresolved);
        }
        if !resolution.cells.is_empty() && resolution.unresolved.is_empty() {
            let aliases = model.labels.entry(key(name)).or_default();
            if aliases.is_empty() {
                aliases.extend(resolution.cells.iter().cloned());
            }
        }
        model
            .refs
            .insert(Reference::Name(name.to_lowercase()), resolution);
    }
    Ok(model)
}
/// Split reference-list or range punctuation only outside Excel single-quoted names.
/// A doubled apostrophe is a literal apostrophe inside a quoted identifier.
fn reference_tokens<'a>(text: &'a str, separators: &[char]) -> Vec<&'a str> {
    let mut quoted = false;
    let mut chars = text.char_indices().peekable();
    let mut start = 0;
    let mut tokens = Vec::new();
    while let Some((offset, c)) = chars.next() {
        if c == '\'' {
            if quoted && chars.peek().is_some_and(|(_, next)| *next == '\'') {
                chars.next();
            } else {
                quoted = !quoted;
            }
        } else if !quoted && separators.contains(&c) {
            tokens.push(text[start..offset].trim());
            start = offset + c.len_utf8();
        }
    }
    tokens.push(text[start..].trim());
    tokens
}

fn expand_reference(reference: &str) -> Result<Vec<String>, Error> {
    let tokens = reference_tokens(reference, &[':']);
    let [start, end] = tokens.as_slice() else {
        return Ok(if tokens.len() == 1 {
            vec![reference.trim().into()]
        } else {
            vec![]
        });
    };
    let Some((sheet, first)) = start.rsplit_once('!') else {
        return Ok(vec![]);
    };
    fn address(s: &str) -> Option<(u32, u32)> {
        let s = s.replace('$', "");
        let i = s.find(|c: char| c.is_ascii_digit())?;
        let mut col = 0u32;
        for c in s[..i].bytes() {
            if !c.is_ascii_alphabetic() {
                return None;
            }
            col = col
                .checked_mul(26)?
                .checked_add(u32::from(c.to_ascii_uppercase() - b'A') + 1)?;
        }
        Some((col, s[i..].parse().ok()?))
    }
    let Some((c1, r1)) = address(first) else {
        return Ok(vec![]);
    };
    let Some((c2, r2)) = address(end) else {
        return Ok(vec![]);
    };
    if c1 == 0
        || r1 == 0
        || c2 < c1
        || r2 < r1
        || u64::from(c2 - c1 + 1) * u64::from(r2 - r1 + 1) > 100_000
    {
        return Err(model_error(reference));
    }
    let mut out = Vec::new();
    for r in r1..=r2 {
        for c in c1..=c2 {
            let mut n = c;
            let mut letters = Vec::new();
            while n > 0 {
                letters.push(char::from(b'A' + ((n - 1) % 26) as u8));
                n = (n - 1) / 26;
            }
            let col: String = letters.into_iter().rev().collect();
            out.push(format!("{sheet}!{col}{r}"));
        }
    }
    Ok(out)
}
fn resolve_reference(reference: &str, model: &Model) -> Result<Resolution, Error> {
    let mut out = Resolution::default();
    let expanded = expand_reference(reference)?;
    if expanded.is_empty() {
        out.unresolved.push(reference.into());
    }
    for reference in expanded {
        match Reference::parse(&reference).and_then(|r| model.refs.get(&r)) {
            Some(values) => {
                out.cells.extend(values.cells.iter().cloned());
                out.unresolved.extend(values.unresolved.iter().cloned());
                if values.cells.is_empty() && values.unresolved.is_empty() {
                    out.unresolved.push(reference);
                }
            }
            None => out.unresolved.push(reference),
        }
    }
    Ok(out)
}

fn source_refs(source: &str, model: &Model) -> Result<Resolution, Error> {
    let mut out = Resolution::default();
    let source = source.strip_prefix("Source:").unwrap_or(source).trim();
    for token in reference_tokens(source, &[',', ';', '[', ']', '\n']) {
        // A workbook citation is metadata, not a cell/name reference. Do not discard
        // unresolved cell references merely because another token resolved successfully.
        if token.is_empty() || (!token.contains('!') && crate::extension(token) == "xlsx") {
            continue;
        }
        let values = resolve_reference(token, model)?;
        out.cells.extend(values.cells);
        out.unresolved.extend(values.unresolved);
    }
    if out.cells.is_empty() && out.unresolved.is_empty() {
        out.unresolved.push(source.into());
    }
    Ok(out)
}

pub(crate) fn run(req: &CheckInput, opts: &Options) -> Result<Output, Error> {
    let tolerance = req.tolerance.unwrap_or(0.000001);
    if !tolerance.is_finite() || tolerance < 0. {
        return Err(crate::input_error(MessageCode::CheckTolerance, vec![]));
    }
    if crate::extension(&req.input) != "pptx" {
        return Err(crate::input_error(
            MessageCode::UnsupportedReadInput,
            params(&[("path", &req.input)]),
        ));
    }
    let bytes = crate::read_request(&opts.base_dir, &req.input, opts)?;
    let pkg = Package::open(&bytes, opts.max_input_bytes)?;
    let model = req
        .model
        .as_deref()
        .map(|path| {
            let bytes = crate::read_request(&opts.base_dir, path, opts)?;
            if crate::extension(path) != "xlsx" {
                return Err(model_error(path));
            }
            xlsx(&bytes, opts.max_input_bytes)
        })
        .transpose()?;
    let mut all = Vec::<Atom>::new();
    let mut findings = Vec::new();
    // Group by explicit identity only. Unrelated foreign tables never merge by appearance.
    let mut logical_tables =
        BTreeMap::<String, (usize, Vec<String>, BTreeMap<usize, Vec<String>>)>::new();
    let mut warnings = vec![Warning::new(MessageCode::CheckScope, vec![], &opts.locale)];
    for (index, part) in pkg.slides()?.iter().enumerate() {
        let slide = index + 1;
        let dom = pkg.xml(part)?;
        let mut current = Vec::new();
        let mut sources = Vec::new();
        let mut objects = Vec::new();
        let tree = crate::ooxml::slide_tree(&dom)?;
        if !crate::pptx::shape_objects(tree, &mut objects) {
            warnings.push(Warning::new(
                MessageCode::ReadCompatibilityUnsupported,
                params(&[("slide", &slide.to_string())]),
                &opts.locale,
            ));
        }
        for shape in objects.iter().filter(|shape| shape.name == "p:sp") {
            let ph = descendants(shape, "p:ph")
                .first()
                .and_then(|p| p.attr("type"))
                .unwrap_or("");
            let name = descendants(shape, "p:cNvPr")
                .first()
                .and_then(|p| p.attr("name"))
                .unwrap_or("");
            if matches!(ph, "sldNum" | "dt" | "ftr") {
                continue;
            }
            for para in descendants(shape, "a:p") {
                let line = content(para);
                if name == "Source footnotes" || line.starts_with("Source:") {
                    sources.push(line);
                } else {
                    let role = if matches!(ph, "title" | "ctrTitle") {
                        NumericRole::Label
                    } else {
                        NumericRole::Prose
                    };
                    current.extend(atoms(&line, slide, role, Source::Text));
                }
            }
        }
        for table in objects
            .iter()
            .flat_map(|object| descendants(object, "a:tbl"))
        {
            let mut rows: Vec<Vec<String>> = table
                .children_named("a:tr")
                .map(|r| r.children_named("a:tc").map(content).collect())
                .collect();
            // Logical reassembly keeps raw cell slices; normalize header units only after
            // all slices of a row have been joined.
            let raw_rows = rows.clone();
            normalize_table(&mut rows);
            if let Some(headers) = rows.first() {
                for header in headers {
                    current.extend(atoms(header, slide, NumericRole::Label, Source::Table));
                }
            }
            if let Some((id, start)) = crate::pptx::table_metadata(table)? {
                let entry = logical_tables.entry(id).or_insert_with(|| {
                    (
                        slide,
                        raw_rows.first().cloned().unwrap_or_default(),
                        BTreeMap::new(),
                    )
                });
                for (offset, row) in raw_rows.iter().skip(1).enumerate() {
                    let index = start
                        .checked_add(offset)
                        .ok_or_else(|| crate::ooxml::invalid("table row offset"))?;
                    if let Some(previous) = entry.2.get_mut(&index) {
                        // A single oversized row can itself span slides; its cell slices are
                        // contiguous text, unlike separate paragraphs inside a cell.
                        previous.resize(previous.len().max(row.len()), String::new());
                        for (col, cell) in row.iter().enumerate() {
                            previous[col].push_str(cell);
                        }
                    } else {
                        entry.2.insert(index, row.clone());
                    }
                }
            } else {
                table_checks(&rows, slide, tolerance, &mut findings);
            }
            current.extend(table_facts(&rows, slide));
        }
        for (_, chart) in pkg.related(part, "chart")? {
            let chart = pkg.xml(&chart)?;
            for para in descendants(&chart, "a:p") {
                current.extend(atoms(
                    &content(para),
                    slide,
                    NumericRole::Prose,
                    Source::Chart,
                ));
            }
            for kind in crate::charts::unsupported_types(&chart) {
                warnings.push(Warning::new(
                    MessageCode::ReadChartUnsupported,
                    params(&[("slide", &slide.to_string()), ("type", &kind)]),
                    &opts.locale,
                ));
            }
            for series in crate::charts::cached_series(&chart)? {
                for point in series.points.values() {
                    let Some(value) = point.value else { continue };
                    let text = value.to_string();
                    let label = point
                        .label
                        .as_ref()
                        .filter(|_| !series.name.trim().is_empty())
                        .map(|category| {
                            format!(
                                "{} {}{category}",
                                series.name,
                                if series.scatter { "X " } else { "" }
                            )
                        })
                        .unwrap_or_default();
                    let context = format!("{label}: {text}");
                    for number in numbers(&table_cell(&text, &series.name)) {
                        current.push(fact(&context, number, &label, slide, Source::Chart));
                    }
                }
            }
        }
        if let Some(model) = &model {
            let mut linked = 0;
            for source in &sources {
                let resolution = source_refs(source, model)?;
                for reference in resolution.unresolved {
                    findings.push(Finding {
                        slide,
                        text: reference,
                        value: 0.,
                        issue: MessageCode::SourceUnresolved.as_str(),
                        expected: None,
                    });
                }
                for cell in resolution.cells {
                    let candidates: Vec<_> = current
                        .iter()
                        .filter(|a| {
                            !cell.label.is_empty()
                                && (key(&a.text).contains(&key(&cell.label))
                                    || key(&format!("{} {}", a.metric.metric, a.metric.segment))
                                        .contains(&key(&cell.label)))
                        })
                        .collect();
                    let candidates = if candidates.is_empty() && current.len() == 1 {
                        current.iter().collect()
                    } else {
                        candidates
                    };
                    if candidates.len() == 1 {
                        linked += 1;
                        let atom = candidates[0];
                        if (atom.number.value - cell.value).abs() > tolerance {
                            findings.push(finding(
                                atom,
                                MessageCode::ModelMismatch,
                                Some(cell.value),
                            ));
                        }
                    }
                }
            }
            if sources.is_empty() {
                for atom in &current {
                    let label = key(&atom.text[..atom.number.start]);
                    if let Some(cells) = model.labels.get(&label)
                        && cells.len() == 1
                    {
                        linked += 1;
                        if (atom.number.value - cells[0].value).abs() > tolerance {
                            findings.push(finding(
                                atom,
                                MessageCode::ModelMismatch,
                                Some(cells[0].value),
                            ));
                        }
                    }
                }
            }
            if linked == 0 {
                warnings.push(Warning::new(
                    MessageCode::ModelUnlinked,
                    params(&[("slide", &slide.to_string())]),
                    &opts.locale,
                ));
            }
        }
        let unclassified: Vec<_> = current
            .iter()
            .filter(|a| a.metric.metric.is_empty())
            .collect();
        if !unclassified.is_empty() {
            let sources = unclassified
                .iter()
                .map(|a| a.source.name())
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
                .join(", ");
            warnings.push(Warning::new(
                MessageCode::CheckUnclassifiedNumbers,
                params(&[
                    ("slide", &slide.to_string()),
                    ("count", &unclassified.len().to_string()),
                    ("sources", &sources),
                ]),
                &opts.locale,
            ));
        }
        all.extend(current);
    }
    for (slide, headers, body) in logical_tables.into_values() {
        let mut rows = vec![headers];
        rows.extend(body.into_values());
        normalize_table(&mut rows);
        table_checks(&rows, slide, tolerance, &mut findings);
    }
    let mut seen: BTreeMap<FactKey, &Atom> = BTreeMap::new();
    for atom in &all {
        // A bare numeric paragraph has no metric identity; do not equate unrelated figures.
        if atom.metric.metric.is_empty() {
            continue;
        }
        if let Some(previous) = seen.get(&atom.metric) {
            if previous.slide != atom.slide
                && (previous.number.value - atom.number.value).abs() > tolerance
            {
                findings.push(finding(
                    atom,
                    MessageCode::MetricMismatch,
                    Some(previous.number.value),
                ));
            }
        } else {
            seen.insert(atom.metric.clone(), atom);
        }
    }
    let summary = Msg::new(
        MessageCode::CheckDone,
        params(&[
            ("count", &findings.len().to_string()),
            ("numbers", &all.len().to_string()),
        ]),
    );
    let mut data = serde_json::to_value(findings).map_err(|source| Error::CheckJson { source })?;
    let mut truncated = false;
    for finding in data.as_array_mut().into_iter().flatten() {
        if let Some(text) = finding["text"].as_str()
            && text.chars().count() > 512
        {
            finding["text"] = serde_json::Value::String(text.chars().take(512).collect());
            finding["text_truncated"] = serde_json::Value::Bool(true);
            truncated = true;
        }
    }
    if truncated {
        warnings.push(Warning::new(
            MessageCode::ResponseTruncated,
            params(&[("limit", "512")]),
            &opts.locale,
        ));
    }
    Ok(crate::output(
        None,
        Some(bytes.len() as u64),
        summary,
        None,
        Some(data),
        warnings,
        &opts.locale,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn financial_units_and_currency_normalize() {
        let n =
            numbers("Revenue $1,250.5 mln; margin 25%; leverage 4.2x; loss (3.5) bn; value €2bn");
        assert_eq!(n.len(), 5);
        assert_eq!(n[0].value, 1_250_500_000.);
        assert_eq!(n[0].unit, "USD");
        assert_eq!(n[1].value, 0.25);
        assert_eq!(n[2].unit, "x");
        assert_eq!(n[3].value, -3.5e9);
        assert_eq!(n[4].value, 2e9);
    }
}
