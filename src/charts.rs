//! Small native DrawingML charts with an editable embedded SpreadsheetML workbook.

use std::io::{Cursor, Write};
use zip::write::SimpleFileOptions;

use crate::Error;
use crate::deck::{Chart, ChartKind, Theme};
use crate::xmlw::Xml;

const C: &str = "http://schemas.openxmlformats.org/drawingml/2006/chart";
const A: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
const R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const S: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";

fn column(mut n: usize) -> String {
    let mut s = String::new();
    n += 1;
    while n > 0 {
        s.insert(0, (b'A' + ((n - 1) % 26) as u8) as char);
        n = (n - 1) / 26;
    }
    s
}

fn value(x: &mut Xml, tag: &str, val: impl ToString) {
    x.empty(tag, &[("val", &val.to_string())]);
}

fn cache(x: &mut Xml, formula: &str, values: &[String], numeric: bool) {
    let (reference, cache) = if numeric {
        ("c:numRef", "c:numCache")
    } else {
        ("c:strRef", "c:strCache")
    };
    x.open(reference, &[]);
    x.el("c:f", &[], formula);
    x.open(cache, &[]);
    if numeric {
        x.el("c:formatCode", &[], "General");
    }
    value(x, "c:ptCount", values.len());
    for (i, v) in values.iter().enumerate() {
        x.open("c:pt", &[("idx", &i.to_string())]);
        x.el("c:v", &[], v);
        x.close("c:pt");
    }
    x.close(cache);
    x.close(reference);
}

/// Replace generated literal styling with theme references resolved through the owning slide's
/// selected company layout/master. Cached data and chart geometry remain unchanged.
pub(crate) fn company_style(bytes: &[u8], theme: Theme) -> Result<Vec<u8>, Error> {
    let mut dom = crate::xmldom::parse(bytes)?;
    let mut colors: Vec<_> = theme
        .accent
        .iter()
        .enumerate()
        .map(|(i, rgb)| (*rgb, format!("accent{}", i + 1)))
        .collect();
    colors.extend([(theme.lt1, "bg1".into()), (theme.dk1, "tx1".into())]);
    dom.visit_mut(|el| {
        if el.name == "a:srgbClr"
            && let Some(scheme) = el.attr("val").and_then(|rgb| {
                colors
                    .iter()
                    .find(|(c, _)| *c == rgb)
                    .map(|(_, s)| s.clone())
            })
        {
            el.name = "a:schemeClr".into();
            el.set_attr("val", &scheme);
        }
        if el.name == "a:latin" {
            el.set_attr("typeface", "+mn-lt");
        }
        true
    });
    Ok(crate::xmldom::write(&dom))
}

pub fn xml(chart: &Chart, theme: Theme) -> Vec<u8> {
    let mut x = Xml::new();
    x.decl();
    x.open(
        "c:chartSpace",
        &[("xmlns:c", C), ("xmlns:a", A), ("xmlns:r", R)],
    );
    value(&mut x, "c:lang", "en-US");
    x.open("c:chart", &[]);
    value(&mut x, "c:autoTitleDeleted", 1);
    x.open("c:plotArea", &[]);
    x.empty("c:layout", &[]);
    let tag = match chart.kind {
        ChartKind::Bar | ChartKind::Column => "c:barChart",
        ChartKind::Area => "c:areaChart",
        ChartKind::Scatter => "c:scatterChart",
        ChartKind::Line => "c:lineChart",
        ChartKind::Pie => "c:pieChart",
    };
    x.open(tag, &[]);
    if matches!(chart.kind, ChartKind::Bar | ChartKind::Column) {
        value(
            &mut x,
            "c:barDir",
            if chart.kind == ChartKind::Bar {
                "bar"
            } else {
                "col"
            },
        );
    }
    if chart.kind == ChartKind::Scatter {
        value(&mut x, "c:scatterStyle", "marker");
    }
    if !matches!(chart.kind, ChartKind::Pie | ChartKind::Scatter) {
        value(
            &mut x,
            "c:grouping",
            if matches!(chart.kind, ChartKind::Bar | ChartKind::Column) {
                "clustered"
            } else {
                "standard"
            },
        );
    }
    value(
        &mut x,
        "c:varyColors",
        usize::from(chart.kind == ChartKind::Pie),
    );
    for (i, series) in chart.series.iter().enumerate() {
        x.open("c:ser", &[]);
        value(&mut x, "c:idx", i);
        value(&mut x, "c:order", i);
        let col = column(i + 1);
        x.open("c:tx", &[]);
        cache(
            &mut x,
            &format!("Sheet1!${col}$1"),
            std::slice::from_ref(&series.name),
            false,
        );
        x.close("c:tx");
        x.open("c:spPr", &[]);
        if chart.kind == ChartKind::Line {
            x.open("a:ln", &[("w", "38100")]);
        }
        x.open("a:solidFill", &[]);
        x.empty("a:srgbClr", &[("val", theme.accent[i % 6])]);
        x.close("a:solidFill");
        if chart.kind == ChartKind::Line {
            x.close("a:ln");
        }
        if chart.kind == ChartKind::Scatter {
            x.open("a:ln", &[]);
            x.empty("a:noFill", &[]);
            x.close("a:ln");
        }
        x.close("c:spPr");
        if chart.kind == ChartKind::Scatter {
            x.open("c:marker", &[]);
            value(&mut x, "c:symbol", "circle");
            value(&mut x, "c:size", 7);
            x.open("c:spPr", &[]);
            x.open("a:solidFill", &[]);
            x.empty("a:srgbClr", &[("val", theme.accent[i % 6])]);
            x.close("a:solidFill");
            x.close("c:spPr");
            x.close("c:marker");
        }
        if chart.kind == ChartKind::Pie {
            for k in 0..chart.labels.len() {
                x.open("c:dPt", &[]);
                value(&mut x, "c:idx", k);
                x.open("c:spPr", &[]);
                x.open("a:solidFill", &[]);
                x.empty("a:srgbClr", &[("val", theme.accent[k % 6])]);
                x.close("a:solidFill");
                x.close("c:spPr");
                x.close("c:dPt");
            }
        }
        // Literal per-point formats prevent Office from rounding source values in PDF labels.
        // The editable numeric caches/workbook remain the authority for chart calculations.
        x.open("c:dLbls", &[]);
        for (point, number) in series.values.iter().enumerate() {
            x.open("c:dLbl", &[]);
            value(&mut x, "c:idx", point);
            x.empty(
                "c:numFmt",
                &[
                    ("formatCode", &format!("\"{number}\"")),
                    ("sourceLinked", "0"),
                ],
            );
            if matches!(chart.kind, ChartKind::Bar | ChartKind::Column) {
                x.open("c:spPr", &[]);
                x.open("a:solidFill", &[]);
                x.empty("a:srgbClr", &[("val", theme.lt1)]);
                x.close("a:solidFill");
                x.close("c:spPr");
                value(&mut x, "c:dLblPos", "inBase");
            }
            value(&mut x, "c:showLegendKey", 0);
            value(&mut x, "c:showVal", 1);
            value(&mut x, "c:showCatName", 0);
            value(&mut x, "c:showSerName", 0);
            x.close("c:dLbl");
        }
        value(&mut x, "c:showLegendKey", 0);
        value(&mut x, "c:showVal", 1);
        value(&mut x, "c:showCatName", 0);
        value(&mut x, "c:showSerName", 0);
        x.close("c:dLbls");
        let cat_tag = if chart.kind == ChartKind::Scatter {
            "c:xVal"
        } else {
            "c:cat"
        };
        let val_tag = if chart.kind == ChartKind::Scatter {
            "c:yVal"
        } else {
            "c:val"
        };
        x.open(cat_tag, &[]);
        cache(
            &mut x,
            &format!("Sheet1!$A$2:$A${}", chart.labels.len() + 1),
            &chart.labels,
            chart.kind == ChartKind::Scatter,
        );
        x.close(cat_tag);
        x.open(val_tag, &[]);
        let values = series
            .values
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        cache(
            &mut x,
            &format!("Sheet1!${col}$2:${col}${}", values.len() + 1),
            &values,
            true,
        );
        x.close(val_tag);
        if matches!(chart.kind, ChartKind::Line | ChartKind::Scatter) {
            value(&mut x, "c:smooth", 0);
        }
        x.close("c:ser");
    }
    if matches!(chart.kind, ChartKind::Bar | ChartKind::Column) {
        value(&mut x, "c:gapWidth", 70);
    }
    if chart.kind != ChartKind::Pie {
        value(&mut x, "c:axId", 1);
        value(&mut x, "c:axId", 2);
    }
    x.close(tag);
    if chart.kind != ChartKind::Pie {
        let horizontal = chart.kind == ChartKind::Bar;
        let category_axis = if chart.kind == ChartKind::Scatter {
            "c:valAx"
        } else {
            "c:catAx"
        };
        for (tag, id, cross, pos) in [
            (category_axis, 1, 2, if horizontal { "l" } else { "b" }),
            ("c:valAx", 2, 1, if horizontal { "b" } else { "l" }),
        ] {
            x.open(tag, &[]);
            value(&mut x, "c:axId", id);
            x.open("c:scaling", &[]);
            value(&mut x, "c:orientation", "minMax");
            if id == 2 && matches!(chart.kind, ChartKind::Bar | ChartKind::Column) {
                if chart
                    .series
                    .iter()
                    .flat_map(|s| &s.values)
                    .all(|v| *v >= 0.)
                {
                    value(&mut x, "c:min", 0);
                } else if chart
                    .series
                    .iter()
                    .flat_map(|s| &s.values)
                    .all(|v| *v <= 0.)
                {
                    value(&mut x, "c:max", 0);
                }
            }
            x.close("c:scaling");
            value(&mut x, "c:delete", 0);
            value(&mut x, "c:axPos", pos);
            if id == 2 {
                x.empty("c:majorGridlines", &[]);
                x.empty(
                    "c:numFmt",
                    &[("formatCode", "General"), ("sourceLinked", "1")],
                );
            }
            // Keep category labels at the plot edge, away from negative value labels at zero.
            value(
                &mut x,
                "c:tickLblPos",
                if id == 1 { "low" } else { "nextTo" },
            );
            value(&mut x, "c:crossAx", cross);
            value(&mut x, "c:crosses", "autoZero");
            if id == 2 {
                value(&mut x, "c:crossBetween", "between");
            }
            x.close(tag);
        }
    }
    x.close("c:plotArea");
    x.open("c:legend", &[]);
    value(&mut x, "c:legendPos", "b");
    x.empty("c:layout", &[]);
    value(&mut x, "c:overlay", 0);
    x.close("c:legend");
    value(&mut x, "c:plotVisOnly", 1);
    x.close("c:chart");
    x.open("c:spPr", &[]);
    x.open("a:solidFill", &[]);
    x.empty("a:srgbClr", &[("val", theme.lt1)]);
    x.close("a:solidFill");
    x.open("a:ln", &[]);
    x.empty("a:noFill", &[]);
    x.close("a:ln");
    x.close("c:spPr");
    x.open("c:txPr", &[]);
    x.empty("a:bodyPr", &[]);
    x.empty("a:lstStyle", &[]);
    x.open("a:p", &[]);
    x.open("a:pPr", &[]);
    x.open("a:defRPr", &[("sz", "1600")]);
    x.open("a:solidFill", &[]);
    x.empty("a:srgbClr", &[("val", theme.dk1)]);
    x.close("a:solidFill");
    x.empty("a:latin", &[("typeface", theme.minor_font)]);
    x.close("a:defRPr");
    x.close("a:pPr");
    x.empty("a:endParaRPr", &[("lang", "en-US")]);
    x.close("a:p");
    x.close("c:txPr");
    x.open("c:externalData", &[("r:id", "rId1")]);
    value(&mut x, "c:autoUpdate", 0);
    x.close("c:externalData");
    x.close("c:chartSpace");
    x.finish()
}

struct PointCache {
    points: std::collections::BTreeMap<u32, String>,
    complete: bool,
}

/// Read only explicit cache points; indices/counts never allocate slots or imply zero values.
fn cache_points(slot: Option<&crate::xmldom::El>) -> Result<PointCache, Error> {
    let mut points = std::collections::BTreeMap::new();
    let mut complete = false;
    if let Some(slot) = slot {
        let cache = ["c:strCache", "c:numCache", "c:strLit", "c:numLit"]
            .iter()
            .find_map(|tag| crate::ooxml::descendants(slot, tag).first().copied());
        if let Some(cache) = cache {
            complete = true;
            for point in cache.children_named("c:pt") {
                let index = point
                    .attr("idx")
                    .ok_or_else(|| crate::ooxml::invalid("chart point index"))?
                    .parse::<u32>()
                    .map_err(|source| Error::XmlSource {
                        source: Box::new(source),
                    })?;
                if let Some(value) = point.child("c:v") {
                    if points.insert(index, value.text()).is_some() {
                        return Err(crate::ooxml::invalid("duplicate chart point index"));
                    }
                } else {
                    complete = false;
                }
            }
            if let Some(count) = cache.child("c:ptCount").and_then(|c| c.attr("val")) {
                let count = count.parse::<usize>().map_err(|source| Error::XmlSource {
                    source: Box::new(source),
                })?;
                complete &= count == points.len();
            }
        }
    }
    Ok(PointCache { points, complete })
}

pub(crate) struct CachedPoint {
    pub label: Option<String>,
    pub value: Option<f64>,
}
pub(crate) struct CachedSeries {
    pub name: String,
    pub scatter: bool,
    pub complete: bool,
    pub points: std::collections::BTreeMap<u32, CachedPoint>,
}

/// The single category/X and value/Y join used by extraction and numeric QC.
pub(crate) fn cached_series(chart: &crate::xmldom::El) -> Result<Vec<CachedSeries>, Error> {
    let mut out = Vec::new();
    for ser in crate::ooxml::descendants(chart, "c:ser") {
        let scatter = ser.child("c:xVal").is_some();
        let labels = cache_points(ser.child(if scatter { "c:xVal" } else { "c:cat" }))?;
        let values = cache_points(ser.child(if scatter { "c:yVal" } else { "c:val" }))?;
        let complete = labels.complete && values.complete;
        let mut points = std::collections::BTreeMap::new();
        for (idx, label) in labels.points {
            let label = if scatter {
                // Invalid X labels keep the existing scatter-validation error category.
                let value = label
                    .parse::<f64>()
                    .ok()
                    .filter(|v| v.is_finite())
                    .ok_or_else(|| {
                        Error::Input(crate::Msg::new(
                            crate::MessageCode::ChartScatterData,
                            Vec::new(),
                        ))
                    })?;
                if value == 0.0 {
                    "0".into()
                } else {
                    value.to_string()
                }
            } else {
                label
            };
            points.insert(
                idx,
                CachedPoint {
                    label: (!label.trim().is_empty()).then_some(label),
                    value: None,
                },
            );
        }
        for (idx, value) in values.points {
            let value = value
                .parse::<f64>()
                .map_err(|source| Error::ChartData { source })?;
            if !value.is_finite() {
                return Err(Error::Input(crate::Msg::new(
                    crate::MessageCode::ChartEmpty,
                    Vec::new(),
                )));
            }
            points
                .entry(idx)
                .or_insert(CachedPoint {
                    label: None,
                    value: None,
                })
                .value = Some(value);
        }
        let name = ser
            .child("c:tx")
            .and_then(|tx| crate::ooxml::descendants(tx, "c:v").first().copied())
            .map(|v| v.text())
            .unwrap_or_default();
        out.push(CachedSeries {
            name,
            scatter,
            complete,
            points,
        });
    }
    Ok(out)
}

pub(crate) fn unsupported_types(chart: &crate::xmldom::El) -> Vec<String> {
    chart
        .child("c:chart")
        .and_then(|c| c.child("c:plotArea"))
        .into_iter()
        .flat_map(|p| p.elements())
        .filter_map(|group| {
            let name = group.name.strip_prefix("c:")?;
            (name.ends_with("Chart")
                && !matches!(
                    name,
                    "barChart" | "lineChart" | "pieChart" | "areaChart" | "scatterChart"
                ))
            .then(|| name.to_string())
        })
        .collect()
}

/// Read cached data from the same bytes used by Office when its workbook is not open.
/// Incomplete pairs cannot be represented by the dense public Chart, so fail explicitly.
#[cfg(test)]
pub fn read(bytes: &[u8]) -> Result<Option<Chart>, Error> {
    Ok(read_all(bytes)?.into_iter().next())
}

/// Keep each plot group as a separate chart block; the public model has one kind per chart.
pub(crate) fn read_all(bytes: &[u8]) -> Result<Vec<Chart>, Error> {
    let dom = crate::xmldom::parse(bytes)?;
    let Some(plot) = dom.child("c:chart").and_then(|c| c.child("c:plotArea")) else {
        return Ok(Vec::new());
    };
    let unsupported = unsupported_types(&dom);
    if !unsupported.is_empty() {
        return Err(Error::Unsupported(crate::Msg::new(
            crate::MessageCode::ReadChartUnsupported,
            crate::messages::params(&[("type", &unsupported.join(", "))]),
        )));
    }
    let mut charts = Vec::new();
    for group in plot.elements() {
        let kind = match group.name.as_str() {
            "c:barChart" => ChartKind::Bar,
            "c:lineChart" => ChartKind::Line,
            "c:pieChart" => ChartKind::Pie,
            "c:areaChart" => ChartKind::Area,
            "c:scatterChart" => ChartKind::Scatter,
            _ => continue,
        };
        charts.push(read_group(group, kind)?);
    }
    Ok(charts)
}

fn read_group(group: &crate::xmldom::El, kind: ChartKind) -> Result<Chart, Error> {
    let kind = if kind == ChartKind::Bar
        && group.child("c:barDir").and_then(|d| d.attr("val")) == Some("col")
    {
        ChartKind::Column
    } else {
        kind
    };
    let mut labels = Vec::new();
    let mut indices = Vec::new();
    let mut series = Vec::new();
    for ser in cached_series(group)? {
        if !ser.complete {
            return Err(Error::Input(crate::Msg::new(
                crate::MessageCode::ReadChartIncomplete,
                Vec::new(),
            )));
        }
        let mut current_labels = Vec::new();
        let mut current_indices = Vec::new();
        let mut values = Vec::new();
        for (idx, point) in ser.points {
            let (Some(label), Some(value)) = (point.label, point.value) else {
                return Err(Error::Input(crate::Msg::new(
                    crate::MessageCode::ReadChartIncomplete,
                    Vec::new(),
                )));
            };
            current_indices.push(idx);
            current_labels.push(label);
            values.push(value);
        }
        if series.is_empty() {
            labels = current_labels;
            indices = current_indices;
        } else if labels != current_labels || indices != current_indices {
            return Err(Error::Input(crate::Msg::new(
                crate::MessageCode::ReadChartIncomplete,
                Vec::new(),
            )));
        }
        series.push(crate::Series {
            name: ser.name,
            values,
        });
    }
    Ok(Chart {
        kind,
        labels,
        series,
    })
}

pub fn workbook(chart: &Chart) -> Result<Vec<u8>, Error> {
    let mut sheet = Xml::new();
    sheet.decl();
    sheet.open("worksheet", &[("xmlns", S)]);
    sheet.open("sheetData", &[]);
    for row in 0..=chart.labels.len() {
        sheet.open("row", &[("r", &(row + 1).to_string())]);
        for col in 0..=chart.series.len() {
            let cell = format!("{}{}", column(col), row + 1);
            let text = row == 0 || (col == 0 && chart.kind != ChartKind::Scatter);
            let v = if row == 0 {
                if col == 0 {
                    String::new()
                } else {
                    chart.series[col - 1].name.clone()
                }
            } else if col == 0 {
                chart.labels[row - 1].clone()
            } else {
                chart.series[col - 1].values[row - 1].to_string()
            };
            sheet.open(
                "c",
                &[("r", &cell), ("t", if text { "inlineStr" } else { "n" })],
            );
            if text {
                sheet.open("is", &[]);
                sheet.el("t", &[], &v);
                sheet.close("is");
            } else {
                sheet.el("v", &[], &v);
            }
            sheet.close("c");
        }
        sheet.close("row");
    }
    sheet.close("sheetData");
    sheet.close("worksheet");
    let parts = [
        ("[Content_Types].xml", br#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>"#.to_vec()),
        ("_rels/.rels", format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{R}/officeDocument" Target="xl/workbook.xml"/></Relationships>"#).into_bytes()),
        ("xl/workbook.xml", format!(r#"<workbook xmlns="{S}" xmlns:r="{R}"><sheets><sheet name="Sheet1" sheetId="1" r:id="rId1"/></sheets></workbook>"#).into_bytes()),
        ("xl/_rels/workbook.xml.rels", format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{R}/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#).into_bytes()),
        ("xl/worksheets/sheet1.xml", sheet.finish()),
    ];
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes) in parts {
        zip.start_file(
            name,
            SimpleFileOptions::default().last_modified_time(zip::DateTime::default()),
        )
        .map_err(Error::zip)?;
        zip.write_all(&bytes).map_err(Error::zip)?;
    }
    Ok(zip.finish().map_err(Error::zip)?.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scatter_cache_validates_x_coordinates() -> Result<(), Box<dyn std::error::Error>> {
        let chart = Chart {
            kind: ChartKind::Scatter,
            labels: vec!["1.5".into()],
            series: vec![crate::Series {
                name: "Y".into(),
                values: vec![2.0],
            }],
        };
        let bytes = xml(&chart, crate::deck::theme("light"));
        assert!(read(&bytes)?.is_some());
        let invalid = String::from_utf8(bytes)?.replace("<c:v>1.5</c:v>", "<c:v>NaN</c:v>");
        let error = read(invalid.as_bytes())
            .err()
            .ok_or("expected invalid X rejection")?;
        assert_eq!(error.code(), "input.chart_scatter_data");
        Ok(())
    }
}
