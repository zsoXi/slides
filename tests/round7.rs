use serde_json::{Value, json};
use slides::{Input, Options, Output, run};
use std::collections::BTreeMap;
use std::io::{Cursor, Read, Write};
use std::path::Path;
use std::process::Command;

type Result = std::result::Result<(), Box<dyn std::error::Error>>;
fn call(dir: &Path, value: Value) -> std::result::Result<Output, slides::Error> {
    run(
        &serde_json::from_value::<Input>(value)
            .map_err(|source| slides::Error::CheckJson { source })?,
        &Options::new(dir),
    )
}
fn parts(
    path: &Path,
) -> std::result::Result<BTreeMap<String, Vec<u8>>, Box<dyn std::error::Error>> {
    let mut zip = zip::ZipArchive::new(Cursor::new(std::fs::read(path)?))?;
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
fn pack(path: &Path, parts: &BTreeMap<String, Vec<u8>>) -> Result {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes) in parts {
        // Stored entries keep this test about XML depth rather than compression ratio.
        zip.start_file(
            name,
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored),
        )?;
        zip.write_all(bytes)?;
    }
    std::fs::write(path, zip.finish()?.into_inner())?;
    Ok(())
}
fn xml(
    p: &BTreeMap<String, Vec<u8>>,
    name: &str,
) -> std::result::Result<String, Box<dyn std::error::Error>> {
    Ok(String::from_utf8(
        p.get(name).ok_or("missing part")?.clone(),
    )?)
}

#[test]
fn deeply_nested_xml_returns_json_without_stack_overflow() -> Result {
    let dir = tempfile::tempdir()?;
    call(
        dir.path(),
        json!({"op":"create","output":"deep.pptx","markdown":"# Control"}),
    )?;
    let mut p = parts(&dir.path().join("deep.pptx"))?;
    let slide = xml(&p, "ppt/slides/slide1.xml")?;
    let nested = format!(
        "{}{}",
        "<p:grpSp>".repeat(100_000),
        "</p:grpSp>".repeat(100_000)
    );
    p.insert(
        "ppt/slides/slide1.xml".into(),
        slide
            .replace("</p:spTree>", &(nested + "</p:spTree>"))
            .into_bytes(),
    );
    pack(&dir.path().join("deep.pptx"), &p)?;
    for op in [
        json!({"op":"read","input":"deep.pptx"}),
        json!({"op":"edit","input":"deep.pptx","output":"out.pptx","replace":{"find":"Control","replace":"Updated"}}),
        json!({"op":"check","input":"deep.pptx"}),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_slides"))
            .current_dir(dir.path())
            .arg(op.to_string())
            .output()?;
        assert!(!output.status.success());
        let response: Value = serde_json::from_slice(&output.stdout)?;
        assert_eq!(response["summary_code"], "xml.depth_limit");
    }
    Ok(())
}

#[test]
fn bounded_group_trees_remain_readable_and_ancillary_depth_errors_are_not_suppressed() -> Result {
    let dir = tempfile::tempdir()?;
    call(
        dir.path(),
        json!({"op":"create","output":"base.pptx","slides":[{"title":"Control","notes":"A note"}]}),
    )?;
    let base = parts(&dir.path().join("base.pptx"))?;
    let mut p = base.clone();
    let slide = xml(&p, "ppt/slides/slide1.xml")?;
    let grouped = slide
        .replacen("<p:sp>", &("<p:grpSp>".repeat(240) + "<p:sp>"), 1)
        .replace("</p:spTree>", &("</p:grpSp>".repeat(240) + "</p:spTree>"));
    p.insert("ppt/slides/slide1.xml".into(), grouped.into_bytes());
    pack(&dir.path().join("bounded.pptx"), &p)?;
    for _ in 0..2 {
        let read = call(dir.path(), json!({"op":"read","input":"bounded.pptx"}))?;
        assert_eq!(read.data.ok_or("no slides")?[0]["title"], "Control");
        call(
            dir.path(),
            json!({"op":"edit","input":"bounded.pptx","output":"bounded.pptx","replace":{"find":"Control","replace":"Control"}}),
        )?;
    }
    for name in [
        "ppt/notesSlides/notesSlide1.xml",
        "ppt/slideLayouts/slideLayout1.xml",
    ] {
        let mut p = base.clone();
        p.insert(
            name.into(),
            format!("{}{}", "<x>".repeat(100_000), "</x>".repeat(100_000)).into_bytes(),
        );
        pack(&dir.path().join("ancillary.pptx"), &p)?;
        let error = call(dir.path(), json!({"op":"read","input":"ancillary.pptx"}))
            .err()
            .ok_or("ignored ancillary depth error")?;
        assert_eq!(error.code(), "xml.depth_limit");
    }
    Ok(())
}

#[test]
fn cdata_plain_and_mixed_text_read_convert_and_replace_equally() -> Result {
    let dir = tempfile::tempdir()?;
    let title = "Northstar & Analytics";
    call(
        dir.path(),
        json!({"op":"create","output":"base.pptx","slides":[{"title":title}]}),
    )?;
    let original = parts(&dir.path().join("base.pptx"))?;
    for encoding in [
        "Northstar &amp; Analytics",
        "<![CDATA[Northstar & Analytics]]>",
        "North<![CDATA[star &]]> Analytics",
    ] {
        let mut p = original.clone();
        p.insert(
            "ppt/slides/slide1.xml".into(),
            xml(&p, "ppt/slides/slide1.xml")?
                .replace("Northstar &amp; Analytics", encoding)
                .into_bytes(),
        );
        pack(&dir.path().join("source.pptx"), &p)?;
        let read = call(dir.path(), json!({"op":"read","input":"source.pptx"}))?;
        assert_eq!(read.data.ok_or("no data")?[0]["title"], title);
        call(
            dir.path(),
            json!({"op":"convert","input":"source.pptx","output":"source.md"}),
        )?;
        assert!(std::fs::read_to_string(dir.path().join("source.md"))?.contains(title));
        call(
            dir.path(),
            json!({"op":"edit","input":"source.pptx","output":"edited.pptx","replace":{"find":title,"replace":"Updated analytics"}}),
        )?;
        let read = call(dir.path(), json!({"op":"read","input":"edited.pptx"}))?;
        assert_eq!(read.data.ok_or("no data")?[0]["title"], "Updated analytics");
    }
    Ok(())
}

#[test]
fn xml_control_characters_are_rejected_before_any_write() -> Result {
    let dir = tempfile::tempdir()?;
    call(
        dir.path(),
        json!({"op":"create","output":"safe.pptx","markdown":"# Safe"}),
    )?;
    let before = std::fs::read(dir.path().join("safe.pptx"))?;
    for control in [
        '\0', '\u{1}', '\u{8}', '\u{b}', '\u{c}', '\u{1f}', '\u{fffe}', '\u{ffff}',
    ] {
        let bad = format!("Bad{control}text");
        for field in ["title", "subtitle", "text", "notes", "image", "layout"] {
            let mut slide = json!({});
            slide[field] = json!(bad);
            let error = call(
                dir.path(),
                json!({"op":"create","output":"safe.pptx","slides":[slide]}),
            )
            .err()
            .ok_or("accepted invalid field")?;
            assert_eq!(error.code(), "input.xml_character");
            assert_eq!(error.params()["field"], format!("/slides/0/{field}"));
            assert_eq!(std::fs::read(dir.path().join("safe.pptx"))?, before);
        }
        for (request, field) in [
            (
                json!({"op":"create","output":"safe.pptx","slides":[{"title":bad}]}),
                "/slides/0/title",
            ),
            (
                json!({"op":"create","output":"safe.pptx","slides":[{"table":{"headers":[bad],"rows":[]}}]}),
                "/slides/0/table/headers/0",
            ),
            (
                json!({"op":"create","output":"safe.pptx","slides":[{"chart":{"type":"column","labels":[bad],"series":[[1.0]]}}]}),
                "/slides/0/chart/labels/0",
            ),
            (
                json!({"op":"create","output":"safe.pptx","slides":[{"sources":[bad]}]}),
                "/slides/0/sources/0",
            ),
            (
                json!({"op":"edit","input":"safe.pptx","output":"safe.pptx","replace":{"find":"Safe","replace":bad}}),
                "/replace/replace",
            ),
            (
                json!({"op":"edit","input":"safe.pptx","output":"safe.pptx","slides":[{"index":0,"notes":bad}]}),
                "/slides/0/notes",
            ),
            (
                json!({"op":"edit","input":"safe.pptx","output":"safe.pptx","add":[{"text":bad}]}),
                "/add/0/text",
            ),
        ] {
            let error = call(dir.path(), request)
                .err()
                .ok_or("accepted invalid XML text")?;
            assert_eq!(error.code(), "input.xml_character");
            assert_eq!(error.params()["field"], field);
            assert_eq!(std::fs::read(dir.path().join("safe.pptx"))?, before);
        }
        std::fs::write(dir.path().join("bad.md"), format!("# {bad}"))?;
        let error = call(
            dir.path(),
            json!({"op":"convert","input":"bad.md","output":"safe.pptx"}),
        )
        .err()
        .ok_or("converted invalid text")?;
        assert_eq!(error.code(), "input.xml_character");
        assert_eq!(std::fs::read(dir.path().join("safe.pptx"))?, before);
    }
    let output = Command::new(env!("CARGO_BIN_EXE_slides")).current_dir(dir.path()).arg(json!({"op":"edit","input":"safe.pptx","output":"safe.pptx","replace":{"find":"Safe","replace":"Bad\0title"}}).to_string()).output()?;
    assert!(!output.status.success());
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(response["summary_code"], "input.xml_character");
    assert_eq!(response["summary_params"]["field"], "/replace/replace");
    assert_eq!(std::fs::read(dir.path().join("safe.pptx"))?, before);
    call(
        dir.path(),
        json!({"op":"create","output":"valid.pptx","slides":[{"title":"Zażółć gęślą jaźń\t & < > 😀","text":"Line\nLine\rLine\u{85}"}]}),
    )?;
    Ok(())
}

#[test]
fn read_cursor_visits_all_slides_in_every_representation() -> Result {
    let dir = tempfile::tempdir()?;
    let slides: Vec<_> = (0..10)
        .map(|i| json!({"title":format!("Slide {i}")}))
        .collect();
    call(
        dir.path(),
        json!({"op":"create","output":"pages.pptx","slides":slides}),
    )?;
    for representation in ["text", "data", "both"] {
        let mut offset = 0;
        let mut pages = 0;
        loop {
            let read = call(
                dir.path(),
                json!({"op":"read","input":"pages.pptx","representation":representation,"offset":offset,"limit":2}),
            )?;
            let meta = read.truncation.ok_or("no metadata")?;
            assert_eq!(meta["page_slides"], 2);
            if representation != "data" {
                let text = read.text.ok_or("no text")?;
                for i in offset..offset + 2 {
                    assert!(text.contains(&format!("Slide {i}")));
                }
            }
            pages += 1;
            if meta["next_offset"].is_null() {
                break;
            }
            let next = meta["next_offset"].as_u64().ok_or("bad cursor")? as usize;
            assert_eq!(next, offset + 2);
            offset = next;
            assert!(pages <= 5);
        }
        assert_eq!(pages, 5);
    }
    let read = call(
        dir.path(),
        json!({"op":"read","input":"pages.pptx","representation":"text","limit":2,"max_chars":0}),
    )?;
    let meta = read.truncation.ok_or("no metadata")?;
    assert_eq!(meta["text"], true);
    assert_eq!(meta["next_offset"], 2);
    assert_eq!(meta["total_slides"], 10);
    Ok(())
}

#[test]
fn chart_periods_follow_cache_indices_and_missing_categories_are_not_guessed() -> Result {
    let dir = tempfile::tempdir()?;
    for (first, second, mismatch) in [
        ("Q1 2027", "Q2 2027", false),
        ("Q1 2027", "Q1 2027", true),
        ("2027", "2028", false),
    ] {
        let slides: Vec<_> = [(first,10.0),(second,20.0)].iter().map(|(label,value)|json!({"title":"Revenue","chart":{"type":"column","labels":[label],"series":[{"name":"Revenue","values":[value]}]}})).collect();
        call(
            dir.path(),
            json!({"op":"create","output":"periods.pptx","slides":slides}),
        )?;
        let check = call(dir.path(), json!({"op":"check","input":"periods.pptx"}))?;
        let findings = check.data.ok_or("no findings")?;
        assert_eq!(
            findings.to_string().contains("check.metric_mismatch"),
            mismatch,
            "{findings}"
        );
        let mut p = parts(&dir.path().join("periods.pptx"))?;
        for i in 1..=2 {
            let name = format!("ppt/charts/chart{i}.xml");
            // Category and value caches are sparse; only matching idx values form an identity.
            let chart = xml(&p, &name)?;
            let start = chart.find("<c:cat>").ok_or("no categories")?;
            let end = chart.find("</c:cat>").ok_or("no categories end")? + "</c:cat>".len();
            let category = chart[start..end].replace("idx=\"0\"", "idx=\"7\"");
            let chart = format!("{}{}{}", &chart[..start], category, &chart[end..]);
            p.insert(name, chart.into_bytes());
        }
        pack(&dir.path().join("missing.pptx"), &p)?;
        let check = call(dir.path(), json!({"op":"check","input":"missing.pptx"}))?;
        assert!(
            !check
                .data
                .ok_or("no findings")?
                .to_string()
                .contains("check.metric_mismatch")
        );
        for (name, data) in &mut p {
            if name.starts_with("ppt/charts/chart") && name.ends_with(".xml") {
                *data = String::from_utf8(data.clone())?
                    .replace("idx=\"0\"", "idx=\"7\"")
                    .into_bytes();
            }
        }
        pack(&dir.path().join("matched-sparse.pptx"), &p)?;
        let check = call(
            dir.path(),
            json!({"op":"check","input":"matched-sparse.pptx"}),
        )?;
        assert_eq!(
            check
                .data
                .ok_or("no findings")?
                .to_string()
                .contains("check.metric_mismatch"),
            mismatch
        );
    }
    Ok(())
}
