//! Overflow, complete chart families and foreign-package preservation regressions.

use std::collections::BTreeMap;
use std::io::{Cursor, Read};
use std::path::Path;
use std::process::Command;

use serde_json::{Value, json};
use slides::{Input, Options, Output, run};

type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

fn call(base: &Path, request: Value) -> Result<Output> {
    Ok(run(
        &serde_json::from_value::<Input>(request)?,
        &Options::new(base),
    )?)
}

fn parts(bytes: &[u8]) -> Result<BTreeMap<String, Vec<u8>>> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes))?;
    let mut out = BTreeMap::new();
    for i in 0..zip.len() {
        let mut file = zip.by_index(i)?;
        let name = file.name()?.to_string();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        out.insert(name, bytes);
    }
    Ok(out)
}

fn text<'a>(p: &'a BTreeMap<String, Vec<u8>>, name: &str) -> Result<&'a str> {
    Ok(std::str::from_utf8(p.get(name).ok_or("missing part")?)?)
}

fn compact(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}

#[test]
fn long_text_is_split_without_losing_content_and_summary_counts_output() -> Result {
    let dir = tempfile::tempdir()?;
    let content = (0..700).map(|n| format!("item{n:04} ")).collect::<String>();
    let result = call(
        dir.path(),
        json!({"op":"create", "output":"long.pptx", "slides":[{"title":"A long lesson", "text":content, "notes":"Keep this note"}]}),
    )?;
    assert!(result.warnings.iter().any(|w| w.code == "slide.split"));
    assert!(
        result
            .warnings
            .iter()
            .any(|w| w.code == "slide.text_overflow")
    );
    let read = call(
        dir.path(),
        json!({"op":"read", "input":"long.pptx", "max_chars":100000}),
    )?;
    let data = read.data.ok_or("no data")?;
    let slides = data.as_array().ok_or("no slides")?;
    assert!(slides.len() > 1);
    assert_eq!(result.summary_params["count"], slides.len().to_string());
    let joined = slides
        .iter()
        .filter_map(|s| s["text"].as_str())
        .collect::<Vec<_>>()
        .join("");
    assert_eq!(compact(&joined), compact(&content));
    for s in &slides[1..] {
        assert!(s["title"].as_str().ok_or("no title")?.ends_with("(cont.)"));
    }
    assert_eq!(slides[0]["notes"], "Keep this note");
    assert!(slides[1..].iter().all(|s| s["notes"] == ""));
    // The same input is deterministic on consecutive and independent parallel calls.
    let original = std::fs::read(dir.path().join("long.pptx"))?;
    let mut archive = zip::ZipArchive::new(Cursor::new(&original))?;
    for i in 0..archive.len() {
        assert_eq!(
            archive.by_index(i)?.last_modified(),
            Some(zip::DateTime::default())
        );
    }
    call(
        dir.path(),
        json!({"op":"create", "output":"again.pptx", "slides":[{"title":"A long lesson", "text":content, "notes":"Keep this note"}]}),
    )?;
    assert_eq!(original, std::fs::read(dir.path().join("again.pptx"))?);
    std::thread::scope(|scope| -> Result {
        let handles = (0..2).map(|n| {
            let base = dir.path(); let content = &content;
            scope.spawn(move || call(base, json!({"op":"create", "output":format!("parallel{n}.pptx"), "slides":[{"title":"A long lesson", "text":content, "notes":"Keep this note"}]})).map(|_| ()).map_err(|e| e.to_string()))
        }).collect::<Vec<_>>();
        for h in handles {
            assert!(matches!(h.join(), Ok(Ok(()))));
        }
        Ok(())
    })?;
    for n in 0..2 {
        assert_eq!(
            original,
            std::fs::read(dir.path().join(format!("parallel{n}.pptx")))?
        );
    }
    Ok(())
}

#[test]
fn long_decks_have_no_arbitrary_continuation_limit() -> Result {
    let dir = tempfile::tempdir()?;
    let content = "W".repeat(40000);
    let result = call(
        dir.path(),
        json!({"op":"create", "output":"many.pptx", "slides":[{"title":"Large appendix", "text":content}]}),
    )?;
    let count = result.summary_params["count"]
        .as_str()
        .ok_or("no count")?
        .parse::<usize>()?;
    assert!(count > 32);
    let read = call(
        dir.path(),
        json!({"op":"read", "input":"many.pptx", "max_chars":100000}),
    )?;
    let data = read.data.ok_or("no data")?;
    let joined = data
        .as_array()
        .ok_or("no slides")?
        .iter()
        .filter_map(|s| s["text"].as_str())
        .collect::<String>();
    assert_eq!(joined, content);
    Ok(())
}

#[test]
fn long_tables_repeat_headers_and_preserve_every_row() -> Result {
    let dir = tempfile::tempdir()?;
    let rows = (0..100)
        .map(|n| vec![format!("ROW{n:03}"), format!("{}", n * 100)])
        .collect::<Vec<_>>();
    let result = call(
        dir.path(),
        json!({"op":"create", "output":"table.pptx", "slides":[{"title":"Ledger", "table":{"headers":["Item", "Value"], "rows":rows}}]}),
    )?;
    assert!(result.warnings.iter().any(|w| w.code == "slide.split"));
    let read = call(
        dir.path(),
        json!({"op":"read", "input":"table.pptx", "max_chars":100000}),
    )?;
    let data = read.data.ok_or("no data")?;
    let slides = data.as_array().ok_or("no slides")?;
    assert!(slides.len() > 1);
    let mut joined = String::new();
    for slide in slides {
        let text = slide["text"].as_str().ok_or("no text")?;
        assert!(text.starts_with("Item\tValue"));
        joined.push_str(text);
    }
    for n in 0..100 {
        assert_eq!(joined.matches(&format!("ROW{n:03}")).count(), 1);
    }
    Ok(())
}

#[test]
fn code_and_nested_lists_survive_markdown_and_code_paginate() -> Result {
    let dir = tempfile::tempdir()?;
    let code = (0..100)
        .map(|n| format!("    let item_{n:03} = {n};\n"))
        .collect::<String>();
    let md = format!(
        "## Steps\n- Parent\n  - Child\n    - Grandchild\n- Sibling\n\n> Instructor note\n\n## Code\n```rust\n{code}```\n"
    );
    let result = call(
        dir.path(),
        json!({"op":"create", "output":"lesson.pptx", "markdown":md}),
    )?;
    assert!(result.warnings.iter().any(|w| w.code == "slide.split"));
    let p = parts(&std::fs::read(dir.path().join("lesson.pptx"))?)?;
    let first = text(&p, "ppt/slides/slide1.xml")?;
    for token in [
        "Parent",
        "Child",
        "Grandchild",
        "Sibling",
        "lvl=\"1\"",
        "lvl=\"2\"",
    ] {
        assert!(first.contains(token), "{token}");
    }
    assert!(
        p.values()
            .any(|b| String::from_utf8_lossy(b).contains("DejaVu Sans Mono"))
    );
    let read = call(
        dir.path(),
        json!({"op":"read", "input":"lesson.pptx", "max_chars":100000}),
    )?;
    let data = read.data.ok_or("no data")?;
    assert_eq!(data[0]["notes"], "Instructor note");
    let all = data
        .as_array()
        .ok_or("no slides")?
        .iter()
        .filter_map(|s| s["text"].as_str())
        .collect::<Vec<_>>()
        .join("");
    for n in 0..100 {
        assert_eq!(all.matches(&format!("item_{n:03}")).count(), 1);
    }
    Ok(())
}

#[test]
fn all_six_chart_types_have_editable_workbooks_and_vector_pdf() -> Result {
    let dir = tempfile::tempdir()?;
    for (kind, tag, extra) in [
        ("bar", "barChart", "val=\"bar\""),
        ("column", "barChart", "val=\"col\""),
        ("line", "lineChart", ""),
        ("pie", "pieChart", ""),
        ("area", "areaChart", ""),
        ("scatter", "scatterChart", "c:xVal"),
    ] {
        let labels = if kind == "scatter" {
            json!(["1.5", "3", "7"])
        } else {
            json!(["North", "South", "West"])
        };
        let slides = json!([{"title":"Results", "chart":{"type":kind, "labels":labels, "series":[{"name":"Revenue", "values":[24,63,39]}]}}]);
        call(
            dir.path(),
            json!({"op":"create", "output":"chart.pptx", "slides":slides}),
        )?;
        let p = parts(&std::fs::read(dir.path().join("chart.pptx"))?)?;
        let chart = text(&p, "ppt/charts/chart1.xml")?;
        assert!(chart.contains(&format!("<c:{tag}>")));
        assert!(chart.contains(extra));
        let book = parts(p.get("ppt/embeddings/chart1.xlsx").ok_or("no book")?)?;
        let sheet = text(&book, "xl/worksheets/sheet1.xml")?;
        for token in ["Revenue", ">24<", ">63<", ">39<"] {
            assert!(chart.contains(token));
            assert!(sheet.contains(token));
        }
        if kind == "scatter" {
            assert!(sheet.contains("r=\"A2\" t=\"n\""));
            assert!(sheet.contains(">1.5<"));
            assert!(chart.contains("c:yVal"));
            assert_eq!(chart.matches("<c:valAx>").count(), 2);
        }
        let read = call(dir.path(), json!({"op":"read", "input":"chart.pptx"}))?;
        assert!(read.text.ok_or("no text")?.contains("63"));
        call(
            dir.path(),
            json!({"op":"convert", "input":"chart.pptx", "output":"chart.pdf"}),
        )?;
        assert!(
            Command::new("qpdf")
                .args([
                    "--qdf",
                    "--object-streams=disable",
                    "--stream-data=uncompress"
                ])
                .arg(dir.path().join("chart.pdf"))
                .arg(dir.path().join("chart-qdf.pdf"))
                .status()?
                .success()
        );
        let pdf = std::fs::read(dir.path().join("chart-qdf.pdf"))?;
        let pdf = String::from_utf8_lossy(&pdf);
        assert!(pdf.contains(" l") || pdf.contains(" re") || pdf.contains(" c"));
        assert!(!pdf.contains("/Subtype /Image"), "chart was rasterized");
    }
    Ok(())
}

#[test]
fn scatter_rejects_nonnumeric_and_nonfinite_x_before_writing() -> Result {
    let dir = tempfile::tempdir()?;
    for label in ["North", "NaN", "inf", "1e999"] {
        let req: Input = serde_json::from_value(
            json!({"op":"create", "output":"bad.pptx", "slides":[{"chart":{"type":"scatter", "labels":[label], "series":[[1]]}}]}),
        )?;
        let error = run(&req, &Options::new(dir.path()))
            .err()
            .ok_or("expected rejection")?;
        assert_eq!(error.code(), "input.chart_scatter_data");
        assert!(!dir.path().join("bad.pptx").exists());
    }
    Ok(())
}

#[test]
fn foreign_office_edit_preserves_unaffected_parts_byte_for_byte() -> Result {
    let dir = tempfile::tempdir()?;
    call(
        dir.path(),
        json!({"op":"create", "output":"source.pptx", "slides":[{"title":"Replace me", "text":"Old text"}, {"title":"Keep exactly", "text":"Untouched content", "notes":"Untouched note"}, {"title":"Remove this"}]}),
    )?;
    let foreign = dir.path().join("foreign");
    std::fs::create_dir(&foreign)?;
    let profile = format!(
        "-env:UserInstallation=file://{}",
        dir.path().join("profile").display()
    );
    assert!(
        Command::new("soffice")
            .arg(profile)
            .args(["--headless", "--convert-to", "pptx", "--outdir"])
            .arg(&foreign)
            .arg(dir.path().join("source.pptx"))
            .status()?
            .success()
    );
    let before = parts(&std::fs::read(foreign.join("source.pptx"))?)?;
    call(
        dir.path(),
        json!({"op":"edit", "input":"foreign/source.pptx", "output":"edited.pptx", "replace":{"find":"Old text", "replace":"New text"}, "delete":[2], "add":[{"title":"Added", "text":"New slide"}]}),
    )?;
    let after = parts(&std::fs::read(dir.path().join("edited.pptx"))?)?;
    let allowed = [
        "[Content_Types].xml",
        "ppt/presentation.xml",
        "ppt/_rels/presentation.xml.rels",
        "ppt/slides/slide1.xml",
        "ppt/slides/slide3.xml",
        "ppt/slides/_rels/slide3.xml.rels",
    ];
    for (name, bytes) in &before {
        if !allowed.contains(&name.as_str()) {
            assert_eq!(Some(bytes), after.get(name), "unexpectedly changed {name}");
        }
    }
    let read = call(dir.path(), json!({"op":"read", "input":"edited.pptx"}))?;
    let data = read.data.ok_or("no data")?;
    assert_eq!(data.as_array().ok_or("no slides")?.len(), 3);
    assert!(
        data[0]["text"]
            .as_str()
            .ok_or("no text")?
            .contains("New text")
    );
    assert_eq!(data[1]["title"], "Keep exactly");
    assert_eq!(data[2]["title"], "Added");
    Ok(())
}

#[test]
fn entity_characters_survive_read_edit_and_direct_pdf() -> Result {
    let dir = tempfile::tempdir()?;
    let code = "if a < b && b > 0 { Ok(\"A&B\") }";
    call(
        dir.path(),
        json!({"op":"create", "output":"entities.pptx", "markdown":format!("## Operators\n```rust\n{code}\n```\n")}),
    )?;
    let read = call(dir.path(), json!({"op":"read", "input":"entities.pptx"}))?;
    assert!(read.text.ok_or("no text")?.contains(code));
    call(
        dir.path(),
        json!({"op":"edit", "input":"entities.pptx", "output":"edited.pptx", "replace":{"find":"A&B", "replace":"C&D"}}),
    )?;
    let read = call(dir.path(), json!({"op":"read", "input":"edited.pptx"}))?;
    assert!(read.text.ok_or("no text")?.contains("C&D"));
    call(
        dir.path(),
        json!({"op":"create", "output":"entities.pdf", "markdown":format!("## Operators\n```rust\n{code}\n```\n")}),
    )?;
    let result = Command::new("pdftotext")
        .arg(dir.path().join("entities.pdf"))
        .arg("-")
        .output()?;
    assert!(result.status.success());
    let text = String::from_utf8(result.stdout)?;
    assert!(text.contains("a < b && b > 0"), "{text}");
    assert!(text.contains("A&B"), "{text}");
    Ok(())
}

#[test]
fn dense_pdf_words_stay_inside_the_body_with_default_and_fallback_fonts() -> Result {
    let dir = tempfile::tempdir()?;
    let content = (0..600).map(|n| format!("ITEM{n:04} ")).collect::<String>();
    let slides = json!([{"title":"Dense narrative", "text":content}]);
    call(
        dir.path(),
        json!({"op":"create", "output":"dense.pptx", "slides":slides}),
    )?;
    for mode in ["convert", "create"] {
        let output = format!("{mode}.pdf");
        let request = if mode == "convert" {
            json!({"op":"convert", "input":"dense.pptx", "output":output})
        } else {
            json!({"op":"create", "output":output, "slides":slides})
        };
        call(dir.path(), request)?;
        let bbox = dir.path().join("bounds.html");
        assert!(
            Command::new("pdftotext")
                .arg("-bbox")
                .arg(dir.path().join(output))
                .arg(&bbox)
                .status()?
                .success()
        );
        let bytes = std::fs::read(bbox)?;
        let mut reader = quick_xml::Reader::from_reader(bytes.as_slice());
        let mut bounds = Vec::new();
        let mut count = 0;
        loop {
            match reader.read_event()? {
                quick_xml::events::Event::Start(e) if e.name().as_ref() == "word" => {
                    bounds.clear();
                    for attr in e.attributes() {
                        let attr = attr?;
                        bounds.push((
                            attr.key.as_ref().to_string(),
                            attr.normalized_value(quick_xml::XmlVersion::default())?
                                .parse::<f64>()?,
                        ));
                    }
                }
                quick_xml::events::Event::Text(t) if t.as_ref().starts_with("ITEM") => {
                    count += 1;
                    for (key, value) in &bounds {
                        match key.as_str() {
                            "xMin" => assert!(*value >= 46.7),
                            "xMax" => assert!(*value <= 913.3),
                            "yMin" => assert!(*value >= 118.7, "{mode}: {value}"),
                            "yMax" => assert!(*value <= 471.7, "{mode}: {value}"),
                            _ => {}
                        }
                    }
                }
                quick_xml::events::Event::Eof => break,
                _ => {}
            }
        }
        assert_eq!(count, 600);
    }
    Ok(())
}

#[test]
fn four_themes_use_distinct_palettes_and_consistent_grid() -> Result {
    let dir = tempfile::tempdir()?;
    let mut themes = Vec::new();
    for theme in ["light", "dark", "corporate", "creative"] {
        call(
            dir.path(),
            json!({"op":"create", "output":"theme.pptx", "theme":theme, "slides":[{"title":"Plan", "bullets":["First", "Second"]}]}),
        )?;
        let p = parts(&std::fs::read(dir.path().join("theme.pptx"))?)?;
        themes.push(text(&p, "ppt/theme/theme1.xml")?.to_string());
        let slide = text(&p, "ppt/slides/slide1.xml")?;
        assert!(slide.contains("indent=\"-171450\""));
        assert!(slide.contains("marL=\"228600\""));
    }
    for i in 0..4 {
        for j in i + 1..4 {
            assert_ne!(themes[i], themes[j]);
        }
    }
    Ok(())
}

#[test]
fn impossible_table_fails_instead_of_writing_a_clipped_file() -> Result {
    let dir = tempfile::tempdir()?;
    let req: Input = serde_json::from_value(
        json!({"op":"create", "output":"bad.pptx", "slides":[{"table":{"headers":vec!["H"; 200], "rows":[vec!["X"; 200]]}}]}),
    )?;
    let error = run(&req, &Options::new(dir.path()))
        .err()
        .ok_or("expected fit error")?;
    assert_eq!(error.code(), "slide.cannot_fit");
    assert!(!dir.path().join("bad.pptx").exists());
    Ok(())
}
