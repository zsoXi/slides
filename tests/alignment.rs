//! End-to-end alignment checks for ordinary and company-template exports.
use std::collections::BTreeMap;
use std::io::{Cursor, Read, Write};
use std::path::Path;
use std::process::Command;

use quick_xml::events::Event;
use serde_json::{Value, json};
use slides::{Input, Options, run};

type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

fn call(base: &Path, request: Value) -> Result {
    run(
        &serde_json::from_value::<Input>(request)?,
        &Options::new(base),
    )?;
    Ok(())
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
fn pack(path: &Path, parts: &BTreeMap<String, Vec<u8>>) -> Result {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes) in parts {
        zip.start_file(name, zip::write::SimpleFileOptions::default())?;
        zip.write_all(bytes)?;
    }
    std::fs::write(path, zip.finish()?.into_inner())?;
    Ok(())
}
fn attr(xml: &str, tag: &str, name: &str) -> Result<Option<String>> {
    let mut reader = quick_xml::Reader::from_str(xml);
    loop {
        match reader.read_event()? {
            Event::Start(e) | Event::Empty(e) if e.name().as_ref() == tag => {
                for a in e.attributes() {
                    let a = a?;
                    if a.key.as_ref() == name {
                        return Ok(Some(
                            a.normalized_value(quick_xml::XmlVersion::default())?
                                .into_owned(),
                        ));
                    }
                }
                return Ok(None);
            }
            Event::Eof => return Ok(None),
            _ => {}
        }
    }
}
fn shapes(xml: &str, tag: &str) -> Vec<String> {
    xml.split(&format!("<{tag}>"))
        .skip(1)
        .filter_map(|s| s.split(&format!("</{tag}>")).next())
        .map(str::to_string)
        .collect()
}
fn slide(parts: &BTreeMap<String, Vec<u8>>, n: usize) -> Result<String> {
    let suffix = format!("ppt/slides/slide{n}.xml");
    Ok(String::from_utf8(
        parts
            .iter()
            .find(|(p, _)| p.ends_with(&suffix))
            .ok_or("missing slide")?
            .1
            .clone(),
    )?)
}
fn body(xml: &str) -> Result<String> {
    shapes(xml, "p:sp")
        .into_iter()
        .find(|s| attr(s, "p:ph", "type").ok().flatten().as_deref() == Some("body"))
        .ok_or_else(|| "missing body".into())
}
fn fixture() -> Value {
    json!([
        {"title":"Our priorities","short_slide_accent":false,"bullets":["Deliver a simpler experience","Grow recurring revenue","Invest in reliability"]},
        {"title":"Investment plan","table":{"headers":["Initiative","Budget"],"rows":[["Quality","120"],["Growth","85"]]}},
        {"title":"Revenue by quarter","chart":{"type":"column","labels":["Q1","Q2"],"series":[[24,38]]}},
        {"title":"Operating principles","text":"Keep a clear focus on customer value."},
        {"layout":"title","title":"Strategy","subtitle":"A focused plan"},
        {"layout":"section","title":"Next chapter"},
        {"layout":"quote","title":"Our principle","text":"Clear priorities deliver lasting results."}
    ])
}
// Strip only body anchors; title and subtitle anchors remain deliberate exceptions.
fn template(base: &Path, mode: &str) -> Result<BTreeMap<String, Vec<u8>>> {
    call(
        base,
        json!({"op":"create","output":"seed.pptx","theme":"corporate","slides":fixture()}),
    )?;
    let mut parts = unpack(&base.join("seed.pptx"))?;
    for (name, bytes) in &mut parts {
        if !(name.starts_with("ppt/slideLayouts/") || name.starts_with("ppt/slideMasters/"))
            || !name.ends_with(".xml")
        {
            continue;
        }
        let old = String::from_utf8(bytes.clone())?;
        let mut new = old.clone();
        for shape in shapes(&old, "p:sp") {
            if attr(&shape, "p:ph", "type")?.as_deref() == Some("body") {
                let value = if matches!(mode, "layout" | "both")
                    && name.starts_with("ppt/slideLayouts/")
                {
                    " anchor=\"b\""
                } else if matches!(mode, "master" | "both") && name.starts_with("ppt/slideMasters/")
                {
                    " anchor=\"ctr\""
                } else {
                    ""
                };
                new = new.replace(&shape, &shape.replace(" anchor=\"t\"", value));
            }
        }
        if mode == "missing" {
            new = new.replace("<a:bodyPr/>", "");
        }
        *bytes = new.into_bytes();
    }
    pack(&base.join("company.pptx"), &parts)?;
    Ok(parts)
}

#[test]
fn ordinary_and_template_content_have_explicit_top_anchors_and_matching_object_tops() -> Result {
    let dir = tempfile::tempdir()?;
    let company = template(dir.path(), "default")?;
    for company_path in [Value::Null, json!("company.pptx")] {
        call(
            dir.path(),
            json!({"op":"create","output":"deck.pptx","template":company_path,"slides":fixture()}),
        )?;
        let parts = unpack(&dir.path().join("deck.pptx"))?;
        let first = body(&slide(&parts, 1)?)?;
        assert_eq!(attr(&first, "a:bodyPr", "anchor")?.as_deref(), Some("t"));
        let top = attr(&first, "a:off", "y")?.ok_or("no body top")?;
        for n in [2, 3] {
            let frame = shapes(&slide(&parts, n)?, "p:graphicFrame")
                .into_iter()
                .next()
                .ok_or("no object")?;
            assert_eq!(attr(&frame, "a:off", "y")?.as_deref(), Some(top.as_str()));
        }
        assert_eq!(
            attr(&body(&slide(&parts, 4)?)?, "a:bodyPr", "anchor")?.as_deref(),
            Some("t")
        );
        for n in [5, 6] {
            let title = shapes(&slide(&parts, n)?, "p:sp")
                .into_iter()
                .find(|s| attr(s, "p:ph", "type").ok().flatten().as_deref() == Some("ctrTitle"))
                .ok_or("no title")?;
            assert_eq!(attr(&title, "a:bodyPr", "anchor")?.as_deref(), Some("ctr"));
        }
        if !company_path.is_null() {
            for (name, bytes) in &company {
                if name.starts_with("ppt/slideLayouts/") || name.starts_with("ppt/slideMasters/") {
                    assert_eq!(parts.get(name), Some(bytes), "changed {name}");
                }
            }
        }
    }
    Ok(())
}

#[test]
fn explicit_company_anchors_override_defaults_in_layouts_and_masters() -> Result {
    let dir = tempfile::tempdir()?;
    for (mode, expected) in [
        ("layout", "b"),
        ("master", "ctr"),
        ("both", "b"),
        ("missing", "t"),
    ] {
        template(dir.path(), mode)?;
        call(
            dir.path(),
            json!({"op":"create","output":"deck.pptx","template":"company.pptx","slides":fixture()}),
        )?;
        let parts = unpack(&dir.path().join("deck.pptx"))?;
        for n in [1, 4] {
            assert_eq!(
                attr(&body(&slide(&parts, n)?)?, "a:bodyPr", "anchor")?.as_deref(),
                Some(expected),
                "{mode}, slide {n}"
            );
        }
    }
    Ok(())
}

fn render(base: &Path, pdf: &str, prefix: &str) -> Result {
    let out = Command::new("pdftoppm")
        .args(["-scale-to-x", "800", "-scale-to-y", "-1", "-png"])
        .arg(base.join(pdf))
        .arg(base.join(prefix))
        .output()?;
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(())
}
fn gap(path: &Path) -> Result<usize> {
    let image = printpdf::RawImage::decode_from_bytes(&std::fs::read(path)?, &mut Vec::new())?;
    let printpdf::RawImageData::U8(pixels) = image.pixels else {
        return Err("unexpected raster type".into());
    };
    assert_eq!(image.width, 800);
    let channels = pixels.len() / (image.width * image.height);
    assert!(channels >= 3);
    let pixel =
        |x: usize, y: usize| &pixels[(y * 800 + x) * channels..(y * 800 + x) * channels + 3];
    let background = pixel(790, 200);
    let ink = |y| {
        (39..761)
            .filter(|&x| {
                pixel(x, y)
                    .iter()
                    .zip(background)
                    .any(|(a, b)| a.abs_diff(*b) > 45)
            })
            .count()
            >= 3
    };
    let title_bottom = (20..90).rfind(|&y| ink(y)).ok_or("no title ink")?;
    let content_top = (95..400).find(|&y| ink(y)).ok_or("no content ink")?;
    assert!(content_top > title_bottom);
    Ok(content_top - title_bottom)
}

#[test]
fn libreoffice_png_content_starts_within_forty_pixels_of_title_in_both_paths() -> Result {
    let dir = tempfile::tempdir()?;
    template(dir.path(), "default")?;
    for theme in ["corporate", "light", "dark"] {
        for company in [false, true] {
            call(
                dir.path(),
                json!({"op":"create","output":"deck.pptx","theme":theme,"template":if company {json!("company.pptx")} else {Value::Null},"slides":fixture()}),
            )?;
            call(
                dir.path(),
                json!({"op":"convert","input":"deck.pptx","output":"deck.pdf"}),
            )?;
            render(dir.path(), "deck.pdf", "converted")?;
            if !company {
                call(
                    dir.path(),
                    json!({"op":"create","output":"direct.pdf","theme":theme,"slides":fixture()}),
                )?;
                render(dir.path(), "direct.pdf", "direct")?;
            }
            for n in 1..=4 {
                let measured = gap(&dir.path().join(format!("converted-{n}.png")))?;
                println!("{theme}, template={company}, slide {n}: gap={measured}px");
                assert!(
                    measured <= 40,
                    "{theme}, template={company}, slide {n}: gap={measured}px"
                );
                if !company {
                    let direct = gap(&dir.path().join(format!("direct-{n}.png")))?;
                    assert!(
                        direct <= 40,
                        "{theme}, direct PDF, slide {n}: gap={direct}px"
                    );
                }
            }
        }
    }
    Ok(())
}
