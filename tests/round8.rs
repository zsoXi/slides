//! Semantic text, typography, package integrity and scatter identity regressions.
use serde_json::{Value, json};
use slides::{Input, Options, Output, run};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Read, Write};
use std::path::Path;
use std::process::Command;

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
    let mut out = BTreeMap::new();
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
fn resolve(source: &str, raw: &str) -> Result<String> {
    let mut bits: Vec<_> = if raw.starts_with('/') {
        vec![]
    } else {
        source
            .rsplit_once('/')
            .map(|(d, _)| d.split('/').collect())
            .unwrap_or_default()
    };
    for bit in raw.split('/') {
        match bit {
            "" | "." => {}
            ".." => {
                bits.pop().ok_or("escaped package")?;
            }
            _ => bits.push(bit),
        }
    }
    Ok(bits.join("/"))
}
fn elements(bytes: &[u8], name: &str) -> Result<Vec<BTreeMap<String, String>>> {
    let mut reader = quick_xml::Reader::from_reader(bytes);
    let mut result = Vec::new();
    loop {
        match reader.read_event()? {
            quick_xml::events::Event::Start(e) | quick_xml::events::Event::Empty(e)
                if e.name().as_ref() == name =>
            {
                let mut attrs = BTreeMap::new();
                for attr in e.attributes() {
                    let attr = attr?;
                    attrs.insert(
                        attr.key.as_ref().to_string(),
                        attr.normalized_value(quick_xml::XmlVersion::default())?
                            .into_owned(),
                    );
                }
                result.push(attrs);
            }
            quick_xml::events::Event::Eof => break,
            _ => {}
        }
    }
    Ok(result)
}
fn graph(parts: &Parts) -> Result {
    let mut edges = BTreeMap::new();
    for (path, bytes) in parts.iter().filter(|(p, _)| p.ends_with(".rels")) {
        let source = if path == "_rels/.rels" {
            String::new()
        } else {
            let (dir, file) = path.rsplit_once("/_rels/").ok_or("bad rels path")?;
            format!("{dir}/{}", file.strip_suffix(".rels").ok_or("bad suffix")?)
        };
        assert!(
            source.is_empty() || parts.contains_key(&source),
            "orphan relationship {path}"
        );
        let mut targets = Vec::new();
        for rel in elements(bytes, "Relationship")? {
            if rel
                .get("TargetMode")
                .is_some_and(|s| s.eq_ignore_ascii_case("external"))
            {
                continue;
            }
            let target = resolve(&source, rel.get("Target").ok_or("no target")?)?;
            assert!(parts.contains_key(&target), "dangling {path} -> {target}");
            targets.push(target);
        }
        edges.insert(source, targets);
    }
    let mut reached = BTreeSet::from(["[Content_Types].xml".to_string()]);
    let mut pending = vec![String::new()];
    while let Some(source) = pending.pop() {
        if !reached.insert(source.clone()) {
            continue;
        }
        let relpath = if source.is_empty() {
            "_rels/.rels".into()
        } else {
            let (dir, file) = source.rsplit_once('/').unwrap_or(("", &source));
            format!("{dir}/_rels/{file}.rels")
                .trim_start_matches('/')
                .to_string()
        };
        if parts.contains_key(&relpath) {
            reached.insert(relpath);
        }
        pending.extend(edges.get(&source).cloned().unwrap_or_default());
    }
    let orphaned: Vec<_> = parts.keys().filter(|p| !reached.contains(*p)).collect();
    assert!(orphaned.is_empty(), "orphan parts: {orphaned:?}");
    for ct in elements(
        parts.get("[Content_Types].xml").ok_or("no types")?,
        "Override",
    )? {
        assert!(
            parts.contains_key(
                ct.get("PartName")
                    .ok_or("no part name")?
                    .trim_start_matches('/')
            )
        );
    }
    let ids = elements(
        parts.get("ppt/presentation.xml").ok_or("no presentation")?,
        "p:sldId",
    )?;
    let rels = elements(
        parts
            .get("ppt/_rels/presentation.xml.rels")
            .ok_or("no presentation rels")?,
        "Relationship",
    )?;
    for id in ids {
        let rel = rels
            .iter()
            .find(|r| r.get("Id") == id.get("r:id"))
            .ok_or("dangling slide id")?;
        assert!(rel.get("Type").is_some_and(|t| t.ends_with("/slide")));
    }
    Ok(())
}
fn verify(dir: &Path, file: &str, count: usize) -> Result {
    graph(&parts(&dir.join(file))?)?;
    let read = call(dir, json!({"op":"read","input":file}))?;
    assert_eq!(
        read.data
            .ok_or("no data")?
            .as_array()
            .ok_or("no slides")?
            .len(),
        count
    );
    call(
        dir,
        json!({"op":"convert","input":file,"output":"check.pdf"}),
    )?;
    let info = Command::new("pdfinfo")
        .arg(dir.join("check.pdf"))
        .output()?;
    assert!(info.status.success());
    let info = String::from_utf8(info.stdout)?;
    let pages = info
        .lines()
        .find(|s| s.starts_with("Pages:"))
        .ok_or("no page count")?;
    assert_eq!(
        pages
            .split_whitespace()
            .nth(1)
            .ok_or("no pages")?
            .parse::<usize>()?,
        count
    );
    if let Some(dest) = std::env::var_os("SLIDES_TEST_ARTIFACTS") {
        let dest = std::path::PathBuf::from(dest)
            .join(format!("{}-{count}", file.trim_end_matches(".pptx")));
        std::fs::create_dir_all(&dest)?;
        std::fs::copy(dir.join(file), dest.join("deck.pptx"))?;
        std::fs::copy(dir.join("check.pdf"), dest.join("deck.pdf"))?;
        assert!(
            Command::new("pdftoppm")
                .args(["-scale-to", "800", "-png"])
                .arg(dest.join("deck.pdf"))
                .arg(dest.join("slide"))
                .status()?
                .success()
        );
    }
    Ok(())
}
fn title_xml(parts: &Parts) -> Result<&str> {
    let slide = xml(parts, "ppt/slides/slide1.xml")?;
    slide
        .split("<p:sp>")
        .find(|s| s.contains("name=\"Title\""))
        .and_then(|s| s.split("</p:sp>").next())
        .ok_or_else(|| "no title".into())
}
fn title_runs(parts: &mut Parts, runs: &str) -> Result {
    let slide = xml(parts, "ppt/slides/slide1.xml")?.to_string();
    let title = title_xml(parts)?;
    let start = title.find("<a:r>").ok_or("no run")?;
    let end = title.find("</a:r>").ok_or("no run end")? + "</a:r>".len();
    let changed = format!("{}{}{}", &title[..start], runs, &title[end..]);
    parts.insert(
        "ppt/slides/slide1.xml".into(),
        slide.replace(title, &changed).into_bytes(),
    );
    Ok(())
}

#[test]
fn semantic_breaks_tabs_fields_and_split_runs_roundtrip() -> Result {
    let dir = tempfile::tempdir()?;
    call(
        dir.path(),
        json!({"op":"create","output":"base.pptx","slides":[{"title":"Control"}]}),
    )?;
    let original = parts(&dir.path().join("base.pptx"))?;
    for (n, runs, expected) in [
        (
            0,
            "<a:r><a:t>Northstar</a:t></a:r><a:br/><a:r><a:t>Analytics</a:t></a:r>",
            "Northstar\nAnalytics",
        ),
        (
            1,
            "<a:r><a:t>Northstar\nAnalytics</a:t></a:r>",
            "Northstar\nAnalytics",
        ),
        (
            2,
            "<a:r><a:t>Northstar\t</a:t></a:r><a:fld id=\"{12345678-1234-1234-1234-123456789012}\" type=\"datetime\"><a:rPr b=\"1\"/><a:t>Analytics</a:t></a:fld>",
            "Northstar\tAnalytics",
        ),
        (
            3,
            "<a:r><a:t>North</a:t></a:r><a:r><a:rPr b=\"1\"/><a:t>star Analytics</a:t></a:r>",
            "Northstar Analytics",
        ),
    ] {
        let mut p = original.clone();
        title_runs(&mut p, runs)?;
        pack(&dir.path().join("source.pptx"), &p)?;
        let read = call(dir.path(), json!({"op":"read","input":"source.pptx"}))?;
        assert_eq!(read.data.ok_or("no data")?[0]["title"], expected);
        call(
            dir.path(),
            json!({"op":"convert","input":"source.pptx","output":"source.md"}),
        )?;
        assert!(std::fs::read_to_string(dir.path().join("source.md"))?.contains(expected));
        let output = format!("semantic-{n}.pptx");
        let result = call(
            dir.path(),
            json!({"op":"edit","input":"source.pptx","output":output,"replace":{"find":expected,"replace":"Updated\nAnalytics"}}),
        )?;
        assert!(!result.warnings.iter().any(|w| w.code == "edit.no_match"));
        let read = call(dir.path(), json!({"op":"read","input":output}))?;
        assert_eq!(
            read.data.ok_or("no data")?[0]["title"],
            "Updated\nAnalytics"
        );
        verify(dir.path(), &output, 1)?;
    }
    Ok(())
}

#[test]
fn cross_run_replacement_preserves_first_style_surroundings_and_reports_no_match() -> Result {
    let dir = tempfile::tempdir()?;
    call(
        dir.path(),
        json!({"op":"create","output":"source.pptx","slides":[{"title":"Control"}]}),
    )?;
    let mut p = parts(&dir.path().join("source.pptx"))?;
    title_runs(
        &mut p,
        "<a:r><a:rPr sz=\"1200\"/><a:t>Before </a:t></a:r><a:r><a:rPr sz=\"3400\" b=\"1\"/><a:t>Northstar </a:t></a:r><a:r><a:rPr sz=\"1800\" i=\"1\"/><a:t>Analytics after</a:t></a:r>",
    )?;
    pack(&dir.path().join("source.pptx"), &p)?;
    let edited = call(
        dir.path(),
        json!({"op":"edit","input":"source.pptx","output":"styled.pptx","replace":{"find":"Northstar Analytics","replace":"Zażółć"}}),
    )?;
    assert!(edited.warnings.is_empty(), "{:?}", edited.warnings);
    let after = parts(&dir.path().join("styled.pptx"))?;
    let title = title_xml(&after)?;
    assert!(title.contains("<a:rPr sz=\"1200\"/><a:t>Before </a:t>"));
    assert!(title.contains("<a:rPr sz=\"3400\" b=\"1\"/><a:t>Zażółć</a:t>"));
    assert!(title.contains("<a:rPr sz=\"1800\" i=\"1\"/><a:t> after</a:t>"));
    verify(dir.path(), "styled.pptx", 1)?;
    let out = call(
        dir.path(),
        json!({"op":"edit","input":"styled.pptx","output":"no-match.pptx","replace":{"find":"not present","replace":"unused"}}),
    )?;
    assert!(out.warnings.iter().any(|w| w.code == "edit.no_match"));
    assert_eq!(after, parts(&dir.path().join("no-match.pptx"))?);
    verify(dir.path(), "no-match.pptx", 1)?;
    Ok(())
}

#[test]
fn replacement_inside_a_field_preserves_one_field_identity_and_its_style() -> Result {
    let dir = tempfile::tempdir()?;
    call(
        dir.path(),
        json!({"op":"create","output":"source.pptx","slides":[{"title":"Control"}]}),
    )?;
    let mut p = parts(&dir.path().join("source.pptx"))?;
    title_runs(
        &mut p,
        "<a:fld id=\"{12345678-1234-1234-1234-123456789012}\" type=\"custom\"><a:rPr b=\"1\" sz=\"3400\"/><a:t>Northstar Analytics</a:t></a:fld>",
    )?;
    pack(&dir.path().join("source.pptx"), &p)?;
    call(
        dir.path(),
        json!({"op":"edit","input":"source.pptx","output":"field.pptx","replace":{"find":"star Ana","replace":"wind Ana"}}),
    )?;
    let after = parts(&dir.path().join("field.pptx"))?;
    let title = title_xml(&after)?;
    assert_eq!(title.matches("<a:fld").count(), 1);
    assert!(title.contains("type=\"custom\""));
    assert!(title.contains("b=\"1\" sz=\"3400\""));
    let read = call(dir.path(), json!({"op":"read","input":"field.pptx"}))?;
    assert_eq!(
        read.data.ok_or("no data")?[0]["title"],
        "Northwind Analytics"
    );
    verify(dir.path(), "field.pptx", 1)?;
    Ok(())
}

#[test]
fn field_edits_preserve_typography_and_identical_slide_bytes() -> Result {
    let dir = tempfile::tempdir()?;
    call(
        dir.path(),
        json!({"op":"create","output":"source.pptx","slides":[{"title":"The problem","bullets":["Existing body"]}]}),
    )?;
    let original = parts(&dir.path().join("source.pptx"))?;
    call(
        dir.path(),
        json!({"op":"edit","input":"source.pptx","output":"identical.pptx","slides":[{"index":0,"title":"The problem","bullets":["Existing body"]}]}),
    )?;
    assert_eq!(original, parts(&dir.path().join("identical.pptx"))?);
    verify(dir.path(), "identical.pptx", 1)?;
    for inherited in [false, true] {
        let mut p = original.clone();
        if inherited {
            let title = title_xml(&p)?.to_string();
            let mut changed = title.clone();
            while let Some(start) = changed.find("<a:rPr") {
                let end =
                    start + changed[start..].find("</a:rPr>").ok_or("rPr end")? + "</a:rPr>".len();
                changed.replace_range(start..end, "");
            }
            let slide = xml(&p, "ppt/slides/slide1.xml")?.replace(&title, &changed);
            p.insert("ppt/slides/slide1.xml".into(), slide.into_bytes());
        }
        pack(&dir.path().join("input.pptx"), &p)?;
        let output = if inherited {
            "inherited.pptx"
        } else {
            "local.pptx"
        };
        let result = call(
            dir.path(),
            json!({"op":"edit","input":"input.pptx","output":output,"slides":[{"index":0,"title":"New problem","bullets":["New body"]}]}),
        )?;
        assert!(result.warnings.is_empty(), "{:?}", result.warnings);
        let after = parts(&dir.path().join(output))?;
        assert_eq!(
            title_xml(&p)?.replace("The problem", "New problem"),
            title_xml(&after)?
        );
        verify(dir.path(), output, 1)?;
    }
    Ok(())
}

#[test]
fn deleting_template_relative_and_absolute_slides_keeps_the_package_consistent() -> Result {
    let dir = tempfile::tempdir()?;
    call(
        dir.path(),
        json!({"op":"create","output":"template.pptx","slides":[{"title":"Example","bullets":["Example"],"notes":"Example notes"}]}),
    )?;
    let slides = json!([
        {"title":"Remove me","notes":"Deleted notes","chart":{"type":"column","labels":["Q1"],"series":[[10]]}},
        {"title":"Keep me","notes":"Retained notes"},
        {"title":"Keep last","bullets":["Preserve this"]}
    ]);
    for template in [false, true] {
        let mut req = json!({"op":"create","output":"source.pptx","slides":slides});
        if template {
            req["template"] = json!("template.pptx");
        }
        call(dir.path(), req)?;
        for absolute in [false, true] {
            let mut p = parts(&dir.path().join("source.pptx"))?;
            let path = "ppt/_rels/presentation.xml.rels";
            let mut rels = xml(&p, path)?.to_string();
            for rel in elements(p.get(path).ok_or("no rels")?, "Relationship")? {
                if rel.get("Type").is_some_and(|t| t.ends_with("/slide")) {
                    let raw = rel.get("Target").ok_or("no target")?;
                    let resolved = resolve("ppt/presentation.xml", raw)?;
                    let target = if absolute {
                        format!("/{resolved}")
                    } else {
                        format!("../{resolved}")
                    };
                    rels = rels.replace(
                        &format!("Target=\"{raw}\""),
                        &format!("Target=\"{target}\""),
                    );
                }
            }
            p.insert(path.into(), rels.into_bytes());
            pack(&dir.path().join("input.pptx"), &p)?;
            let output = format!("deleted-{template}-{absolute}.pptx");
            call(
                dir.path(),
                json!({"op":"edit","input":"input.pptx","output":output,"delete":[0],"add":[{"title":"Added","notes":"New note"}]}),
            )?;
            let read = call(dir.path(), json!({"op":"read","input":output}))?;
            let data = read.data.ok_or("no data")?;
            assert_eq!(data[0]["title"], "Keep me");
            assert_eq!(data[0]["notes"], "Retained notes");
            assert_eq!(data[2]["title"], "Added");
            verify(dir.path(), &output, 3)?;
            call(
                dir.path(),
                json!({"op":"edit","input":output,"output":output,"delete":[0]}),
            )?;
            verify(dir.path(), &output, 2)?;
        }
    }
    Ok(())
}

#[test]
fn foreign_slide_deletion_removes_incoming_links_and_custom_show_entries() -> Result {
    let dir = tempfile::tempdir()?;
    call(
        dir.path(),
        json!({"op":"create","output":"seed.pptx","slides":[
            {"title":"Deleted","notes":"Delete note"},{"title":"Retained","notes":"Keep note"}
        ]}),
    )?;
    std::fs::create_dir(dir.path().join("office"))?;
    let profile = tempfile::tempdir()?;
    let office = Command::new("soffice")
        .arg(format!(
            "-env:UserInstallation=file://{}",
            profile.path().display()
        ))
        .args(["--headless", "--convert-to", "pptx", "--outdir"])
        .arg(dir.path().join("office"))
        .arg(dir.path().join("seed.pptx"))
        .output()?;
    assert!(office.status.success());
    let original = parts(&dir.path().join("office/seed.pptx"))?;
    for absolute in [false, true] {
        let mut p = original.clone();
        for (path, bytes) in &mut p {
            if !path.ends_with(".rels") || path == "_rels/.rels" {
                continue;
            }
            let (prefix, file) = path
                .rsplit_once("/_rels/")
                .ok_or("no relationship source")?;
            let source = format!("{prefix}/{}", file.trim_end_matches(".rels"));
            let mut doc = String::from_utf8(bytes.clone())?;
            for rel in elements(bytes, "Relationship")? {
                if rel.get("TargetMode").is_some_and(|s| s == "External") {
                    continue;
                }
                let raw = rel.get("Target").ok_or("no target")?;
                let resolved = resolve(&source, raw)?;
                if absolute {
                    doc = doc.replace(
                        &format!("Target=\"{raw}\""),
                        &format!("Target=\"/{resolved}\""),
                    );
                }
            }
            *bytes = doc.into_bytes();
        }
        let pres_path = "ppt/presentation.xml";
        let first_id = elements(p.get(pres_path).ok_or("no presentation")?, "p:sldId")?
            .first()
            .and_then(|id| id.get("r:id"))
            .ok_or("no slide id")?
            .clone();
        let pres = xml(&p, pres_path)?.replace("</p:presentation>", &format!("<p:custShowLst><p:custShow name=\"Demo\" id=\"0\"><p:sldLst><p:sld r:id=\"{first_id}\"/></p:sldLst></p:custShow></p:custShowLst></p:presentation>"));
        p.insert(pres_path.into(), pres.into_bytes());
        let rel_path = "ppt/slides/_rels/slide2.xml.rels";
        let target = if absolute {
            "/ppt/slides/slide1.xml"
        } else {
            "slide1.xml"
        };
        let rels = xml(&p, rel_path)?.replace("</Relationships>", &format!("<Relationship Id=\"removedLink\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide\" Target=\"{target}\"/></Relationships>"));
        p.insert(rel_path.into(), rels.into_bytes());
        let slide_path = "ppt/slides/slide2.xml";
        let slide = xml(&p, slide_path)?.replacen(
            "</a:rPr>",
            "<a:hlinkClick r:id=\"removedLink\" action=\"ppaction://hlinksldjump\"/></a:rPr>",
            1,
        );
        p.insert(slide_path.into(), slide.into_bytes());
        pack(&dir.path().join("foreign.pptx"), &p)?;
        let output = format!("foreign-deleted-{absolute}.pptx");
        call(
            dir.path(),
            json!({"op":"edit","input":"foreign.pptx","output":output,"delete":[0]}),
        )?;
        let after = parts(&dir.path().join(&output))?;
        assert!(!xml(&after, rel_path)?.contains("removedLink"));
        assert!(!xml(&after, slide_path)?.contains("removedLink"));
        assert!(elements(after.get(pres_path).ok_or("no presentation")?, "p:sld")?.is_empty());
        verify(dir.path(), &output, 1)?;
        std::thread::scope(|scope| -> Result {
            let handles = (0..2).map(|i| {
                let base = dir.path();
                scope.spawn(move || call(base, json!({"op":"edit","input":"foreign.pptx","output":format!("parallel{i}.pptx"),"delete":[0]})).map(|_| ()).map_err(|e| e.to_string()))
            }).collect::<Vec<_>>();
            for h in handles {
                assert!(matches!(h.join(), Ok(Ok(()))));
            }
            Ok(())
        })?;
        for i in 0..2 {
            let file = format!("parallel{i}.pptx");
            assert_eq!(after, parts(&dir.path().join(&file))?);
            verify(dir.path(), &file, 1)?;
        }
    }
    Ok(())
}

#[test]
fn missing_slide_targets_and_dangling_relationships_fail_before_writing() -> Result {
    let dir = tempfile::tempdir()?;
    call(
        dir.path(),
        json!({"op":"create","output":"source.pptx","slides":[{"title":"Keep"}]}),
    )?;
    let original = parts(&dir.path().join("source.pptx"))?;
    std::fs::write(dir.path().join("output.pptx"), b"previous output")?;
    for slide in [false, true] {
        let mut p = original.clone();
        if slide {
            p.remove("ppt/slides/slide1.xml");
        } else {
            let path = "ppt/slides/_rels/slide1.xml.rels";
            let rels = xml(&p, path)?.replace("</Relationships>", "<Relationship Id=\"bad\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/image\" Target=\"../media/missing.png\"/></Relationships>");
            p.insert(path.into(), rels.into_bytes());
        }
        pack(&dir.path().join("invalid.pptx"), &p)?;
        if slide {
            assert!(call(dir.path(), json!({"op":"read","input":"invalid.pptx"})).is_err());
        }
        let result = call(
            dir.path(),
            json!({"op":"edit","input":"invalid.pptx","output":"output.pptx","replace":{"find":"Keep","replace":"New"}}),
        );
        assert!(result.is_err());
        assert_eq!(
            std::fs::read(dir.path().join("output.pptx"))?,
            b"previous output"
        );
    }
    Ok(())
}

#[test]
fn multiple_cross_run_matches_and_body_breaks_export_without_gluing_words() -> Result {
    let dir = tempfile::tempdir()?;
    call(
        dir.path(),
        json!({"op":"create","output":"source.pptx","slides":[{"title":"Paragraphs","bullets":["Northstar\\nAnalytics"]}]}),
    )?;
    let mut p = parts(&dir.path().join("source.pptx"))?;
    let path = "ppt/slides/slide1.xml";
    let slide = xml(&p, path)?.replace(
        "Northstar\\nAnalytics",
        "Northstar</a:t></a:r><a:br/><a:r><a:t>Analytics",
    );
    p.insert(path.into(), slide.into_bytes());
    pack(&dir.path().join("source.pptx"), &p)?;
    call(
        dir.path(),
        json!({"op":"convert","input":"source.pptx","output":"body.md"}),
    )?;
    assert!(
        std::fs::read_to_string(dir.path().join("body.md"))?.contains("Northstar  \n  Analytics")
    );
    title_runs(
        &mut p,
        "<a:r><a:t>éNor</a:t></a:r><a:r><a:rPr b=\"1\"/><a:t>th star; North star!</a:t></a:r>",
    )?;
    pack(&dir.path().join("source.pptx"), &p)?;
    call(
        dir.path(),
        json!({"op":"edit","input":"source.pptx","output":"multiple.pptx","replace":{"find":"North star","replace":"Żółć"}}),
    )?;
    let read = call(dir.path(), json!({"op":"read","input":"multiple.pptx"}))?;
    assert_eq!(read.data.ok_or("no data")?[0]["title"], "éŻółć; Żółć!");
    verify(dir.path(), "multiple.pptx", 1)?;
    Ok(())
}

#[test]
fn overflow_triggers_autofit_and_an_explicit_fitting_warning() -> Result {
    let dir = tempfile::tempdir()?;
    call(
        dir.path(),
        json!({"op":"create","output":"source.pptx","slides":[{"title":"Original title","bullets":["Keep body"]}]}),
    )?;
    let result = call(
        dir.path(),
        json!({"op":"edit","input":"source.pptx","output":"fitted.pptx","slides":[{"index":0,"title":"An updated heading with enough words to need a smaller font"}]}),
    )?;
    assert!(
        result.warnings.iter().any(|w| w.code == "edit.text_fitted"),
        "{:?}",
        result.warnings
    );
    let p = parts(&dir.path().join("fitted.pptx"))?;
    let title = title_xml(&p)?;
    assert!(title.contains("<a:normAutofit"));
    assert!(title.contains("b=\"1\""));
    assert!(!title.contains("sz=\"3400\""));
    verify(dir.path(), "fitted.pptx", 1)?;
    Ok(())
}

#[test]
fn adding_notes_to_an_existing_slide_registers_the_notes_master() -> Result {
    let dir = tempfile::tempdir()?;
    call(
        dir.path(),
        json!({"op":"create","output":"template.pptx","slides":[{"title":"Example"}]}),
    )?;
    for template in [false, true] {
        let mut request =
            json!({"op":"create","output":"source.pptx","slides":[{"title":"Existing"}]});
        if template {
            request["template"] = json!("template.pptx");
        }
        call(dir.path(), request)?;
        let output = format!("notes-{template}.pptx");
        call(
            dir.path(),
            json!({"op":"edit","input":"source.pptx","output":output,"slides":[{"index":0,"notes":"A new note"}]}),
        )?;
        let p = parts(&dir.path().join(&output))?;
        let ids = elements(
            p.get("ppt/presentation.xml").ok_or("no presentation")?,
            "p:notesMasterId",
        )?;
        assert_eq!(ids.len(), 1);
        let rels = elements(
            p.get("ppt/_rels/presentation.xml.rels").ok_or("no rels")?,
            "Relationship",
        )?;
        assert!(rels.iter().any(|r| r.get("Id") == ids[0].get("r:id")
            && r.get("Type").is_some_and(|s| s.ends_with("/notesMaster"))));
        let read = call(dir.path(), json!({"op":"read","input":output}))?;
        assert_eq!(read.data.ok_or("no data")?[0]["notes"], "A new note");
        verify(dir.path(), &output, 1)?;
    }
    Ok(())
}

#[test]
fn scatter_points_use_series_and_x_joined_by_cache_index() -> Result {
    let dir = tempfile::tempdir()?;
    for (x1, x2, name2, mismatch) in [
        ("1", "1", "Distance", true),
        ("1", "2", "Distance", false),
        ("1", "1", "Other", false),
        ("1", "1.0", "Distance", true),
    ] {
        call(
            dir.path(),
            json!({"op":"create","output":"scatter.pptx","slides":[
                {"title":"First","chart":{"type":"scatter","labels":[x1],"series":[{"name":"Distance","values":[10]}]}},
                {"title":"Second","chart":{"type":"scatter","labels":[x2],"series":[{"name":name2,"values":[20]}]}}
            ]}),
        )?;
        let base = parts(&dir.path().join("scatter.pptx"))?;
        for sparse in [false, true] {
            let mut p = base.clone();
            for i in 1..=2 {
                let name = format!("ppt/charts/chart{i}.xml");
                let chart = xml(&p, &name)?;
                let start = chart.find("<c:xVal>").ok_or("no X")?;
                let end = chart.find("</c:xVal>").ok_or("no X end")? + "</c:xVal>".len();
                let x = chart[start..end].replace("idx=\"0\"", "idx=\"7\"");
                let chart = format!("{}{}{}", &chart[..start], x, &chart[end..]);
                let chart = if sparse {
                    chart.replace("idx=\"0\"", "idx=\"7\"")
                } else {
                    chart
                };
                p.insert(name, chart.into_bytes());
            }
            pack(&dir.path().join("indexed.pptx"), &p)?;
            let check = call(dir.path(), json!({"op":"check","input":"indexed.pptx"}))?;
            let findings = check.data.ok_or("no findings")?;
            assert_eq!(
                findings.to_string().contains("check.metric_mismatch"),
                mismatch && sparse,
                "{findings}"
            );
        }
    }
    Ok(())
}
