//! Regression coverage for company templates combined with pagination and citations.
use quick_xml::events::Event;
use serde_json::{Value, json};
use slides::{Input, Options, Output, run};
use std::collections::BTreeMap;
use std::io::{Cursor, Read, Write};
use std::path::Path;

type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

fn call(dir: &Path, value: Value) -> Result<Output> {
    Ok(run(
        &serde_json::from_value::<Input>(value)?,
        &Options::new(dir),
    )?)
}

#[derive(Default)]
struct Element {
    name: String,
    attrs: BTreeMap<String, String>,
    children: Vec<Element>,
    text: String,
}
impl Element {
    fn all<'a>(&'a self, name: &str) -> Vec<&'a Element> {
        let mut out = Vec::new();
        if self.name == name {
            out.push(self);
        }
        for child in &self.children {
            out.extend(child.all(name));
        }
        out
    }
    fn first(&self, name: &str) -> Result<&Element> {
        self.all(name)
            .into_iter()
            .next()
            .ok_or_else(|| format!("missing {name}").into())
    }
    fn attr(&self, name: &str) -> Result<&str> {
        Ok(self
            .attrs
            .get(name)
            .ok_or_else(|| format!("missing {name}"))?)
    }
    fn number(&self, name: &str) -> Result<i64> {
        Ok(self.attr(name)?.parse()?)
    }
}
fn xml(bytes: &[u8]) -> Result<Element> {
    let mut reader = quick_xml::Reader::from_reader(bytes);
    let mut stack = vec![Element::default()];
    loop {
        let event = reader.read_event()?;
        let empty = matches!(&event, Event::Empty(_));
        match event {
            Event::Start(e) | Event::Empty(e) => {
                let mut element = Element {
                    name: e.name().as_ref().to_string(),
                    ..Element::default()
                };
                for attr in e.attributes() {
                    let attr = attr?;
                    element.attrs.insert(
                        attr.key.as_ref().to_string(),
                        attr.normalized_value(quick_xml::XmlVersion::default())?
                            .into_owned(),
                    );
                }
                if empty {
                    stack.last_mut().ok_or("no parent")?.children.push(element);
                } else {
                    stack.push(element);
                }
            }
            Event::End(_) => {
                let element = stack.pop().ok_or("no element")?;
                stack.last_mut().ok_or("no parent")?.children.push(element);
            }
            Event::Text(t) => stack
                .last_mut()
                .ok_or("no text parent")?
                .text
                .push_str(&t.xml_content(quick_xml::XmlVersion::default())),
            Event::Eof => break,
            _ => {}
        }
    }
    stack.pop().ok_or_else(|| "no document".into())
}
fn unpack(path: &Path) -> Result<BTreeMap<String, Vec<u8>>> {
    let mut zip = zip::ZipArchive::new(Cursor::new(std::fs::read(path)?))?;
    let mut parts = BTreeMap::new();
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes)?;
        parts.insert(entry.name()?.to_string(), bytes);
    }
    Ok(parts)
}
fn small_company_template(dir: &Path) -> Result {
    call(
        dir,
        json!({"op":"create","output":"seed.pptx","theme":"corporate","markdown":"# Sample"}),
    )?;
    let mut parts = unpack(&dir.join("seed.pptx"))?;
    // The single-content layout has a narrow, lower company body placeholder.
    let part = parts
        .get_mut("ppt/slideLayouts/slideLayout2.xml")
        .ok_or("no layout")?;
    let old = String::from_utf8(part.clone())?;
    let new = old.replace("y=\"1508760\"", "y=\"1800000\"").replace(
        "cx=\"11003280\" cy=\"4480560\"",
        "cx=\"4200000\" cy=\"4200000\"",
    );
    assert_ne!(new, old);
    assert!(new.contains("cx=\"4200000\""));
    *part = new.into_bytes();
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes) in parts {
        zip.start_file(name, zip::write::SimpleFileOptions::default())?;
        zip.write_all(&bytes)?;
    }
    std::fs::write(dir.join("company.pptx"), zip.finish()?.into_inner())?;
    Ok(())
}
fn source_bounds(path: &Path) -> Result<usize> {
    let mut count = 0;
    for (name, bytes) in unpack(path)? {
        if !name.contains("/slides/slide") || !name.ends_with(".xml") {
            continue;
        }
        let dom = xml(&bytes)?;
        let shapes = dom.all("p:sp");
        let source = shapes
            .iter()
            .find(|s| {
                s.first("p:cNvPr").and_then(|n| n.attr("name")).ok() == Some("Source footnotes")
            })
            .ok_or("missing source box")?;
        let top = source.first("a:off")?.number("y")?;
        let bottom = top + source.first("a:ext")?.number("cy")?;
        assert!(bottom <= 6858000, "sources extend below slide in {name}");
        for shape in shapes.into_iter().chain(dom.all("p:graphicFrame")) {
            let shape_name = shape.first("p:cNvPr")?.attr("name")?;
            if matches!(
                shape_name,
                "Source footnotes" | "Slide number" | "Theme accent"
            ) {
                continue;
            }
            let y = shape.first("a:off")?.number("y")?;
            let height = shape.first("a:ext")?.number("cy")?;
            assert!(
                height > 0 && y + height < top,
                "content overlaps sources: {name} {shape_name}"
            );
            for row in shape.all("a:tr") {
                assert!(row.number("h")? > 0);
            }
            if shape.name == "p:graphicFrame" && !shape.all("a:tr").is_empty() {
                let rows: i64 = shape
                    .all("a:tr")
                    .iter()
                    .map(|r| r.number("h"))
                    .collect::<Result<Vec<_>>>()?
                    .iter()
                    .sum();
                assert!(rows <= height, "table rows exceed frame");
            }
        }
        count += 1;
    }
    Ok(count)
}
fn citations() -> Vec<String> {
    (1..=10)
        .map(|n| format!("Source: planning report, appendix {n}"))
        .collect()
}

#[test]
fn template_pagination_uses_company_capacity_and_preserves_sources_and_objects() -> Result {
    let dir = tempfile::tempdir()?;
    small_company_template(dir.path())?;
    let bullets: Vec<_> = (0..32)
        .map(|n| {
            format!("Checkpoint{n:03}: confirm ownership and delivery readiness before launch")
        })
        .collect();
    let sources = citations();
    let slides = json!([
        {"title":"Readiness","bullets":bullets,"sources":sources,"notes":"First fragment only"},
        {"title":"Metrics","table":{"headers":["Metric","Value"],"rows":[["Revenue","25"],["Margin","20%"]]},"sources":sources},
        {"title":"Trend","chart":{"type":"column","labels":["Q1","Q2"],"series":[[10,20]]},"sources":sources}
    ]);
    let plain = call(
        dir.path(),
        json!({"op":"create","output":"plain.pptx","slides":slides}),
    )?;
    let request = json!({"op":"create","output":"company-deck.pptx","template":"company.pptx","slides":slides});
    let result = call(dir.path(), request.clone())?;
    let count: usize = result.summary_params["count"]
        .as_str()
        .ok_or("no count")?
        .parse()?;
    let plain_count: usize = plain.summary_params["count"]
        .as_str()
        .ok_or("no count")?
        .parse()?;
    assert!(count > plain_count, "company width must affect pagination");
    assert!(
        result
            .warnings
            .iter()
            .any(|w| w.code == "slide.split" && w.params["slide"] == "1")
    );
    assert!(
        !result
            .warnings
            .iter()
            .any(|w| w.code == "edit.text_overflow")
    );
    let read = call(dir.path(), json!({"op":"read","input":"company-deck.pptx"}))?
        .data
        .ok_or("no slides")?;
    let output = read.as_array().ok_or("no slide list")?;
    assert_eq!(output.len(), count);
    assert_eq!(source_bounds(&dir.path().join("company-deck.pptx"))?, count);
    let text: String = output.iter().filter_map(|s| s["text"].as_str()).collect();
    for n in 0..32 {
        assert_eq!(text.matches(&format!("Checkpoint{n:03}")).count(), 1);
    }
    assert_eq!(
        output
            .iter()
            .filter(|s| s["notes"] == "First fragment only")
            .count(),
        1
    );
    for slide in output {
        assert_eq!(slide["sources"], json!(sources));
    }
    assert!(text.contains("Revenue\t25"));
    assert!(text.contains("10\t20"));
    let first = std::fs::read(dir.path().join("company-deck.pptx"))?;
    let mut archive = zip::ZipArchive::new(Cursor::new(&first))?;
    for i in 0..archive.len() {
        assert_eq!(
            archive.by_index(i)?.last_modified(),
            Some(zip::DateTime::default())
        );
    }
    call(dir.path(), request.clone())?;
    assert_eq!(first, std::fs::read(dir.path().join("company-deck.pptx"))?);
    std::thread::scope(|scope| -> Result {
        let mut handles = Vec::new();
        for name in ["parallel-a.pptx", "parallel-b.pptx"] {
            let mut request = request.clone();
            request["output"] = json!(name);
            let path = dir.path();
            handles.push(
                scope.spawn(move || -> std::result::Result<Vec<u8>, String> {
                    call(path, request).map_err(|e| e.to_string())?;
                    std::fs::read(path.join(name)).map_err(|e| e.to_string())
                }),
            );
        }
        for handle in handles {
            assert_eq!(handle.join().map_err(|_| "worker failed")??, first);
        }
        Ok(())
    })?;
    Ok(())
}

#[test]
fn citations_reserve_space_and_reduce_capacity_without_a_template() -> Result {
    let dir = tempfile::tempdir()?;
    let bullets: Vec<_> = (0..20)
        .map(|n| format!("Action {n}: validate delivery"))
        .collect();
    let plain = call(
        dir.path(),
        json!({"op":"create","output":"plain.pptx","slides":[{"title":"Plan","bullets":bullets}]}),
    )?;
    let cited = call(
        dir.path(),
        json!({"op":"create","output":"cited.pptx","slides":[{"title":"Plan","bullets":bullets,"sources":citations()}]}),
    )?;
    let count = |o: &Output| -> Result<usize> {
        Ok(o.summary_params["count"]
            .as_str()
            .ok_or("no count")?
            .parse()?)
    };
    assert!(count(&cited)? > count(&plain)?);
    assert_eq!(
        source_bounds(&dir.path().join("cited.pptx"))?,
        count(&cited)?
    );
    Ok(())
}

#[test]
fn inherited_company_body_geometry_is_fitted_without_modifying_the_master() -> Result {
    let dir = tempfile::tempdir()?;
    small_company_template(dir.path())?;
    let mut parts = unpack(&dir.path().join("company.pptx"))?;
    let master = parts
        .get("ppt/slideMasters/slideMaster1.xml")
        .ok_or("no master")?
        .clone();
    let layout = parts
        .get_mut("ppt/slideLayouts/slideLayout2.xml")
        .ok_or("no layout")?;
    let old = String::from_utf8(layout.clone())?;
    let new = old.replace("<a:xfrm><a:off x=\"594360\" y=\"1800000\"/><a:ext cx=\"4200000\" cy=\"4200000\"/></a:xfrm>", "");
    assert_ne!(new, old);
    *layout = new.into_bytes();
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes) in parts {
        zip.start_file(name, zip::write::SimpleFileOptions::default())?;
        zip.write_all(&bytes)?;
    }
    std::fs::write(
        dir.path().join("inherited.pptx"),
        zip.finish()?.into_inner(),
    )?;
    let result = call(
        dir.path(),
        json!({"op":"create","template":"inherited.pptx","output":"fitted.pptx","slides":[{"title":"Plan","bullets":["Confirm readiness","Review owners"],"sources":citations()}]}),
    )?;
    assert_eq!(result.summary_params["count"], "1");
    assert_eq!(source_bounds(&dir.path().join("fitted.pptx"))?, 1);
    let output = unpack(&dir.path().join("fitted.pptx"))?;
    assert_eq!(
        output.get("ppt/slideMasters/slideMaster1.xml"),
        Some(&master)
    );
    Ok(())
}

#[test]
fn template_dimensions_cannot_overflow_source_geometry() -> Result {
    let dir = tempfile::tempdir()?;
    small_company_template(dir.path())?;
    let mut parts = unpack(&dir.path().join("company.pptx"))?;
    let pres = parts
        .get_mut("ppt/presentation.xml")
        .ok_or("no presentation")?;
    *pres = String::from_utf8(pres.clone())?
        .replace("cx=\"12192000\"", "cx=\"9223372036854775807\"")
        .into_bytes();
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes) in parts {
        zip.start_file(name, zip::write::SimpleFileOptions::default())?;
        zip.write_all(&bytes)?;
    }
    std::fs::write(dir.path().join("huge.pptx"), zip.finish()?.into_inner())?;
    let request: Input = serde_json::from_value(
        json!({"op":"create","template":"huge.pptx","output":"bad.pptx","slides":[{"title":"Plan","text":"Confirm readiness","sources":["Source: planning report"]}]}),
    )?;
    let error = run(&request, &Options::new(dir.path()))
        .err()
        .ok_or("accepted overflowing dimensions")?;
    assert_eq!(error.code(), "package.invalid");
    assert!(!dir.path().join("bad.pptx").exists());
    Ok(())
}

#[test]
fn create_convert_template_matrix_preserves_pdf_rejection_code() -> Result {
    let dir = tempfile::tempdir()?;
    small_company_template(dir.path())?;
    let markdown = format!(
        "## Plan\n{}\n<!-- source: Source: planning report -->\n",
        (0..40)
            .map(|n| format!("- Delivery{n:03}: confirm readiness and owners\n"))
            .collect::<String>()
    );
    std::fs::write(dir.path().join("input.md"), &markdown)?;
    for template in [Value::Null, json!("company.pptx")] {
        let created = call(
            dir.path(),
            json!({"op":"create","output":"created.pptx","template":template,"markdown":markdown}),
        )?;
        let converted = call(
            dir.path(),
            json!({"op":"convert","input":"input.md","output":"converted.pptx","template":template}),
        )?;
        assert_eq!(
            created.summary_params["count"],
            converted.summary_params["count"]
        );
        assert_eq!(
            std::fs::read(dir.path().join("created.pptx"))?,
            std::fs::read(dir.path().join("converted.pptx"))?
        );
        if template.is_null() {
            call(
                dir.path(),
                json!({"op":"create","output":"created.pdf","markdown":markdown}),
            )?;
            call(
                dir.path(),
                json!({"op":"convert","input":"input.md","output":"converted.pdf"}),
            )?;
            call(
                dir.path(),
                json!({"op":"convert","input":"created.pptx","output":"rendered.pdf"}),
            )?;
        }
    }
    for request in [
        json!({"op":"create","output":"bad.pdf","template":"company.pptx","markdown":"# Plan"}),
        json!({"op":"convert","input":"input.md","output":"bad.pdf","template":"company.pptx"}),
    ] {
        let input: Input = serde_json::from_value(request)?;
        assert_eq!(
            run(&input, &Options::new(dir.path()))
                .err()
                .ok_or("template PDF was accepted")?
                .code(),
            "template.output"
        );
        assert!(!dir.path().join("bad.pdf").exists());
    }
    Ok(())
}
