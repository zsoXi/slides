//! Top alignment, short-slide accents and safe fitting in existing Office packages.

use std::collections::BTreeMap;
use std::io::{Cursor, Read, Write};
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

fn parts(path: &Path) -> Result<BTreeMap<String, Vec<u8>>> {
    let mut archive = zip::ZipArchive::new(Cursor::new(std::fs::read(path)?))?;
    let mut parts = BTreeMap::new();
    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;
        let name = file.name()?.to_string();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        parts.insert(name, bytes);
    }
    Ok(parts)
}

fn xml<'a>(parts: &'a BTreeMap<String, Vec<u8>>, name: &str) -> Result<&'a str> {
    Ok(std::str::from_utf8(parts.get(name).ok_or("missing part")?)?)
}

fn shape<'a>(slide: &'a str, name: &str) -> Result<&'a str> {
    slide
        .split("<p:sp>")
        .find(|s| s.contains(&format!("name=\"{name}\"")))
        .and_then(|s| s.split("</p:sp>").next())
        .ok_or_else(|| "missing shape".into())
}

#[test]
fn ordinary_content_lists_tables_and_charts_start_below_the_title() -> Result {
    let dir = tempfile::tempdir()?;
    for theme in ["light", "dark", "corporate", "creative"] {
        call(
            dir.path(),
            json!({"op":"create","output":"grid.pptx","theme":theme,"slides":[
                {"title":"Short list","bullets":["One","Two","Three"]},
                {"title":"Small table","table":{"headers":["Metric","Value"],"rows":[["Growth","25%"]]}},
                {"title":"Results","chart":{"type":"column","labels":["Q1","Q2"],"series":[[2,4]]}},
                {"layout":"title","title":"A title"}, {"layout":"section","title":"A section"},
                {"layout":"quote","title":"A customer","text":"A useful quotation."}
            ]}),
        )?;
        let p = parts(&dir.path().join("grid.pptx"))?;
        let content = shape(xml(&p, "ppt/slides/slide1.xml")?, "Content Placeholder")?;
        assert!(content.contains("anchor=\"t\""));
        assert!(content.contains("y=\"1508760\""));
        let table = xml(&p, "ppt/slides/slide2.xml")?
            .split("<p:graphicFrame>")
            .nth(1)
            .ok_or("no table")?;
        assert!(table.contains("y=\"1508760\""));
        let chart = xml(&p, "ppt/slides/slide3.xml")?
            .split("<p:graphicFrame>")
            .nth(1)
            .ok_or("no chart")?;
        assert!(chart.contains("y=\"1508760\""));
        for n in [4, 5] {
            assert!(
                shape(xml(&p, &format!("ppt/slides/slide{n}.xml"))?, "Title")?
                    .contains("anchor=\"ctr\"")
            );
        }
    }
    Ok(())
}

#[test]
fn short_slide_panels_are_deterministic_and_can_be_disabled() -> Result {
    let dir = tempfile::tempdir()?;
    let slides = json!([
        {"title":"One priority","bullets":["A clear outcome"]},
        {"title":"Two priorities","bullets":["One","Two"]},
        {"title":"Three priorities","bullets":["One","Two","Three"]},
        {"title":"Plain","short_slide_accent":false,"bullets":["One","Two","Three"]},
        {"title":"Full","bullets":["One","Two","Three","Four"]}
    ]);
    for name in ["a.pptx", "b.pptx"] {
        call(
            dir.path(),
            json!({"op":"create","output":name,"slides":slides}),
        )?;
    }
    assert_eq!(
        std::fs::read(dir.path().join("a.pptx"))?,
        std::fs::read(dir.path().join("b.pptx"))?
    );
    let p = parts(&dir.path().join("a.pptx"))?;
    for n in 1..=5 {
        let s = xml(&p, &format!("ppt/slides/slide{n}.xml"))?;
        assert_eq!(
            s.matches("name=\"Theme accent\"").count(),
            if n <= 3 { 3 } else { 1 }
        );
        assert!(shape(s, "Content Placeholder")?.contains("anchor=\"t\""));
    }
    assert!(slides::schema().to_string().contains("short_slide_accent"));
    Ok(())
}

fn foreign_deck(base: &Path) -> Result {
    call(
        base,
        json!({"op":"create","output":"source.pptx","slides":[
            {"title":"Untouched","bullets":["Keep this slide"]},
            {"title":"Original heading","bullets":["Original body"],"notes":"Original note"},
            {"title":"Table","table":{"headers":["Name","Value"],"rows":[["Original cell","10"]]}}
        ]}),
    )?;
    std::fs::create_dir(base.join("office"))?;
    let result = Command::new("soffice")
        .arg(format!(
            "-env:UserInstallation=file://{}",
            base.join("profile").display()
        ))
        .args(["--headless", "--convert-to", "pptx", "--outdir"])
        .arg(base.join("office"))
        .arg(base.join("source.pptx"))
        .output()?;
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(base.join("office/source.pptx").exists());
    Ok(())
}

fn overflow_warning(result: &Output, slide: &str, total: usize) -> Result {
    let w = result
        .warnings
        .iter()
        .find(|w| w.code == "edit.text_overflow")
        .ok_or("missing overflow warning")?;
    assert_eq!(w.params["slide"], slide);
    let hidden = w.params["hidden_words"]
        .as_str()
        .ok_or("missing hidden_words")?
        .parse::<usize>()?;
    assert!(hidden > 0 && hidden <= total);
    assert!(!w.message.contains('{'));
    Ok(())
}

#[test]
fn foreign_replace_clips_at_minimum_warns_and_preserves_other_parts() -> Result {
    let dir = tempfile::tempdir()?;
    foreign_deck(dir.path())?;
    let result = call(
        dir.path(),
        json!({"op":"edit","input":"office/source.pptx","output":"edited.pptx","replace":{"find":"Original heading","replace":"Overflow ".repeat(300)}}),
    )?;
    overflow_warning(&result, "2", 300)?;
    let before = parts(&dir.path().join("office/source.pptx"))?;
    let after = parts(&dir.path().join("edited.pptx"))?;
    for (name, bytes) in &before {
        if name != "ppt/slides/slide2.xml" {
            assert_eq!(Some(bytes), after.get(name), "{name}");
        }
    }
    let s = xml(&after, "ppt/slides/slide2.xml")?;
    let title = s
        .split("<p:sp>")
        .find(|s| s.contains("<a:t>Overflow"))
        .and_then(|s| s.split("</p:sp>").next())
        .ok_or("missing edited title")?;
    assert!(title.contains("sz=\"1400\""));
    assert!(title.contains("<a:noAutofit"));
    assert!(title.contains("vertOverflow=\"clip\""));
    assert!(title.contains("horzOverflow=\"clip\""));
    assert_eq!(title.matches("Overflow ").count(), 300);
    // Actual renderer: clipped title words cannot overlap the body field below it.
    call(
        dir.path(),
        json!({"op":"convert","input":"edited.pptx","output":"edited.pdf"}),
    )?;
    let out = Command::new("pdftotext")
        .args(["-f", "2", "-l", "2", "-bbox"])
        .arg(dir.path().join("edited.pdf"))
        .arg("-")
        .output()?;
    assert!(out.status.success());
    let bbox = String::from_utf8(out.stdout)?;
    let mut reader = quick_xml::Reader::from_str(&bbox);
    let mut max_y = 0.0f64;
    let mut count = 0;
    loop {
        match reader.read_event()? {
            quick_xml::events::Event::Start(e) if e.name().as_ref() == "word" => {
                let mut y = 0.0;
                for attr in e.attributes() {
                    let a = attr?;
                    if a.key.as_ref() == "yMax" {
                        y = a.value.parse::<f64>()?;
                    }
                }
                if reader.read_text(e.name())?.as_ref() == "Overflow" {
                    count += 1;
                    max_y = max_y.max(y);
                }
            }
            quick_xml::events::Event::Eof => break,
            _ => {}
        }
    }
    assert!(count > 0 && count < 300, "{count}");
    assert!(max_y <= 101.0, "title bottom={max_y}");
    Ok(())
}

#[test]
fn foreign_slide_edits_fit_or_warn_after_replacements_and_deletions() -> Result {
    let dir = tempfile::tempdir()?;
    foreign_deck(dir.path())?;
    let request = json!({"op":"edit","input":"office/source.pptx","output":"edited.pptx",
        "replace":{"find":"Original heading","replace":"First intermediate edit"},
        "slides":[{"index":1,"title":"Final ".repeat(300),"bullets":["Body ".repeat(600)]}],"delete":[0]});
    let result = call(dir.path(), request.clone())?;
    overflow_warning(&result, "1", 600)?;
    assert_eq!(result.warnings.len(), 2);
    let p = parts(&dir.path().join("edited.pptx"))?;
    let s = xml(&p, "ppt/slides/slide2.xml")?;
    assert!(!s.contains("intermediate"));
    assert_eq!(s.matches("Final").count(), 300);
    assert_eq!(s.matches("Body ").count(), 600);
    let original = std::fs::read(dir.path().join("edited.pptx"))?;
    call(dir.path(), request.clone())?;
    assert_eq!(original, std::fs::read(dir.path().join("edited.pptx"))?);
    std::thread::scope(|scope| -> Result {
        let handles = (0..2)
            .map(|n| {
                let mut req = request.clone();
                req["output"] = json!(format!("parallel{n}.pptx"));
                let base = dir.path();
                scope.spawn(move || call(base, req).map(|_| ()).map_err(|e| e.to_string()))
            })
            .collect::<Vec<_>>();
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
fn edited_fields_that_fit_keep_typography_and_small_cells_warn() -> Result {
    let dir = tempfile::tempdir()?;
    foreign_deck(dir.path())?;
    let out = call(
        dir.path(),
        json!({"op":"edit","input":"office/source.pptx","output":"fit.pptx","slides":[{"index":1,"title":"A longer heading that still fits","bullets":["A clear outcome","Another useful outcome"]}]}),
    )?;
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    let p = parts(&dir.path().join("fit.pptx"))?;
    assert_eq!(
        xml(&p, "ppt/slides/slide2.xml")?
            .matches("<a:normAutofit")
            .count(),
        0,
        "text that fits must not change body properties"
    );
    let out = call(
        dir.path(),
        json!({"op":"edit","input":"office/source.pptx","output":"cell.pptx","replace":{"find":"Original cell","replace":"Cell ".repeat(500)}}),
    )?;
    overflow_warning(&out, "3", 500)?;
    Ok(())
}

#[test]
fn inherited_geometry_and_unknown_geometry_do_not_silently_skip_fitting() -> Result {
    let dir = tempfile::tempdir()?;
    call(
        dir.path(),
        json!({"op":"create","output":"source.pptx","slides":[{"title":"Original heading","bullets":["Short content"]}]}),
    )?;
    let mut p = parts(&dir.path().join("source.pptx"))?;
    // Remove local title geometry: it must be resolved from the slide layout.
    let original = xml(&p, "ppt/slides/slide1.xml")?.to_string();
    let title = shape(&original, "Title")?;
    let start = title.find("<a:xfrm>").ok_or("no transform")?;
    let end = title.find("</a:xfrm>").ok_or("no transform end")? + "</a:xfrm>".len();
    p.insert(
        "ppt/slides/slide1.xml".into(),
        original.replacen(&title[start..end], "", 1).into_bytes(),
    );
    for (filename, strip_layout) in [("inherited.pptx", false), ("unknown.pptx", true)] {
        let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, bytes) in &p {
            z.start_file(name, zip::write::SimpleFileOptions::default())?;
            let data =
                if strip_layout && name.contains("slideLayout2.xml") && !name.ends_with("rels") {
                    // Deliberately unavailable geometry must warn, not claim to fit.
                    String::from_utf8(bytes.clone())?
                        .replace("a:ext", "a:unavailable")
                        .into_bytes()
                } else {
                    bytes.clone()
                };
            z.write_all(&data)?;
        }
        std::fs::write(dir.path().join(filename), z.finish()?.into_inner())?;
        let result = call(
            dir.path(),
            json!({"op":"edit","input":filename,"output":"out.pptx","replace":{"find":"Original heading","replace":if strip_layout {"New title".to_string()} else {"Long ".repeat(300)}}}),
        )?;
        overflow_warning(&result, "1", if strip_layout { 2 } else { 300 })?;
    }
    Ok(())
}
