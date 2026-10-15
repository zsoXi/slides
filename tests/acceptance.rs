//! Supported authoring scenarios; fixtures are generated locally, never downloaded.
use serde_json::{Value, json};
use slides::{Input, Options, Output, run};
use std::collections::BTreeMap;
use std::io::{Cursor, Read, Write};
use std::path::Path;
use std::process::Command;

type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

fn call(base: &Path, request: Value) -> Result<Output> {
    Ok(run(
        &serde_json::from_value::<Input>(request)?,
        &Options::new(base),
    )?)
}
fn parts(path: &Path) -> Result<BTreeMap<String, Vec<u8>>> {
    let mut zip = zip::ZipArchive::new(Cursor::new(std::fs::read(path)?))?;
    let mut result = BTreeMap::new();
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes)?;
        result.insert(entry.name()?.into_owned(), bytes);
    }
    Ok(result)
}
fn image(base: &Path) -> Result {
    let raw = printpdf::RawImage {
        pixels: printpdf::RawImageData::U8(vec![90; 240 * 120 * 3]),
        width: 240,
        height: 120,
        data_format: printpdf::RawImageFormat::RGB8,
        tag: Vec::new(),
    };
    std::fs::write(
        base.join("office.png"),
        raw.encode_to_bytes(&[printpdf::OutputImageFormat::Png])?.0,
    )?;
    Ok(())
}
fn outline() -> Value {
    json!([
        {"layout":"title","title":"Quarterly business review 2027","subtitle":"A focused growth plan"},
        {"title":"Agenda","bullets":["Business performance","  Regional revenue","  Product adoption","Next quarter"]},
        {"layout":"image","title":"Our workplace","image":"office.png","text":"A collaborative workspace"},
        {"title":"Regional revenue","table":{"headers":["Region","Q1","Q2","Q3","Q4","Total"],"rows":[["North","10","20","30","40","100"],["South","15","25","35","45","120"],["Total","25","45","65","85","220"]]}},
        {"title":"Revenue by region","chart":{"type":"bar","labels":["North","South"],"series":[{"name":"Revenue","values":[100,120]}]}},
        {"title":"Revenue trend","chart":{"type":"line","labels":["Q1","Q2","Q3","Q4"],"series":[{"name":"Revenue","values":[25,45,65,85]}]}},
        {"layout":"two_content","title":"Strategic comparison","bullets":["Current: stable delivery","Current: local teams","Next: automated delivery","Next: shared expertise"]},
        {"title":"Thank you","text":"Questions and next steps","notes":"Discuss investment priorities with the board."}
    ])
}
fn markdown() -> &'static str {
    "# Quarterly business review 2027\n\nA focused growth plan\n\n## Agenda\n- Business performance\n  - Regional revenue\n  - Product adoption\n- Next quarter\n\n## Our workplace\n<!-- layout: image -->\n![](office.png)\n\nA collaborative workspace\n\n## Regional revenue\n| Region | Q1 | Q2 | Q3 | Q4 | Total |\n|---|---|---|---|---|---|\n| North | 10 | 20 | 30 | 40 | 100 |\n| South | 15 | 25 | 35 | 45 | 120 |\n| Total | 25 | 45 | 65 | 85 | 220 |\n\n## Revenue by region\n```chart\n{\"type\":\"bar\",\"labels\":[\"North\",\"South\"],\"series\":[{\"name\":\"Revenue\",\"values\":[100,120]}]}\n```\n\n## Revenue trend\n```chart\n{\"type\":\"line\",\"labels\":[\"Q1\",\"Q2\",\"Q3\",\"Q4\"],\"series\":[{\"name\":\"Revenue\",\"values\":[25,45,65,85]}]}\n```\n\n## Strategic comparison\n<!-- layout: two_content -->\n- Current: stable delivery\n- Current: local teams\n- Next: automated delivery\n- Next: shared expertise\n\n## Thank you\nQuestions and next steps\n\n> Discuss investment priorities with the board.\n"
}
fn source(base: &Path) -> Result {
    image(base)?;
    let output = call(
        base,
        json!({"op":"create","output":"review.pptx","slides":outline()}),
    )?;
    assert_eq!(output.summary_params["count"], "8");
    assert!(output.warnings.is_empty(), "{:?}", output.warnings);
    Ok(())
}
fn text_values(bytes: &[u8], tag: &str) -> Result<Vec<String>> {
    let mut reader = quick_xml::Reader::from_reader(bytes);
    let mut result = Vec::new();
    loop {
        match reader.read_event()? {
            quick_xml::events::Event::Start(e) if e.name().as_ref() == tag => result.push(
                reader
                    .read_text(e.name())?
                    .xml_content(quick_xml::XmlVersion::default())
                    .into_owned(),
            ),
            quick_xml::events::Event::Eof => break,
            _ => {}
        }
    }
    Ok(result)
}
fn check_content(base: &Path, name: &str) -> Result {
    let p = parts(&base.join(name))?;
    let all_text: Vec<String> = p
        .iter()
        .filter(|(name, _)| name.ends_with(".xml"))
        .map(|(_, bytes)| text_values(bytes, "a:t"))
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect();
    let joined = all_text.join("\n");
    for slide in outline().as_array().ok_or("outline")? {
        for field in ["title", "subtitle", "text", "notes"] {
            if let Some(text) = slide[field].as_str() {
                assert!(joined.contains(text), "missing {field}: {text}");
            }
        }
        for bullet in slide["bullets"].as_array().into_iter().flatten() {
            assert!(joined.contains(bullet.as_str().ok_or("bullet")?.trim()));
        }
        for cell in slide["table"]["headers"]
            .as_array()
            .into_iter()
            .flatten()
            .chain(
                slide["table"]["rows"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .flat_map(|r| r.as_array().into_iter().flatten()),
            )
        {
            assert!(
                all_text.iter().any(|v| v == cell.as_str().unwrap_or("")),
                "missing cell {cell}"
            );
        }
    }
    let charts: Vec<_> = p
        .iter()
        .filter(|(name, _)| name.contains("/charts/chart") && name.ends_with(".xml"))
        .collect();
    assert_eq!(charts.len(), 2);
    assert!(std::str::from_utf8(charts[0].1)?.contains("<c:min val=\"0\"/>"));
    for ((_, bytes), expected) in charts.iter().zip([
        vec!["Revenue", "North", "South", "100", "120"],
        vec!["Revenue", "Q1", "Q2", "Q3", "Q4", "25", "45", "65", "85"],
    ]) {
        assert_eq!(text_values(bytes, "c:v")?, expected);
    }
    assert_eq!(p.keys().filter(|n| n.ends_with(".xlsx")).count(), 2);
    assert!(
        p.values()
            .any(|v| v == &std::fs::read(base.join("office.png")).unwrap_or_default())
    );
    let slides: Vec<_> = p
        .iter()
        .filter(|(n, _)| n.contains("/slides/slide") && n.ends_with(".xml"))
        .collect();
    assert_eq!(slides.len(), 8);
    let agenda = std::str::from_utf8(slides[1].1)?;
    assert!(agenda.contains("lvl=\"1\""));
    let photo = std::str::from_utf8(slides[2].1)?;
    let pic = photo
        .split("<p:pic>")
        .nth(1)
        .ok_or("picture")?
        .split("</p:pic>")
        .next()
        .ok_or("picture end")?;
    let mut reader = quick_xml::Reader::from_str(pic);
    let mut dimensions = None;
    loop {
        match reader.read_event()? {
            quick_xml::events::Event::Empty(e) if e.name().as_ref() == "a:ext" => {
                let mut x = 0.;
                let mut y = 0.;
                for attr in e.attributes() {
                    let a = attr?;
                    let value = a.value.parse::<f64>()?;
                    if a.key.as_ref() == "cx" {
                        x = value;
                    }
                    if a.key.as_ref() == "cy" {
                        y = value;
                    }
                }
                dimensions = Some((x, y));
                break;
            }
            quick_xml::events::Event::Eof => break,
            _ => {}
        }
    }
    let (x, y) = dimensions.ok_or("picture size")?;
    assert!((x / y - 2.).abs() < 0.001);
    Ok(())
}
fn render(base: &Path, input: &str) -> Result<String> {
    call(
        base,
        json!({"op":"convert","input":input,"output":"review.pdf"}),
    )?;
    let out = Command::new("pdftotext")
        .args(["-layout"])
        .arg(base.join("review.pdf"))
        .arg("-")
        .output()?;
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout)?;
    assert_eq!(text.matches('\u{c}').count(), 8);
    let bounds = Command::new("pdftotext")
        .arg("-bbox")
        .arg(base.join("review.pdf"))
        .arg("-")
        .output()?;
    assert!(bounds.status.success());
    let bounds = String::from_utf8(bounds.stdout)?;
    let mut reader = quick_xml::Reader::from_str(&bounds);
    let (mut width, mut height) = (0., 0.);
    loop {
        match reader.read_event()? {
            quick_xml::events::Event::Start(e)
                if e.name().as_ref() == "page" || e.name().as_ref() == "word" =>
            {
                for attr in e.attributes() {
                    let a = attr?;
                    let value = a.value.parse::<f64>()?;
                    match a.key.as_ref() {
                        "width" => width = value,
                        "height" => height = value,
                        "xMin" | "yMin" => assert!(value >= 0., "negative text bound"),
                        "xMax" => assert!(value <= width, "text exceeds slide width"),
                        "yMax" => assert!(value <= height, "text exceeds slide height"),
                        _ => {}
                    }
                }
            }
            quick_xml::events::Event::Eof => break,
            _ => {}
        }
    }
    let png = Command::new("pdftoppm")
        .args(["-scale-to", "800", "-png"])
        .arg(base.join("review.pdf"))
        .arg(base.join("review"))
        .output()?;
    assert!(png.status.success());
    for i in 1..=8 {
        let bytes = std::fs::read(base.join(format!("review-{i}.png")))?;
        let image = printpdf::RawImage::decode_from_bytes(&bytes, &mut Vec::new())?;
        assert_eq!(image.width, 800);
        assert!(image.height > 400);
        if let Some(destination) = std::env::var_os("SLIDES_TEST_ARTIFACTS") {
            let folder =
                std::path::PathBuf::from(destination).join(input.trim_end_matches(".pptx"));
            std::fs::create_dir_all(&folder)?;
            std::fs::write(folder.join(format!("slide-{i}.png")), bytes)?;
        }
    }
    Ok(text)
}

#[test]
fn s1_outline_json_to_pptx() -> Result {
    let dir = tempfile::tempdir()?;
    source(dir.path())?;
    check_content(dir.path(), "review.pptx")?;
    render(dir.path(), "review.pptx")?;
    Ok(())
}
#[test]
fn s2_markdown_to_pptx() -> Result {
    let dir = tempfile::tempdir()?;
    image(dir.path())?;
    call(
        dir.path(),
        json!({"op":"create","output":"review.pptx","markdown":markdown()}),
    )?;
    check_content(dir.path(), "review.pptx")?;
    render(dir.path(), "review.pptx")?;
    Ok(())
}
#[test]
fn s3_pptx_to_pdf() -> Result {
    let dir = tempfile::tempdir()?;
    source(dir.path())?;
    let text = render(dir.path(), "review.pptx")?;
    for title in outline()
        .as_array()
        .ok_or("outline")?
        .iter()
        .filter_map(|s| s["title"].as_str())
    {
        assert!(text.contains(title), "missing PDF title: {title}");
    }
    for number in [
        "2027", "10", "20", "30", "40", "100", "15", "25", "35", "45", "120", "65", "85", "220",
    ] {
        assert!(
            text.split_whitespace().any(|v| v == number),
            "missing PDF number {number}"
        );
    }
    Ok(())
}
#[test]
fn s4_company_template() -> Result {
    let dir = tempfile::tempdir()?;
    source(dir.path())?;
    let mut template = parts(&dir.path().join("review.pptx"))?;
    for (name, bytes) in &mut template {
        if name.contains("theme/")
            || name.contains("slideMasters/")
            || name.contains("slideLayouts/")
        {
            *bytes = String::from_utf8(bytes.clone())?
                .replace("Calibri Light", "DejaVu Serif")
                .replace("Calibri", "DejaVu Sans")
                .replace("2F5597", "176B57")
                .into_bytes();
        }
    }
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes) in &template {
        zip.start_file(name, zip::write::SimpleFileOptions::default())?;
        zip.write_all(bytes)?;
    }
    std::fs::write(dir.path().join("company.pptx"), zip.finish()?.into_inner())?;
    let result = call(
        dir.path(),
        json!({"op":"create","output":"company-review.pptx","template":"company.pptx","slides":outline()}),
    )?;
    assert_eq!(result.summary_params["count"], "8");
    check_content(dir.path(), "company-review.pptx")?;
    let actual = parts(&dir.path().join("company-review.pptx"))?;
    for (name, bytes) in template
        .iter()
        .filter(|(n, _)| n.contains("theme/") || n.contains("slideMasters/"))
    {
        assert_eq!(
            actual.get(name),
            Some(bytes),
            "company styling changed: {name}"
        );
    }
    assert!(
        actual
            .values()
            .any(|bytes| String::from_utf8_lossy(bytes).contains("DejaVu Serif"))
    );
    assert!(
        actual
            .values()
            .any(|bytes| String::from_utf8_lossy(bytes).contains("176B57"))
    );
    render(dir.path(), "company-review.pptx")?;
    Ok(())
}
