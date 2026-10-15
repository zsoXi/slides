//! Indexed chart caches, semantic blocks and axis-independent table checks.
use serde_json::{Value, json};
use slides::{Input, Options, Output, run};
use std::collections::BTreeMap;
use std::io::{Cursor, Read, Write};
use std::path::Path;
type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;
type Parts = BTreeMap<String, Vec<u8>>;
fn call(dir: &Path, request: Value) -> Result<Output> {
    Ok(run(
        &serde_json::from_value::<Input>(request)?,
        &Options::new(dir),
    )?)
}
fn parts(path: &Path) -> Result<Parts> {
    let mut zip = zip::ZipArchive::new(Cursor::new(std::fs::read(path)?))?;
    let mut out = Parts::new();
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes)?;
        out.insert(entry.name()?.to_string(), bytes);
    }
    Ok(out)
}
fn pack(path: &Path, parts: &Parts) -> Result {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes) in parts {
        zip.start_file(name, zip::write::SimpleFileOptions::default())?;
        zip.write_all(bytes)?;
    }
    std::fs::write(path, zip.finish()?.into_inner())?;
    Ok(())
}
fn xml<'a>(parts: &'a Parts, name: &str) -> Result<&'a str> {
    Ok(std::str::from_utf8(parts.get(name).ok_or("missing part")?)?)
}
fn reverse_points(xml: &str, tag: &str) -> Result<String> {
    let start = xml.find(&format!("<c:{tag}>")).ok_or("no cache")?;
    let end = xml[start..]
        .find(&format!("</c:{tag}>"))
        .ok_or("no cache end")?
        + start;
    let region = &xml[start..end];
    let first = region.find("<c:pt idx=").ok_or("no points")?;
    let last = region.rfind("</c:pt>").ok_or("no point end")? + "</c:pt>".len();
    let points = region[first..last]
        .split_inclusive("</c:pt>")
        .collect::<Vec<_>>();
    Ok(format!(
        "{}{}{}{}",
        &xml[..start + first],
        points.into_iter().rev().collect::<String>(),
        &region[last..],
        &xml[end..]
    ))
}
#[test]
fn read_export_and_qc_join_both_chart_axes_by_index() -> Result {
    let dir = tempfile::tempdir()?;
    for kind in ["column", "scatter"] {
        let labels = if kind == "scatter" {
            json!(["1", "2", "3"])
        } else {
            json!(["North", "South", "West"])
        };
        call(
            dir.path(),
            json!({"op":"create","output":"base.pptx","slides":[
                {"title":"First","chart":{"type":kind,"labels":labels,"series":[{"name":"Revenue","values":[10,20,30]}]}},
                {"title":"Second","chart":{"type":kind,"labels":labels,"series":[{"name":"Revenue","values":[10,20,30]}]}}
            ]}),
        )?;
        let baseline = call(dir.path(), json!({"op":"read","input":"base.pptx"}))?;
        let base = parts(&dir.path().join("base.pptx"))?;
        for (reverse_x, reverse_y) in [(true, false), (false, true), (true, true)] {
            let mut p = base.clone();
            let name = "ppt/charts/chart2.xml";
            let mut chart = xml(&p, name)?.to_string();
            if reverse_x {
                chart = reverse_points(&chart, if kind == "scatter" { "xVal" } else { "cat" })?;
            }
            if reverse_y {
                chart = reverse_points(&chart, if kind == "scatter" { "yVal" } else { "val" })?;
            }
            p.insert(name.into(), chart.into_bytes());
            pack(&dir.path().join("order.pptx"), &p)?;
            let read = call(dir.path(), json!({"op":"read","input":"order.pptx"}))?;
            assert_eq!(
                read.data, baseline.data,
                "{kind}, x={reverse_x}, y={reverse_y}"
            );
            assert_eq!(read.text, baseline.text);
            call(
                dir.path(),
                json!({"op":"convert","input":"order.pptx","output":"order.md"}),
            )?;
            assert_eq!(
                std::fs::read_to_string(dir.path().join("order.md"))?,
                baseline.text.clone().ok_or("no text")?
            );
            let checked = call(dir.path(), json!({"op":"check","input":"order.pptx"}))?;
            assert_eq!(checked.data, Some(json!([])));
        }
    }
    Ok(())
}
#[test]
fn incomplete_chart_cache_is_not_paired_by_position_or_filled_with_zero() -> Result {
    let dir = tempfile::tempdir()?;
    call(
        dir.path(),
        json!({"op":"create","output":"base.pptx","slides":[{"title":"Regions","chart":{"type":"column","labels":["North","South","West"],"series":[{"name":"Revenue","values":[10,20,30]}]}}]}),
    )?;
    let mut p = parts(&dir.path().join("base.pptx"))?;
    let name = "ppt/charts/chart1.xml";
    let chart = xml(&p, name)?.replace("<c:pt idx=\"1\"><c:v>South</c:v></c:pt>", "");
    for both_axes in [false, true] {
        let chart = if both_axes {
            chart.replace("<c:pt idx=\"1\"><c:v>20</c:v></c:pt>", "")
        } else {
            chart.clone()
        };
        p.insert(name.into(), chart.into_bytes());
        pack(&dir.path().join("missing.pptx"), &p)?;
        for request in [
            json!({"op":"read","input":"missing.pptx"}),
            json!({"op":"convert","input":"missing.pptx","output":"missing.md"}),
        ] {
            let result = call(dir.path(), request);
            let error = result
                .err()
                .ok_or("incomplete cache must not imply a complete extraction")?;
            assert_eq!(
                error
                    .downcast_ref::<slides::Error>()
                    .ok_or("not a slides error")?
                    .code(),
                "read.chart_incomplete"
            );
        }
        let checked = call(dir.path(), json!({"op":"check","input":"missing.pptx"}))?;
        assert_eq!(
            checked.summary_params["numbers"],
            if both_axes { "2" } else { "3" }
        );
    }
    Ok(())
}
#[test]
fn unsupported_chart_types_warn_in_read_convert_and_check() -> Result {
    let dir = tempfile::tempdir()?;
    call(
        dir.path(),
        json!({"op":"create","output":"base.pptx","slides":[{"title":"Mix","chart":{"type":"pie","labels":["North","South"],"series":[{"name":"Revenue","values":[10,20]}]}}]}),
    )?;
    let baseline = call(dir.path(), json!({"op":"read","input":"base.pptx"}))?;
    assert!(baseline.warnings.is_empty());
    let mut p = parts(&dir.path().join("base.pptx"))?;
    let name = "ppt/charts/chart1.xml";
    let chart = xml(&p, name)?.replace("c:pieChart", "c:doughnutChart");
    p.insert(name.into(), chart.into_bytes());
    pack(&dir.path().join("doughnut.pptx"), &p)?;
    for request in [
        json!({"op":"read","input":"doughnut.pptx"}),
        json!({"op":"convert","input":"doughnut.pptx","output":"doughnut.md"}),
        json!({"op":"check","input":"doughnut.pptx"}),
    ] {
        let out = call(dir.path(), request)?;
        let warning = out
            .warnings
            .iter()
            .find(|w| w.code == "read.chart_unsupported")
            .ok_or("missing chart warning")?;
        assert_eq!(warning.params["slide"], "1");
        assert_eq!(warning.params["type"], "doughnutChart");
    }
    Ok(())
}
fn company(dir: &Path) -> Result {
    call(
        dir,
        json!({"op":"create","output":"template.pptx","slides":[{"title":"Company","bullets":["Body"]}]}),
    )?;
    let mut p = parts(&dir.join("template.pptx"))?;
    for (name, bytes) in p.iter_mut() {
        if name.contains("slideLayouts/") && name.ends_with(".xml") {
            *bytes = String::from_utf8(bytes.clone())?
                .replace("Content Placeholder", "Company body")
                .replace("+mn-lt", "Liberation Sans")
                .into_bytes();
        }
    }
    pack(&dir.join("template.pptx"), &p)
}
#[test]
fn semantic_code_survives_templates_placeholder_rename_and_edit_add() -> Result {
    let dir = tempfile::tempdir()?;
    company(dir.path())?;
    let code = "\nmessage = \"\"\"First line\n\nThird line\"\"\"\n\nprint(message)\n\n";
    let markdown = format!("## Python\n\n```python\n{code}```\n");
    for template in [false, true] {
        let mut request = json!({"op":"create","output":"code.pptx","markdown":markdown});
        if template {
            request["template"] = json!("template.pptx");
        }
        call(dir.path(), request)?;
        // The private semantic marker, not the mutable Office shape name, owns the block type.
        let mut p = parts(&dir.path().join("code.pptx"))?;
        for (name, bytes) in p.iter_mut() {
            if name.contains("slides/slide") && name.ends_with(".xml") {
                *bytes = String::from_utf8(bytes.clone())?
                    .replace("name=\"Code\"", "name=\"Renamed body\"")
                    .into_bytes();
            }
        }
        pack(&dir.path().join("code.pptx"), &p)?;
        for input in ["code.pptx", "added.pptx"] {
            if input == "added.pptx" {
                call(
                    dir.path(),
                    json!({"op":"edit","input":"template.pptx","output":input,"add":[{"title":"Python","code":code}]}),
                )?;
            }
            let read = call(dir.path(), json!({"op":"read","input":input}))?;
            let data = read.data.ok_or("no data")?;
            let slide = data
                .as_array()
                .ok_or("not array")?
                .last()
                .ok_or("no slide")?;
            assert_eq!(slide["text"], code, "template={template}, {input}");
            assert!(read.text.ok_or("no text")?.contains(code));
            call(
                dir.path(),
                json!({"op":"convert","input":input,"output":"code.md"}),
            )?;
            assert!(std::fs::read_to_string(dir.path().join("code.md"))?.contains(code));
            let p = parts(&dir.path().join(input))?;
            let slides = p
                .iter()
                .filter(|(n, _)| n.contains("slides/slide") && n.ends_with(".xml"))
                .map(|(_, b)| String::from_utf8_lossy(b))
                .collect::<Vec<_>>();
            let code_xml = slides
                .iter()
                .find(|s| s.contains("First line"))
                .ok_or("no code XML")?;
            assert!(code_xml.contains("urn:slides:block:v1"));
            assert!(
                code_xml.contains("typeface=\"DejaVu Sans Mono\""),
                "{code_xml}"
            );
            let shape = code_xml
                .split("<p:sp>")
                .find(|s| s.contains("First line"))
                .ok_or("no code shape")?;
            for props in shape.split("<a:rPr").skip(1) {
                let props = props.split("</a:rPr>").next().ok_or("no properties end")?;
                assert!(props.contains("typeface=\"DejaVu Sans Mono\""), "{props}");
            }
        }
    }
    Ok(())
}
#[test]
fn tables_exclude_totals_and_require_compatible_units_on_either_axis() -> Result {
    let dir = tempfile::tempdir()?;
    // 18 cases (9 data sets transposed). Both axes use the same component/total contract.
    for (headers, rows, expected_total, expected_percent) in [
        (
            json!(["Period", "Subscription", "Services", "Total"]),
            json!([
                ["2027", "70%", "30%", "100%"],
                ["2028", "60%", "40%", "100%"]
            ]),
            0,
            0,
        ),
        (
            json!(["Period", "Subscription", "Services", "Suma"]),
            json!([
                ["2027", "70%", "30%", "100%"],
                ["2028", "60%", "40%", "100%"]
            ]),
            0,
            0,
        ),
        (
            json!(["Period", "Subscription", "Services", "Total"]),
            json!([
                ["2027", "70%", "20%", "100%"],
                ["2028", "60%", "40%", "100%"]
            ]),
            1,
            1,
        ),
        (
            json!(["Region", "Product", "Services", "Total"]),
            json!([["North", "$10", "€20", "$31"]]),
            0,
            0,
        ),
        (
            json!(["Region", "Product", "Services", "Total"]),
            json!([["North", "$10", "$20", "$31"]]),
            1,
            0,
        ),
        (
            json!(["Region", "Product", "Services", "Total"]),
            json!([["North", "$10", "$20", "€31"]]),
            0,
            0,
        ),
        (
            json!(["Region", "Product", "Services", "Suma"]),
            json!([["North", "10", "20", "31"]]),
            1,
            0,
        ),
        (
            json!(["Region", "Total", "Product", "Services"]),
            json!([["North", "31", "10", "20"]]),
            1,
            0,
        ),
        (
            json!(["Region", "Product", "Total", "Services", "Grand total"]),
            json!([["North", "10", "30", "20", "30"]]),
            0,
            0,
        ),
    ] {
        let mut matrix = vec![headers.as_array().ok_or("headers")?.clone()];
        matrix.extend(
            rows.as_array()
                .ok_or("rows")?
                .iter()
                .map(|r| r.as_array().cloned().unwrap_or_default()),
        );
        for transpose in [false, true] {
            let m = if transpose {
                (0..matrix[0].len())
                    .map(|c| matrix.iter().map(|r| r[c].clone()).collect::<Vec<_>>())
                    .collect::<Vec<_>>()
            } else {
                matrix.clone()
            };
            call(
                dir.path(),
                json!({"op":"create","output":"table.pptx","slides":[{"title":"Results","table":{"headers":m[0],"rows":m[1..]}}]}),
            )?;
            let checked = call(dir.path(), json!({"op":"check","input":"table.pptx"}))?;
            let data = checked.data.ok_or("no findings")?;
            let findings = data.as_array().ok_or("not array")?;
            for (issue, count) in [
                ("check.table_total", expected_total),
                ("check.table_percent", expected_percent),
            ] {
                assert_eq!(
                    findings.iter().filter(|f| f["issue"] == issue).count(),
                    count,
                    "transpose={transpose}, {headers}, {rows}: {data}"
                );
            }
        }
    }
    Ok(())
}
#[test]
fn label_value_separators_preserve_periods_and_real_year_range_values() -> Result {
    let dir = tempfile::tempdir()?;
    // 16 cases: different periods, same-period contradictions and actual year-range values.
    for separator in [":", "=", "–", "\t"] {
        for (first, second, numbers, mismatches) in [
            (
                format!("Revenue 2027 {separator} $25 mln"),
                format!("Revenue 2028 {separator} $26 mln"),
                "2",
                0,
            ),
            (
                format!("Revenue 2027 {separator} $25 mln"),
                format!("Revenue 2027 {separator} $26 mln"),
                "2",
                1,
            ),
            (
                format!("Headcount {separator} 2027"),
                format!("Headcount {separator} 2028"),
                "2",
                1,
            ),
            (
                format!("Revenue Q1 2027 {separator} $25 mln"),
                format!("Revenue Q2 2027 {separator} $26 mln"),
                "2",
                0,
            ),
        ] {
            call(
                dir.path(),
                json!({"op":"create","output":"periods.pptx","slides":[{"title":"First","text":first},{"title":"Second","text":second}]}),
            )?;
            let checked = call(dir.path(), json!({"op":"check","input":"periods.pptx"}))?;
            assert_eq!(
                checked.summary_params["numbers"], numbers,
                "{first}, {second}"
            );
            let data = checked.data.ok_or("no data")?;
            assert_eq!(
                data.as_array()
                    .ok_or("not array")?
                    .iter()
                    .filter(|f| f["issue"] == "check.metric_mismatch")
                    .count(),
                mismatches,
                "{first}, {second}: {data}"
            );
        }
    }
    Ok(())
}

#[test]
fn native_block_metadata_survives_company_creation_and_additions() -> Result {
    let dir = tempfile::tempdir()?;
    company(dir.path())?;
    let slide = json!({"title":"Results","table":{"headers":["Region","Value"],"rows":[["North","10"]]},"chart":{"type":"column","labels":["North"],"series":[{"name":"Revenue","values":[10]}]}});
    call(
        dir.path(),
        json!({"op":"create","template":"template.pptx","output":"native.pptx","slides":[slide]}),
    )?;
    call(
        dir.path(),
        json!({"op":"edit","input":"native.pptx","output":"added.pptx","add":[slide]}),
    )?;
    for (input, count) in [("native.pptx", 1), ("added.pptx", 2)] {
        let p = parts(&dir.path().join(input))?;
        let xml = p
            .iter()
            .filter(|(name, _)| name.contains("slides/slide") && name.ends_with(".xml"))
            .map(|(_, bytes)| String::from_utf8_lossy(bytes))
            .collect::<Vec<_>>()
            .join("\n");
        for kind in ["table", "chart"] {
            assert_eq!(
                xml.matches(&format!("kind=\"{kind}\"")).count(),
                count,
                "{input}"
            );
        }
        let read = call(dir.path(), json!({"op":"read","input":input}))?;
        let data = read.data.ok_or("no data")?;
        assert_eq!(data.as_array().ok_or("not array")?.len(), count);
        for slide in data.as_array().ok_or("not array")? {
            assert!(
                slide["text"]
                    .as_str()
                    .ok_or("no text")?
                    .contains("Revenue\t10")
            );
        }
    }
    Ok(())
}

#[test]
fn duplicate_chart_indices_are_rejected_before_export_overwrite() -> Result {
    let dir = tempfile::tempdir()?;
    call(
        dir.path(),
        json!({"op":"create","output":"base.pptx","slides":[{"title":"Results","chart":{"type":"column","labels":["North","South"],"series":[{"name":"Revenue","values":[10,20]}]}}]}),
    )?;
    let mut p = parts(&dir.path().join("base.pptx"))?;
    let name = "ppt/charts/chart1.xml";
    let chart = xml(&p, name)?.replace("<c:pt idx=\"1\"><c:v>South", "<c:pt idx=\"0\"><c:v>South");
    p.insert(name.into(), chart.into_bytes());
    pack(&dir.path().join("bad.pptx"), &p)?;
    std::fs::write(dir.path().join("keep.md"), "previous")?;
    for request in [
        json!({"op":"read","input":"bad.pptx"}),
        json!({"op":"convert","input":"bad.pptx","output":"keep.md"}),
        json!({"op":"check","input":"bad.pptx"}),
    ] {
        let error = call(dir.path(), request)
            .err()
            .ok_or("duplicate indices accepted")?;
        assert_eq!(
            error
                .downcast_ref::<slides::Error>()
                .ok_or("not slides error")?
                .code(),
            "package.invalid"
        );
    }
    assert_eq!(
        std::fs::read_to_string(dir.path().join("keep.md"))?,
        "previous"
    );
    Ok(())
}
