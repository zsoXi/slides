use serde_json::{Value, json};
use slides::{Input, Options, Output, run};
use std::collections::BTreeMap;
use std::io::{Cursor, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
type Result = std::result::Result<(), Box<dyn std::error::Error>>;

fn call(dir: &Path, value: Value) -> std::result::Result<Output, Box<dyn std::error::Error>> {
    Ok(run(
        &serde_json::from_value::<Input>(value)?,
        &Options::new(dir),
    )?)
}
fn parts(
    path: &Path,
) -> std::result::Result<BTreeMap<String, Vec<u8>>, Box<dyn std::error::Error>> {
    let mut zip = zip::ZipArchive::new(Cursor::new(std::fs::read(path)?))?;
    let mut parts = BTreeMap::new();
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let name = entry.name()?.to_string();
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes)?;
        parts.insert(name, bytes);
    }
    Ok(parts)
}
fn pack(path: &Path, parts: &BTreeMap<String, Vec<u8>>) -> Result {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, data) in parts {
        zip.start_file(
            name,
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated),
        )?;
        zip.write_all(data)?;
    }
    std::fs::write(path, zip.finish()?.into_inner())?;
    Ok(())
}
fn png(path: &Path) -> Result {
    let image = printpdf::RawImage {
        pixels: printpdf::RawImageData::U8(vec![180; 200 * 100 * 3]),
        width: 200,
        height: 100,
        data_format: printpdf::RawImageFormat::RGB8,
        tag: Vec::new(),
    };
    std::fs::write(
        path,
        image
            .encode_to_bytes(&[printpdf::OutputImageFormat::Png])?
            .0,
    )?;
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
fn maximum_relationship_id_allows_repeated_and_parallel_edits() -> Result {
    let dir = tempfile::tempdir()?;
    call(
        dir.path(),
        json!({"op":"create","output":"foreign.pptx","markdown":"# Control"}),
    )?;
    let mut p = parts(&dir.path().join("foreign.pptx"))?;
    let name = "ppt/_rels/presentation.xml.rels";
    let rels = xml(&p,name)?.replace("</Relationships>", "<Relationship Id=\"rId4294967295\" Type=\"http://example.org/opaque\" Target=\"opaque.xml\"/></Relationships>");
    p.insert(name.into(), rels.into_bytes());
    p.insert("ppt/opaque.xml".into(), b"<opaque/>".to_vec());
    pack(&dir.path().join("foreign.pptx"), &p)?;
    for _ in 0..2 {
        call(
            dir.path(),
            json!({"op":"edit","input":"foreign.pptx","output":"foreign.pptx","add":[{"title":"Added"}]}),
        )?;
    }
    std::thread::scope(|scope| -> Result {
        let mut jobs = Vec::new();
        for _ in 0..2 {
            let base = dir.path();
            jobs.push(scope.spawn(move || {
            run(&serde_json::from_value::<Input>(json!({"op":"edit","input":"foreign.pptx","output":"foreign.pptx","add":[{"title":"Parallel"}]})).map_err(|source| slides::Error::CheckJson { source })?, &Options::new(base))
        }));
        }
        for job in jobs {
            job.join().map_err(|_| "edit panicked")??;
        }
        Ok(())
    })?;
    let read = call(dir.path(), json!({"op":"read","input":"foreign.pptx"}))?;
    assert_eq!(
        read.data
            .ok_or("no slides")?
            .as_array()
            .ok_or("not array")?
            .len(),
        5
    );
    let p = parts(&dir.path().join("foreign.pptx"))?;
    assert_eq!(p["ppt/opaque.xml"], b"<opaque/>");
    let rels = xml(&p, name)?;
    let mut ids = std::collections::BTreeSet::new();
    let mut reader = quick_xml::Reader::from_str(&rels);
    loop {
        match reader.read_event()? {
            quick_xml::events::Event::Empty(e) | quick_xml::events::Event::Start(e)
                if e.name().as_ref() == "Relationship" =>
            {
                for a in e.attributes() {
                    let a = a?;
                    if a.key.as_ref() == "Id" {
                        assert!(ids.insert(a.value.into_owned()));
                    }
                }
            }
            quick_xml::events::Event::Eof => break,
            _ => {}
        }
    }
    Ok(())
}

#[test]
fn read_and_edit_reject_inflated_size_ratio_and_entry_count() -> Result {
    let dir = tempfile::tempdir()?;
    call(
        dir.path(),
        json!({"op":"create","output":"base.pptx","markdown":"# Control"}),
    )?;
    for kind in ["size", "ratio", "count"] {
        let mut p = parts(&dir.path().join("base.pptx"))?;
        let limit = match kind {
            "size" => {
                p.insert("large.bin".into(), b"bounded package ".repeat(20_000));
                100_000
            }
            "ratio" => {
                p.insert("large.bin".into(), vec![0; 4_000_000]);
                10_000_000
            }
            _ => {
                for i in 0..50_001 {
                    p.insert(format!("extra/{i}"), Vec::new());
                }
                10_000_000
            }
        };
        pack(&dir.path().join("hostile.pptx"), &p)?;
        assert!(std::fs::metadata(dir.path().join("hostile.pptx"))?.len() < limit);
        for request in [
            json!({"op":"read","input":"hostile.pptx"}),
            json!({"op":"edit","input":"hostile.pptx","output":"out.pptx"}),
        ] {
            let mut opts = Options::new(dir.path());
            opts.max_input_bytes = limit;
            let error = run(&serde_json::from_value::<Input>(request)?, &opts)
                .err()
                .ok_or("accepted hostile archive")?;
            assert_eq!(error.code(), "package.invalid", "{kind}: {error}");
        }
    }
    Ok(())
}

#[test]
fn create_bounds_both_sources_and_rejects_conflict_at_runtime() -> Result {
    let dir = tempfile::tempdir()?;
    let mut opts = Options::new(dir.path());
    opts.max_input_bytes = 30;
    for source in [
        json!({"markdown":"# ".to_string()+&"x".repeat(100)}),
        json!({"slides":[{"title":"x".repeat(100)}]}),
    ] {
        let mut request = json!({"op":"create","output":"out.pptx"});
        request
            .as_object_mut()
            .ok_or("not object")?
            .extend(source.as_object().ok_or("not object")?.clone());
        assert_eq!(
            run(&serde_json::from_value::<Input>(request)?, &opts)
                .err()
                .ok_or("accepted large source")?
                .code(),
            "input.too_large"
        );
    }
    assert_eq!(
        call(
            dir.path(),
            json!({"op":"create","output":"out.pptx","markdown":"# A","slides":[{"title":"B"}]})
        )
        .err()
        .ok_or("accepted conflict")?
        .downcast_ref::<slides::Error>()
        .ok_or("not tool error")?
        .code(),
        "input.create_source_conflict"
    );
    let schema = slides::schema();
    let create = schema["oneOf"]
        .as_array()
        .ok_or("no variants")?
        .iter()
        .find(|v| v["properties"]["op"]["const"] == "create")
        .ok_or("no create variant")?;
    assert!(create.get("oneOf").is_none());
    assert_eq!(create["properties"]["markdown"]["type"], "string");
    assert_eq!(create["properties"]["slides"]["type"], "array");
    Ok(())
}

#[test]
fn normal_and_two_line_hero_titles_remain_one_slide_in_all_themes_and_pdf() -> Result {
    let dir = tempfile::tempdir()?;
    for theme in ["light", "dark", "corporate", "creative"] {
        for title in [
            "Incident response essentials",
            "Incident response\nessentials",
        ] {
            for format in ["pptx", "pdf"] {
                let output = call(
                    dir.path(),
                    json!({"op":"create","theme":theme,"output":format!("title.{format}"),"slides":[{"layout":"title","title":title}]}),
                )?;
                assert_eq!(output.summary_params["count"], "1", "{theme} {title}");
                assert!(
                    !output
                        .warnings
                        .iter()
                        .any(|w| matches!(w.code, "slide.split" | "slide.title_overflow"))
                );
                if format == "pptx" {
                    let p = parts(&dir.path().join("title.pptx"))?;
                    let slide = xml(&p, "ppt/slides/slide1.xml")?;
                    assert!(
                        ["4000", "4200", "4400"]
                            .iter()
                            .any(|s| slide.contains(&format!("sz=\"{s}\"")))
                    );
                    let read = call(dir.path(), json!({"op":"read","input":"title.pptx"}))?;
                    assert_eq!(read.data.ok_or("no data")?[0]["title"], title);
                } else {
                    let info = Command::new("pdfinfo")
                        .arg(dir.path().join("title.pdf"))
                        .output()?;
                    assert!(info.status.success());
                    assert!(String::from_utf8(info.stdout)?.lines().any(
                        |l| l.starts_with("Pages:") && l.split_whitespace().last() == Some("1")
                    ));
                }
            }
        }
        for format in ["pptx", "pdf"] {
            let out = call(
                dir.path(),
                json!({"op":"create","theme":theme,"output":format!("markdown.{format}"),"markdown":"# Incident response essentials"}),
            )?;
            assert_eq!(out.summary_params["count"], "1");
        }
    }
    Ok(())
}

#[test]
fn grouped_text_tables_and_images_are_read_once_in_order_and_exported() -> Result {
    let dir = tempfile::tempdir()?;
    png(&dir.path().join("image.png"))?;
    call(
        dir.path(),
        json!({"op":"create","output":"grouped.pptx","slides":[{"title":"Grouped title","text":"Before table","table":{"headers":["Header"],"rows":[["Cell"]]},"image":"image.png"}]}),
    )?;
    let mut p = parts(&dir.path().join("grouped.pptx"))?;
    let name = "ppt/slides/slide1.xml";
    let mut slide = xml(&p, name)?;
    let start = slide.find("<p:sp>").ok_or("no shape")?;
    slide.insert_str(start,"<p:grpSp><p:nvGrpSpPr><p:cNvPr id=\"900\" name=\"Group\"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/><p:grpSp><p:nvGrpSpPr><p:cNvPr id=\"901\" name=\"Nested group\"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>");
    slide = slide.replace("</p:spTree>", "</p:grpSp></p:grpSp></p:spTree>");
    p.insert(name.into(), slide.into_bytes());
    pack(&dir.path().join("grouped.pptx"), &p)?;
    let data = call(dir.path(), json!({"op":"read","input":"grouped.pptx"}))?
        .data
        .ok_or("no data")?;
    assert_eq!(data[0]["title"], "Grouped title");
    let text = data[0]["text"].as_str().ok_or("no text")?;
    assert_eq!(text.matches("Before table").count(), 1);
    assert_eq!(text.matches("Header\tCell").count(), 0);
    assert!(text.find("Before table") < text.find("Header\nCell"));
    assert!(text.contains("ppt/media/image1.png"));
    call(
        dir.path(),
        json!({"op":"convert","input":"grouped.pptx","output":"grouped.md"}),
    )?;
    let md = std::fs::read_to_string(dir.path().join("grouped.md"))?;
    for token in [
        "Grouped title",
        "Before table",
        "Header",
        "Cell",
        "image1.png",
    ] {
        assert_eq!(md.matches(token).count(), 1);
    }
    Ok(())
}

#[test]
fn worksheet_identity_preserves_spaces_and_punctuation() -> Result {
    let dir = tempfile::tempdir()?;
    let mut p = BTreeMap::new();
    p.insert("xl/workbook.xml".into(), br#"<workbook xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="D CF" r:id="r1"/><sheet name="D-CF" r:id="r2"/></sheets><definedNames><definedName name="Exact">'D CF'!$B$1</definedName></definedNames></workbook>"#.to_vec());
    p.insert("xl/_rels/workbook.xml.rels".into(), br#"<Relationships><Relationship Id="r1" Type="x/worksheet" Target="worksheets/one.xml"/><Relationship Id="r2" Type="x/worksheet" Target="worksheets/two.xml"/></Relationships>"#.to_vec());
    for (name, value) in [("one", 10), ("two", 20)] {
        p.insert(format!("xl/worksheets/{name}.xml"),format!("<worksheet><sheetData><row><c r=\"B1\"><v>{value}</v></c></row></sheetData></worksheet>").into_bytes());
    }
    pack(&dir.path().join("model.xlsx"), &p)?;
    call(
        dir.path(),
        json!({"op":"create","output":"qc.pptx","slides":[
            {"title":"Missing","text":"Value: 10","sources":["Source: model.xlsx, DCF!B1"]},
            {"title":"Spaced","text":"Value: 10","sources":["Source: model.xlsx, 'D CF'!B1"]},
            {"title":"Punctuation","text":"Value: 20","sources":["Source: model.xlsx, D-CF!B1"]},
            {"title":"Named","text":"Value: 10","sources":["Source: model.xlsx, Exact"]}
        ]}),
    )?;
    let data = call(
        dir.path(),
        json!({"op":"check","input":"qc.pptx","model":"model.xlsx"}),
    )?
    .data
    .ok_or("no findings")?;
    let findings = data.as_array().ok_or("not array")?;
    assert!(
        findings
            .iter()
            .any(|f| f["slide"] == 1 && f["issue"] == "check.source_unresolved")
    );
    assert!(!findings.iter().any(|f| f["slide"] != 1
        && matches!(
            f["issue"].as_str(),
            Some("check.source_unresolved" | "check.model_mismatch")
        )));
    Ok(())
}

#[test]
fn real_cli_errors_are_bounded_json_with_stable_codes() -> Result {
    let dir = tempfile::tempdir()?;
    for (request, code) in [
        ("{".to_string(), "cli.invalid_json"),
        (
            json!({"op":"read","input":"a.pptx","unknown":1}).to_string(),
            "cli.invalid_json",
        ),
        (
            json!({"op":"read","input":"/forbidden.pptx"}).to_string(),
            "path.unsafe",
        ),
        (
            json!({"op":"read","input":"../forbidden.pptx"}).to_string(),
            "path.unsafe",
        ),
        (
            json!({"op":"create","output":"out.pptx","markdown":"# A","theme":"x".repeat(10000)})
                .to_string(),
            "input.unknown_theme",
        ),
        (
            json!({"op":"convert","input":"x.md","output":"x.pdf","template":"x.pptx"}).to_string(),
            "template.output",
        ),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_slides"))
            .current_dir(dir.path())
            .arg(request)
            .output()?;
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stderr.is_empty());
        assert!(output.stdout.len() < 2500);
        let value: Value = serde_json::from_slice(&output.stdout)?;
        assert_eq!(value["ok"], false);
        assert!(value.get("error").is_none());
        assert_eq!(value["summary_code"], code);
        assert!(value["summary_params"].is_object());
        assert_eq!(value["warnings"], json!([]));
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        let output = Command::new(env!("CARGO_BIN_EXE_slides"))
            .arg(std::ffi::OsString::from_vec(vec![255]))
            .output()?;
        assert_eq!(output.status.code(), Some(1));
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stdout)?["summary_code"],
            "cli.argument_encoding"
        );
    }
    Ok(())
}

#[test]
fn stdin_is_bounded_before_json_parsing() -> Result {
    let dir = tempfile::tempdir()?;
    let mut child = Command::new(env!("CARGO_BIN_EXE_slides"))
        .current_dir(dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    let mut stdin = child.stdin.take().ok_or("no stdin")?;
    let chunk = vec![b' '; 1024 * 1024];
    for _ in 0..100 {
        stdin.write_all(&chunk)?;
    }
    stdin.write_all(b"x")?;
    // Keep stdin open: a bounded reader must reject without waiting for EOF.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while child.try_wait()?.is_none() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let exited = child.try_wait()?.is_some();
    if !exited {
        child.kill()?;
    }
    drop(stdin);
    let output = child.wait_with_output()?;
    assert!(exited, "stdin reader waited for EOF beyond the byte budget");
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout)?["summary_code"],
        "input.too_large"
    );
    Ok(())
}

#[test]
fn read_reports_truncation_and_supports_representation_and_pagination() -> Result {
    let dir = tempfile::tempdir()?;
    call(
        dir.path(),
        json!({"op":"create","output":"read.pptx","slides":[{"title":"First","text":"Alpha beta gamma"},{"title":"Second","text":"Delta epsilon"}]}),
    )?;
    for n in [0, 30] {
        let result = call(
            dir.path(),
            json!({"op":"read","input":"read.pptx","max_chars":n}),
        )?;
        assert!(result.text.ok_or("no text")?.chars().count() <= n);
        assert_eq!(result.data, Some(json!([])));
        assert!(result.warnings.iter().any(|w| w.code == "read.truncated"));
        assert_eq!(result.truncation.ok_or("no metadata")?["data"], true);
    }
    let result = call(
        dir.path(),
        json!({"op":"read","input":"read.pptx","representation":"data","offset":1,"limit":1}),
    )?;
    assert!(result.text.is_none());
    assert_eq!(result.data.ok_or("no data")?[0]["index"], 1);
    let result = call(
        dir.path(),
        json!({"op":"read","input":"read.pptx","representation":"text"}),
    )?;
    assert!(result.data.is_none());
    assert!(result.text.ok_or("no text")?.contains("Second"));
    Ok(())
}

#[cfg(unix)]
#[test]
fn filesystem_rejects_internal_and_dangling_symlinks_and_special_files() -> Result {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir()?;
    call(
        dir.path(),
        json!({"op":"create","output":"real.pptx","markdown":"# File"}),
    )?;
    symlink("real.pptx", dir.path().join("link.pptx"))?;
    symlink("absent.pptx", dir.path().join("dangling.pptx"))?;
    std::fs::create_dir(dir.path().join("sub"))?;
    symlink("sub", dir.path().join("parent"))?;
    for path in ["link.pptx", "dangling.pptx", "parent/new.pptx"] {
        let input: Input =
            serde_json::from_value(json!({"op":"create","output":path,"markdown":"# File"}))?;
        assert_eq!(
            run(&input, &Options::new(dir.path()))
                .err()
                .ok_or("accepted symlink")?
                .code(),
            "path.symlink"
        );
    }
    assert!(
        Command::new("mkfifo")
            .arg(dir.path().join("pipe.pptx"))
            .status()?
            .success()
    );
    for path in ["pipe.pptx", "sub"] {
        let input: Input = serde_json::from_value(json!({"op":"read","input":path}))?;
        assert_eq!(
            run(&input, &Options::new(dir.path()))
                .err()
                .ok_or("accepted special file")?
                .code(),
            "file.not_regular"
        );
    }
    // Hard links are read fully before atomically replacing the output directory entry.
    std::fs::hard_link(dir.path().join("real.pptx"), dir.path().join("alias.pptx"))?;
    let edited = call(
        dir.path(),
        json!({"op":"edit","input":"real.pptx","output":"alias.pptx","replace":{"find":"File","replace":"Updated"}}),
    )?;
    assert_eq!(edited.summary_params["in_place"], "true");
    assert_eq!(
        call(dir.path(), json!({"op":"read","input":"real.pptx"}))?
            .data
            .ok_or("no data")?[0]["title"],
        "File"
    );
    assert_eq!(
        call(dir.path(), json!({"op":"read","input":"alias.pptx"}))?
            .data
            .ok_or("no data")?[0]["title"],
        "Updated"
    );
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir(dir.path().join("readonly"))?;
    let old = std::fs::read(dir.path().join("real.pptx"))?;
    std::fs::write(dir.path().join("readonly/old.pptx"), &old)?;
    std::fs::set_permissions(
        dir.path().join("readonly"),
        std::fs::Permissions::from_mode(0o555),
    )?;
    let result = call(
        dir.path(),
        json!({"op":"edit","input":"real.pptx","output":"readonly/old.pptx","replace":{"find":"File","replace":"Changed"}}),
    );
    std::fs::set_permissions(
        dir.path().join("readonly"),
        std::fs::Permissions::from_mode(0o755),
    )?;
    assert!(
        result.is_err(),
        "writing to a read-only directory succeeded"
    );
    assert_eq!(std::fs::read(dir.path().join("readonly/old.pptx"))?, old);
    Ok(())
}

#[test]
fn render_deadline_terminates_the_renderer_with_a_stable_code() -> Result {
    let dir = tempfile::tempdir()?;
    call(
        dir.path(),
        json!({"op":"create","output":"deck.pptx","markdown":"# Deadline"}),
    )?;
    let input: Input =
        serde_json::from_value(json!({"op":"convert","input":"deck.pptx","output":"deck.pdf"}))?;
    let mut opts = Options::new(dir.path());
    opts.render_timeout = std::time::Duration::ZERO;
    let start = std::time::Instant::now();
    assert_eq!(
        run(&input, &opts)
            .err()
            .ok_or("renderer ignored timeout")?
            .code(),
        "pdf.timeout"
    );
    assert!(start.elapsed() < std::time::Duration::from_secs(5));
    Ok(())
}

fn shape_rect(
    shape: &str,
) -> std::result::Result<(i64, i64, i64, i64), Box<dyn std::error::Error>> {
    let mut r = quick_xml::Reader::from_str(shape);
    let mut values = BTreeMap::new();
    loop {
        match r.read_event()? {
            quick_xml::events::Event::Empty(e) | quick_xml::events::Event::Start(e)
                if matches!(e.name().as_ref(), "a:off" | "a:ext") =>
            {
                for a in e.attributes() {
                    let a = a?;
                    values.insert(a.key.as_ref().to_string(), a.value.parse::<i64>()?);
                }
            }
            quick_xml::events::Event::Eof => break,
            _ => {}
        }
    }
    Ok((
        *values.get("x").ok_or("no x")?,
        *values.get("y").ok_or("no y")?,
        *values.get("cx").ok_or("no cx")?,
        *values.get("cy").ok_or("no cy")?,
    ))
}

#[test]
fn template_image_uses_contain_and_reserves_caption_geometry() -> Result {
    let dir = tempfile::tempdir()?;
    png(&dir.path().join("image.png"))?;
    call(
        dir.path(),
        json!({"op":"create","output":"company.pptx","markdown":"# Company"}),
    )?;
    std::fs::write(
        dir.path().join("image.md"),
        "## Evidence\n<!-- layout: image -->\n![Evidence](image.png)\n\nA caption below the image\n",
    )?;
    let mut company = parts(&dir.path().join("company.pptx"))?;
    let mut extended = false;
    for (name, data) in &mut company {
        if !name.contains("slideLayouts/") || !name.ends_with(".xml") {
            continue;
        }
        let layout = String::from_utf8(data.clone())?;
        if !layout.contains("name=\"image\"") {
            continue;
        }
        let ph = layout
            .find("type=\"body\"")
            .ok_or("no image body placeholder")?;
        let start = layout[..ph].rfind("<p:sp>").ok_or("no body shape")?;
        let end = start + layout[start..].find("</p:sp>").ok_or("no body end")? + 7;
        let extra = layout[start..end]
            .replace("idx=\"1\"", "idx=\"2\"")
            .replace("id=\"3\"", "id=\"99\"");
        *data = layout
            .replace("</p:spTree>", &(extra + "</p:spTree>"))
            .into_bytes();
        extended = true;
    }
    assert!(extended);
    pack(&dir.path().join("two-body-company.pptx"), &company)?;
    let mut invalid_geometry = company.clone();
    for (name, data) in &mut invalid_geometry {
        if name.contains("slideLayouts/") && name.ends_with(".xml") {
            let layout = String::from_utf8(data.clone())?;
            if layout.contains("name=\"image\"") {
                *data = layout
                    .replace("cy=\"4480560\"", "cy=\"9223372036854775807\"")
                    .into_bytes();
            }
        }
    }
    pack(&dir.path().join("invalid-company.pptx"), &invalid_geometry)?;
    let input: Input = serde_json::from_value(
        json!({"op":"convert","input":"image.md","output":"invalid.pptx","template":"invalid-company.pptx"}),
    )?;
    assert_eq!(
        run(&input, &Options::new(dir.path()))
            .err()
            .ok_or("accepted invalid image geometry")?
            .code(),
        "package.invalid"
    );
    for template in [None, Some("company.pptx"), Some("two-body-company.pptx")] {
        let mut request = json!({"op":"convert","input":"image.md","output":"image.pptx"});
        if let Some(template) = template {
            request["template"] = json!(template);
        }
        let result = call(dir.path(), request)?;
        assert_eq!(
            result.summary_params["count"], "1",
            "{template:?}: {:?}",
            result.warnings
        );
        let p = parts(&dir.path().join("image.pptx"))?;
        let part = p
            .keys()
            .find(|name| name.ends_with("/slides/slide1.xml"))
            .ok_or("no slide part")?;
        let slide = xml(&p, part)?;
        let pic_start = slide.find("<p:pic>").ok_or("no picture")?;
        let pic_end = pic_start
            + slide[pic_start..]
                .find("</p:pic>")
                .ok_or("no picture end")?
            + 8;
        let (x, y, w, h) = shape_rect(&slide[pic_start..pic_end])?;
        assert!(
            (w as f64 / h as f64 - 2.0).abs() < 0.001,
            "distorted image with template={template:?}: {w}x{h}, part={part}, xml={}",
            &slide[pic_start..pic_end]
        );
        let caption_end = slide.find("A caption").ok_or("no caption")?;
        let caption_start = slide[..caption_end]
            .rfind("<p:sp>")
            .ok_or("no caption shape")?;
        let caption_end = caption_start
            + slide[caption_start..]
                .find("</p:sp>")
                .ok_or("no caption end")?
            + 7;
        let (_, cy, _, ch) = shape_rect(&slide[caption_start..caption_end])?;
        assert!(
            cy >= y + h,
            "overlapping caption with template={template:?}"
        );
        for shape in slide.split("<p:sp>").skip(1) {
            let shape = shape.split("</p:sp>").next().ok_or("missing shape")?;
            if shape.contains("<a:t>Evidence</a:t>") && !shape.contains("type=\"title\"") {
                let (_, top, _, _) = shape_rect(shape)?;
                assert!(
                    top >= y + h,
                    "mapped alt caption overlaps image: {template:?}"
                );
            }
        }
        assert!(x >= 0 && y >= 0 && cy + ch <= 6858000);
        call(
            dir.path(),
            json!({"op":"convert","input":"image.pptx","output":"image.pdf"}),
        )?;
    }
    Ok(())
}
