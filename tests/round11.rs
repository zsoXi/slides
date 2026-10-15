//! Strict OOXML, safe conversions, numeric roles and lossless text exports.
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
fn cli(dir: &Path, request: Value) -> Result<(bool, Value)> {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_slides"))
        .current_dir(dir)
        .arg(request.to_string())
        .output()?;
    Ok((out.status.success(), serde_json::from_slice(&out.stdout)?))
}
fn strict(parts: &mut Parts) -> Result {
    for (path, bytes) in parts.iter_mut() {
        if path.ends_with(".xml") || path.ends_with(".rels") {
            let mut xml = String::from_utf8(bytes.clone())?;
            for (old, new) in [
                (
                    "http://schemas.openxmlformats.org/presentationml/2006/main",
                    "http://purl.oclc.org/ooxml/presentationml/main",
                ),
                (
                    "http://schemas.openxmlformats.org/drawingml/2006/main",
                    "http://purl.oclc.org/ooxml/drawingml/main",
                ),
                (
                    "http://schemas.openxmlformats.org/drawingml/2006/chart",
                    "http://purl.oclc.org/ooxml/drawingml/chart",
                ),
                (
                    "http://schemas.openxmlformats.org/officeDocument/2006/relationships",
                    "http://purl.oclc.org/ooxml/officeDocument/relationships",
                ),
                (
                    "http://schemas.openxmlformats.org/spreadsheetml/2006/main",
                    "http://purl.oclc.org/ooxml/spreadsheetml/main",
                ),
            ] {
                xml = xml.replace(old, new);
            }
            *bytes = xml.into_bytes();
        }
    }
    Ok(())
}
#[test]
fn strict_namespaces_preserve_read_check_and_export() -> Result {
    let dir = tempfile::tempdir()?;
    call(
        dir.path(),
        json!({"op":"create","output":"base.pptx","slides":[
            {"title":"Northstar Analytics","text":"Revenue: 25","notes":"Keep this note"},
            {"title":"Results","table":{"headers":["Region","Share"],"rows":[["North","40%"],["South","60%"]]},"chart":{"type":"column","labels":["Q1"],"series":[{"name":"Revenue","values":[25]}]}}
        ]}),
    )?;
    let baseline = call(dir.path(), json!({"op":"read","input":"base.pptx"}))?;
    let mut p = parts(&dir.path().join("base.pptx"))?;
    strict(&mut p)?;
    pack(&dir.path().join("strict.pptx"), &p)?;
    let read = call(dir.path(), json!({"op":"read","input":"strict.pptx"}))?;
    assert_eq!(read.data, baseline.data);
    assert_eq!(read.text, baseline.text);
    assert!(read.warnings.is_empty());
    assert_eq!(
        call(dir.path(), json!({"op":"check","input":"strict.pptx"}))?.data,
        call(dir.path(), json!({"op":"check","input":"base.pptx"}))?.data
    );
    call(
        dir.path(),
        json!({"op":"convert","input":"strict.pptx","output":"strict.md"}),
    )?;
    assert_eq!(
        std::fs::read_to_string(dir.path().join("strict.md"))?,
        read.text.ok_or("missing text")?
    );
    Ok(())
}
#[test]
fn unrecognized_presentation_or_slide_tree_fails_without_writing() -> Result {
    let dir = tempfile::tempdir()?;
    call(
        dir.path(),
        json!({"op":"create","output":"base.pptx","markdown":"# Keep me"}),
    )?;
    let original = parts(&dir.path().join("base.pptx"))?;
    for part in ["ppt/presentation.xml", "ppt/slides/slide1.xml"] {
        let mut p = original.clone();
        let xml = String::from_utf8(p.get(part).ok_or("part missing")?.clone())?;
        p.insert(
            part.into(),
            xml.replace(
                "http://schemas.openxmlformats.org/presentationml/2006/main",
                "urn:unknown-presentation",
            )
            .into_bytes(),
        );
        pack(&dir.path().join("unknown.pptx"), &p)?;
        std::fs::write(dir.path().join("keep.md"), b"previous")?;
        for request in [
            json!({"op":"read","input":"unknown.pptx"}),
            json!({"op":"check","input":"unknown.pptx"}),
            json!({"op":"convert","input":"unknown.pptx","output":"keep.md"}),
        ] {
            let (ok, error) = cli(dir.path(), request)?;
            assert!(!ok, "{part}: {error}");
            assert_eq!(error["summary_code"], "package.invalid");
        }
        assert_eq!(std::fs::read(dir.path().join("keep.md"))?, b"previous");
    }
    Ok(())
}
#[test]
fn convert_rejects_same_inode_and_undocumented_pairs_before_writing() -> Result {
    let dir = tempfile::tempdir()?;
    call(
        dir.path(),
        json!({"op":"create","output":"base.pptx","markdown":"# Keep me"}),
    )?;
    let original = std::fs::read(dir.path().join("base.pptx"))?;
    std::fs::hard_link(dir.path().join("base.pptx"), dir.path().join("alias.md"))?;
    for output in ["alias.md", "base.pptx", "copy.pptx"] {
        let (ok, error) = cli(
            dir.path(),
            json!({"op":"convert","input":"base.pptx","output":output}),
        )?;
        assert!(!ok, "{error}");
        assert_eq!(
            error["summary_code"],
            if output == "copy.pptx" {
                "convert.unsupported_pair"
            } else {
                "convert.same_file"
            }
        );
        assert_eq!(std::fs::read(dir.path().join("base.pptx"))?, original);
    }
    assert!(!dir.path().join("copy.pptx").exists());
    std::fs::write(dir.path().join("input.md"), b"# Keep me")?;
    let (ok, error) = cli(
        dir.path(),
        json!({"op":"convert","input":"input.md","output":"copy.md"}),
    )?;
    assert!(!ok);
    assert_eq!(error["summary_code"], "convert.unsupported_pair");
    assert!(!dir.path().join("copy.md").exists());
    let (ok, error) = cli(
        dir.path(),
        json!({"op":"convert","input":"input.md","output":"copy.csv"}),
    )?;
    assert!(!ok);
    assert_eq!(error["summary_code"], "unsupported.convert_output");
    assert!(!dir.path().join("copy.csv").exists());
    Ok(())
}
#[test]
fn all_year_range_values_remain_table_chart_and_model_measurements() -> Result {
    let dir = tempfile::tempdir()?;
    let mut slides = Vec::new();
    for value in [2027, 2028] {
        slides.push(json!({"title":"Staffing","table":{"headers":["Metric","Value"],"rows":[["Headcount",value.to_string()]]},"sources":["Source: model.xlsx, DCF!B2"]}));
        slides.push(json!({"title":"Staffing","chart":{"type":"column","labels":["Employees"],"series":[{"name":"Headcount","values":[value]}]},"sources":["Source: model.xlsx, DCF!B2"]}));
    }
    call(
        dir.path(),
        json!({"op":"create","output":"base.pptx","slides":slides}),
    )?;
    let original = parts(&dir.path().join("base.pptx"))?;
    let mut model = Parts::new();
    model.insert("xl/workbook.xml".into(), b"<workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"><sheets><sheet name=\"DCF\" r:id=\"r1\"/></sheets></workbook>".to_vec());
    model.insert("xl/_rels/workbook.xml.rels".into(), b"<Relationships><Relationship Id=\"r1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet1.xml\"/></Relationships>".to_vec());
    for value in 1900..=2200 {
        let mut p = original.clone();
        for (path, bytes) in p.iter_mut() {
            if path.ends_with(".xml") {
                *bytes = String::from_utf8(bytes.clone())?
                    .replace(">2027<", ">FIRST<")
                    .replace(">2028<", &format!(">{}<", value + 1))
                    .replace(">FIRST<", &format!(">{value}<"))
                    .into_bytes();
            }
        }
        pack(&dir.path().join("values.pptx"), &p)?;
        model.insert("xl/worksheets/sheet1.xml".into(), format!("<worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><sheetData><row><c r=\"A2\" t=\"inlineStr\"><is><t>Headcount</t></is></c><c r=\"B2\"><v>{value}</v></c></row></sheetData></worksheet>").into_bytes());
        let mut export_model = model.clone();
        if value % 2 == 0 {
            strict(&mut export_model)?;
        }
        pack(&dir.path().join("model.xlsx"), &export_model)?;
        let checked = call(
            dir.path(),
            json!({"op":"check","input":"values.pptx","model":"model.xlsx"}),
        )?;
        let data = checked.data.ok_or("no findings")?;
        let findings = data.as_array().ok_or("not array")?;
        assert_eq!(checked.summary_params["numbers"], "4", "{value}: {data}");
        for issue in ["check.metric_mismatch", "check.model_mismatch"] {
            assert_eq!(
                findings.iter().filter(|f| f["issue"] == issue).count(),
                2,
                "{value}: {data}"
            );
        }
        assert!(
            !checked
                .warnings
                .iter()
                .any(|w| w.code == "check.model_unlinked"),
            "{value}"
        );
    }
    Ok(())
}
#[test]
fn prose_measurements_without_a_separator_are_not_classified_by_value() -> Result {
    let dir = tempfile::tempdir()?;
    call(
        dir.path(),
        json!({"op":"create","output":"staff.pptx","slides":[
            {"title":"2027","text":"Headcount 2027"},
            {"title":"2028","text":"Headcount 2028"}
        ]}),
    )?;
    let checked = call(dir.path(), json!({"op":"check","input":"staff.pptx"}))?;
    assert_eq!(checked.summary_params["numbers"], "2");
    let data = checked.data.ok_or("no findings")?;
    assert_eq!(data.as_array().ok_or("not array")?.len(), 1);
    assert_eq!(data[0]["issue"], "check.metric_mismatch");
    assert_eq!(data[0]["value"], 2028.0);
    assert_eq!(data[0]["expected"], 2027.0);
    Ok(())
}

#[test]
fn percentage_partition_axis_comes_from_periods_or_share_headers() -> Result {
    let dir = tempfile::tempdir()?;
    for (headers, rows, count) in [
        (
            json!(["Region", "2027", "2028"]),
            json!([["North", "40%", "45%"], ["South", "60%", "55%"]]),
            0,
        ),
        (
            json!(["Region", "2027", "2028"]),
            json!([["North", "40%", "45%"], ["South", "50%", "55%"]]),
            1,
        ),
        (
            json!(["Period", "North", "South"]),
            json!([["2027", "40%", "60%"], ["2028", "45%", "55%"]]),
            0,
        ),
        (
            json!(["Period", "North", "South"]),
            json!([["2027", "40%", "50%"], ["2028", "45%", "55%"]]),
            1,
        ),
        (
            json!(["Region", "Share"]),
            json!([["North", "40%"], ["South", "50%"]]),
            1,
        ),
        (
            json!(["Metric", "North", "South"]),
            json!([["A", "40%", "45%"], ["B", "50%", "55%"]]),
            0,
        ),
        (
            json!(["Period", "2027", "2028"]),
            json!([["2027", "40%", "45%"], ["2028", "50%", "55%"]]),
            0,
        ),
        (
            json!(["Region", "Q1 2027", "Q2 2027"]),
            json!([["North", "40%", "45%"], ["South", "60%", "55%"]]),
            0,
        ),
        (
            json!(["Region", "2027", "South"]),
            json!([["North", "40%", "45%"], ["South", "50%", "55%"]]),
            0,
        ),
        (
            json!(["Period", "North", "South"]),
            json!([["2027", "40%", "45%"], ["Other", "50%", "55%"]]),
            0,
        ),
    ] {
        call(
            dir.path(),
            json!({"op":"create","output":"shares.pptx","slides":[{"title":"Market mix","table":{"headers":headers,"rows":rows}}]}),
        )?;
        let checked = call(dir.path(), json!({"op":"check","input":"shares.pptx"}))?;
        let data = checked.data.ok_or("no data")?;
        assert_eq!(
            data.as_array()
                .ok_or("not array")?
                .iter()
                .filter(|f| f["issue"] == "check.table_percent")
                .count(),
            count,
            "{headers}: {data}"
        );
    }
    Ok(())
}
#[test]
fn literal_commonmark_round_trip_preserves_titles_body_and_table_cells() -> Result {
    let dir = tempfile::tempdir()?;
    let cases = [
        "Literal *asterisks*",
        "[brackets](path)",
        "<T>",
        "## Not a new slide",
        "First\nSecond",
        "**bold**",
        "_emphasis_",
        "`code`",
        "a | b",
        "a\\b",
        "&amp;",
        "&#35;",
        "![image](path)",
        "---",
        "***",
        "> quote",
        "- bullet",
        "+ bullet",
        "1. numbered",
        "1) numbered",
        "~~~",
        "```",
        "[link]: path",
        "<!-- layout: title -->",
        "# heading",
        "a\nb\nc",
        "a * b",
        "a_b_c",
        "trailing #",
        "https://example.invalid",
        "a\n\nb",
        "\\*escaped*",
        "<br>",
    ];
    for text in cases {
        call(
            dir.path(),
            json!({"op":"create","output":"literal.pptx","slides":[{"layout":"title_content","title":"Literal *asterisks*","text":text,"table":{"headers":["Literal [header]","Value"],"rows":[[text,"First\nSecond"]]}},{"title":"Last slide"}]}),
        )?;
        let first = call(dir.path(), json!({"op":"read","input":"literal.pptx"}))?
            .data
            .ok_or("no data")?;
        call(
            dir.path(),
            json!({"op":"convert","input":"literal.pptx","output":"literal.md"}),
        )?;
        call(
            dir.path(),
            json!({"op":"convert","input":"literal.md","output":"again.pptx"}),
        )?;
        let again = call(dir.path(), json!({"op":"read","input":"again.pptx"}))?
            .data
            .ok_or("no data")?;
        assert_eq!(
            again.as_array().ok_or("not array")?.len(),
            first.as_array().ok_or("not array")?.len(),
            "{text}: {again}"
        );
        for field in ["title", "text"] {
            assert_eq!(again[0][field], first[0][field], "{field}: {text}");
        }
    }
    Ok(())
}
#[test]
fn code_preserves_leading_internal_and_trailing_empty_paragraphs() -> Result {
    let dir = tempfile::tempdir()?;
    let code = "\nmessage = \"\"\"First line\n\nThird line\"\"\"\n\nprint(message)\n\n";
    call(
        dir.path(),
        json!({"op":"create","output":"code.pptx","markdown":format!("## Python\n\n```python\n{code}```\n")}),
    )?;
    let read = call(dir.path(), json!({"op":"read","input":"code.pptx"}))?;
    let text = read.text.ok_or("no text")?;
    assert!(text.contains(code), "{text:?}");
    let data = read.data.ok_or("no data")?;
    assert_eq!(data[0]["text"], code);
    call(
        dir.path(),
        json!({"op":"convert","input":"code.pptx","output":"code.md"}),
    )?;
    assert!(std::fs::read_to_string(dir.path().join("code.md"))?.contains(code));
    Ok(())
}
