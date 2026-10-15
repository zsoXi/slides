//! Design, media, native chart and message-catalog regression tests.

use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::path::Path;

use serde_json::{Value, json};
use slides::{Input, Options, Output, run};

type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

fn call(base: &Path, request: Value) -> Result<Output> {
    Ok(run(
        &serde_json::from_value::<Input>(request)?,
        &Options::new(base),
    )?)
}

fn parts(bytes: &[u8]) -> Result<HashMap<String, Vec<u8>>> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes))?;
    let mut out = HashMap::new();
    for i in 0..zip.len() {
        let mut file = zip.by_index(i)?;
        let name = file.name()?.to_string();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        out.insert(name, bytes);
    }
    Ok(out)
}

fn text<'a>(parts: &'a HashMap<String, Vec<u8>>, part: &str) -> Result<&'a str> {
    Ok(std::str::from_utf8(parts.get(part).ok_or("missing part")?)?)
}

fn parse(bytes: &[u8]) -> Result {
    let mut reader = quick_xml::Reader::from_reader(bytes);
    loop {
        if matches!(reader.read_event()?, quick_xml::events::Event::Eof) {
            break;
        }
    }
    Ok(())
}

fn png(path: &Path, width: usize, height: usize) -> Result {
    let image = printpdf::RawImage {
        pixels: printpdf::RawImageData::U8(vec![80; width * height * 3]),
        width,
        height,
        data_format: printpdf::RawImageFormat::RGB8,
        tag: Vec::new(),
    };
    let data = image.encode_to_bytes(&[printpdf::OutputImageFormat::Png])?;
    std::fs::write(path, data.0)?;
    Ok(())
}

#[test]
fn title_uses_real_layout_and_large_centered_type() -> Result {
    let dir = tempfile::tempdir()?;
    call(
        dir.path(),
        json!({"op":"create", "output":"title.pptx", "markdown":"# Strategy\nA focused plan"}),
    )?;
    let p = parts(&std::fs::read(dir.path().join("title.pptx"))?)?;
    assert!(text(&p, "ppt/slideLayouts/slideLayout1.xml")?.contains("type=\"title\""));
    let slide = text(&p, "ppt/slides/slide1.xml")?;
    assert!(slide.contains("type=\"ctrTitle\""));
    assert!(slide.contains("sz=\"4400\""));
    assert!(slide.contains("algn=\"ctr\""));
    assert!(slide.contains("A focused plan"));
    assert!(text(&p, "ppt/slides/_rels/slide1.xml.rels")?.contains("slideLayout1.xml"));
    Ok(())
}

#[test]
fn themes_have_distinct_backgrounds_accents_and_content_grid() -> Result {
    let dir = tempfile::tempdir()?;
    let mut themes = Vec::new();
    let mut slides = Vec::new();
    for theme in ["light", "dark", "corporate"] {
        call(
            dir.path(),
            json!({"op":"create", "output":"deck.pptx", "theme":theme, "slides":[{"title":"Plan", "bullets":["One", "Two"]}]}),
        )?;
        let p = parts(&std::fs::read(dir.path().join("deck.pptx"))?)?;
        themes.push(text(&p, "ppt/theme/theme1.xml")?.to_string());
        let slide = text(&p, "ppt/slides/slide1.xml")?;
        for required in [
            "<p:bg>",
            "Theme accent",
            "type=\"sldNum\"",
            "sz=\"3400\"",
            "sz=\"2400\"",
            "anchor=\"ctr\"",
            "a:lnSpc",
            "a:spcAft",
        ] {
            assert!(slide.contains(required), "{required}");
        }
        slides.push(slide.to_string());
    }
    for i in 0..3 {
        for j in i + 1..3 {
            assert_ne!(themes[i], themes[j]);
            assert_ne!(slides[i], slides[j]);
        }
    }
    assert!(slides[0].contains("FFFFFF"));
    assert!(slides[1].contains("141414"));
    assert!(slides[2].contains("1F3864"));
    Ok(())
}

#[test]
fn png_embeds_as_pdf_image_and_pptx_media_with_contain_scaling() -> Result {
    let dir = tempfile::tempdir()?;
    png(&dir.path().join("image.png"), 120, 60)?;
    let slides =
        json!([{"layout":"image", "title":"A perspective", "image":"image.png", "text":"Caption"}]);
    call(
        dir.path(),
        json!({"op":"create", "output":"image.pptx", "slides":slides}),
    )?;
    let p = parts(&std::fs::read(dir.path().join("image.pptx"))?)?;
    assert!(p.contains_key("ppt/media/image1.png"));
    assert_eq!(
        p["ppt/media/image1.png"],
        std::fs::read(dir.path().join("image.png"))?
    );
    let slide = text(&p, "ppt/slides/slide1.xml")?;
    let pic = slide
        .split("<p:pic>")
        .nth(1)
        .ok_or("no picture")?
        .split("</p:pic>")
        .next()
        .ok_or("no picture end")?;
    let mut reader = quick_xml::Reader::from_str(pic);
    let mut dimensions = None;
    loop {
        match reader.read_event()? {
            quick_xml::events::Event::Empty(e) if e.name().as_ref() == "a:ext" => {
                let mut w = 0.0;
                let mut h = 0.0;
                for a in e.attributes() {
                    let a = a?;
                    if a.key.as_ref() == "cx" {
                        w = a
                            .normalized_value(quick_xml::XmlVersion::default())?
                            .parse::<f64>()?;
                    }
                    if a.key.as_ref() == "cy" {
                        h = a
                            .normalized_value(quick_xml::XmlVersion::default())?
                            .parse::<f64>()?;
                    }
                }
                dimensions = Some((w, h));
            }
            quick_xml::events::Event::Eof => break,
            _ => {}
        }
    }
    let (w, h) = dimensions.ok_or("no dimensions")?;
    assert!((w / h - 2.0).abs() < 0.001);
    assert!(w <= 11_003_280.0 && h <= 4_480_560.0);
    let result = call(
        dir.path(),
        json!({"op":"create", "output":"image.pdf", "slides":slides}),
    )?;
    assert!(result.warnings.is_empty());
    let pdf = std::fs::read(dir.path().join("image.pdf"))?;
    let pdf = String::from_utf8_lossy(&pdf);
    assert!(pdf.contains("/Subtype/Image") || pdf.contains("/Subtype /Image"));
    Ok(())
}

#[test]
fn jpeg_and_file_conversions_work_end_to_end() -> Result {
    let dir = tempfile::tempdir()?;
    let image = printpdf::RawImage {
        pixels: printpdf::RawImageData::U8(vec![120; 60 * 120 * 3]),
        width: 60,
        height: 120,
        data_format: printpdf::RawImageFormat::RGB8,
        tag: Vec::new(),
    };
    let (bytes, _) = image.encode_to_bytes(&[printpdf::OutputImageFormat::Jpeg])?;
    std::fs::write(dir.path().join("portrait.jpg"), bytes)?;
    std::fs::write(
        dir.path().join("source.md"),
        "# Overview\nA focused plan\n\n## Perspective\n<!-- layout: image -->\n![](portrait.jpg)\n\nCaption\n",
    )?;
    for output in ["converted.pptx", "converted.pdf"] {
        call(
            dir.path(),
            json!({"op":"convert", "input":"source.md", "output":output}),
        )?;
    }
    let p = parts(&std::fs::read(dir.path().join("converted.pptx"))?)?;
    assert!(p.contains_key("ppt/media/image1.jpeg"));
    let pdf = std::fs::read(dir.path().join("converted.pdf"))?;
    let pdf = String::from_utf8_lossy(&pdf);
    assert!(pdf.contains("/Subtype/Image") || pdf.contains("/Subtype /Image"));
    call(
        dir.path(),
        json!({"op":"convert", "input":"converted.pptx", "output":"export.md"}),
    )?;
    let markdown = std::fs::read_to_string(dir.path().join("export.md"))?;
    assert!(markdown.contains("A focused plan"));
    assert!(markdown.contains("Caption"));
    Ok(())
}

#[test]
fn charts_are_native_parseable_and_have_matching_embedded_workbooks() -> Result {
    let dir = tempfile::tempdir()?;
    for kind in ["bar", "line", "pie"] {
        let slides = json!([{"title":"Results", "chart":{"type":kind, "labels":["North", "South"], "series":[{"name":"Revenue", "values":[24, 63]}]}}]);
        call(
            dir.path(),
            json!({"op":"create", "output":"chart.pptx", "slides":slides}),
        )?;
        let p = parts(&std::fs::read(dir.path().join("chart.pptx"))?)?;
        let chart = p.get("ppt/charts/chart1.xml").ok_or("no chart")?;
        parse(chart)?;
        let chart = std::str::from_utf8(chart)?;
        assert!(chart.contains(&format!("<c:{kind}Chart>")));
        for token in [
            "North",
            "South",
            "Revenue",
            ">24<",
            ">63<",
            "c:externalData",
            "Sheet1!$B$2:$B$3",
        ] {
            assert!(chart.contains(token), "{token}");
        }
        let workbook = parts(p.get("ppt/embeddings/chart1.xlsx").ok_or("no workbook")?)?;
        for bytes in workbook.values() {
            parse(bytes)?;
        }
        let worksheet = text(&workbook, "xl/worksheets/sheet1.xml")?;
        for token in ["North", "South", "Revenue", ">24<", ">63<"] {
            assert!(worksheet.contains(token));
        }
        assert!(text(&p, "[Content_Types].xml")?.contains("drawingml.chart+xml"));
        assert!(
            text(&p, "ppt/charts/_rels/chart1.xml.rels")?.contains("../embeddings/chart1.xlsx")
        );
        let read = call(dir.path(), json!({"op":"read", "input":"chart.pptx"}))?;
        assert!(read.text.ok_or("missing text")?.contains("63"));
        call(
            dir.path(),
            json!({"op":"create", "output":"chart.pdf", "slides":slides}),
        )?;
        // QDF exposes the vector content stream rather than only checking for a PDF header.
        let result = std::process::Command::new("qpdf")
            .args([
                "--qdf",
                "--object-streams=disable",
                "--stream-data=uncompress",
            ])
            .arg(dir.path().join("chart.pdf"))
            .arg(dir.path().join("chart-qdf.pdf"))
            .output()?;
        assert!(result.status.success());
        let pdf = std::fs::read(dir.path().join("chart-qdf.pdf"))?;
        let pdf = String::from_utf8_lossy(&pdf);
        assert!(pdf.contains("stream"));
        assert!(
            pdf.contains(" l") || pdf.contains(" re") || pdf.contains(" c"),
            "no vector marks"
        );
        if kind == "bar" {
            let result = std::process::Command::new("pdftoppm")
                .args(["-singlefile", "-r", "72", "-png"])
                .arg(dir.path().join("chart.pdf"))
                .arg(dir.path().join("bars"))
                .output()?;
            assert!(result.status.success());
            let image = printpdf::RawImage::decode_from_bytes(
                &std::fs::read(dir.path().join("bars.png"))?,
                &mut Vec::new(),
            )?;
            let printpdf::RawImageData::U8(pixels) = image.pixels else {
                return Err("unexpected raster type".into());
            };
            let channels = pixels.len() / (image.width * image.height);
            let mut colored = 0;
            for y in 150..400 {
                for x in 100..850 {
                    let i = (y * image.width + x) * channels;
                    if pixels[i].abs_diff(47) < 5
                        && pixels[i + 1].abs_diff(85) < 5
                        && pixels[i + 2].abs_diff(151) < 5
                    {
                        colored += 1;
                    }
                }
            }
            assert!(
                colored > 10_000,
                "missing filled bars in plot area: {colored}"
            );
        }
    }
    Ok(())
}

#[test]
fn markdown_rules_and_layout_directives_select_real_layouts() -> Result {
    let dir = tempfile::tempdir()?;
    let md = "# Opening\nSubtitle\n\n---\n\n## Options\n<!-- layout: two_content -->\n- Alpha\n- Beta\n- Gamma\n- Delta\n\n---\n\n## Principle\n<!-- layout: quote -->\n> Less, but better.\n\n---\n\n## Act\n<!-- layout: section -->\n";
    call(
        dir.path(),
        json!({"op":"create", "output":"md.pptx", "markdown":md}),
    )?;
    let out = call(dir.path(), json!({"op":"read", "input":"md.pptx"}))?;
    let data = out.data.ok_or("missing slides")?;
    let slides = data.as_array().ok_or("not an array")?;
    assert_eq!(slides.len(), 4);
    for (i, layout) in ["title", "two_content", "quote", "section"]
        .iter()
        .enumerate()
    {
        assert_eq!(slides[i]["layout"], *layout);
    }
    let p = parts(&std::fs::read(dir.path().join("md.pptx"))?)?;
    let two = text(&p, "ppt/slides/slide2.xml")?;
    for word in ["Alpha", "Beta", "Gamma", "Delta"] {
        assert!(two.contains(word));
    }
    assert!(text(&p, "ppt/slides/slide3.xml")?.contains("Less, but better."));
    call(
        dir.path(),
        json!({"op":"convert", "input":"md.pptx", "output":"md.pdf"}),
    )?;
    Ok(())
}

#[test]
fn messages_are_structured_and_locale_falls_back_to_english() -> Result {
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("broken.png"), b"\x89PNG\r\n\x1a\ninvalid")?;
    let input = serde_json::from_value::<Input>(
        json!({"op":"create", "output":"warnings.pptx", "slides":[{"image":"broken.png"}]}),
    )?;
    let mut opts = Options::new(dir.path());
    opts.locale = "not-a-locale".into();
    let result = run(&input, &opts)?;
    let value = serde_json::to_value(&result)?;
    assert_eq!(value["summary_code"], "create.done");
    assert_eq!(value["summary_params"]["count"], "1");
    assert_eq!(value["warnings"][0]["code"], "image.not_embedded");
    assert_eq!(value["warnings"][0]["params"]["slide"], "1");
    assert!(!result.warnings[0].message.contains('{'));
    let bad = serde_json::from_value::<Input>(json!({"op":"read", "input":"missing.pptx"}))?;
    let error = run(&bad, &opts).err().ok_or("expected error")?;
    assert_eq!(error.code(), "file.read");
    assert_eq!(error.params()["path"], "\"missing.pptx\"");
    assert!(std::error::Error::source(&error).is_some());
    Ok(())
}

#[test]
fn decoding_errors_preserve_their_original_causes() -> Result {
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("invalid.md"), [0xff, 0xfe])?;
    std::fs::write(dir.path().join("invalid.pptx"), b"not a ZIP package")?;
    for (request, code) in [
        (
            json!({"op":"convert", "input":"invalid.md", "output":"out.pptx"}),
            "input.markdown_not_utf8",
        ),
        (json!({"op":"read", "input":"invalid.pptx"}), "zip.invalid"),
    ] {
        let input = serde_json::from_value::<Input>(request)?;
        let error = run(&input, &Options::new(dir.path()))
            .err()
            .ok_or("expected decoding error")?;
        assert_eq!(error.code(), code);
        assert!(std::error::Error::source(&error).is_some());
    }
    Ok(())
}

#[test]
fn invalid_chart_contracts_are_rejected() -> Result {
    let dir = tempfile::tempdir()?;
    for chart in [
        json!({"type":"bar", "labels":[], "series":[]}),
        json!({"type":"bar", "labels":["a", "b"], "series":[[1]]}),
        json!({"type":"pie", "labels":["a"], "series":[[-1]]}),
        json!({"type":"pie", "labels":["a"], "series":[[0]]}),
        json!({"type":"pie", "labels":["a"], "series":[[1],[2]]}),
        json!({"type":"other", "labels":["a"], "series":[[1]]}),
    ] {
        assert!(
            call(
                dir.path(),
                json!({"op":"create", "output":"bad.pptx", "slides":[{"chart":chart}]})
            )
            .is_err()
        );
    }
    let parsed = serde_json::from_value::<Input>(
        json!({"op":"create", "output":"bad.pptx", "slides":[{"chart":{"type":"bar", "labels":["a"], "series":[{"name":"a", "values":[1], "extra":true}]}}]}),
    );
    assert!(parsed.is_err());
    Ok(())
}

#[test]
fn resolver_rejects_parent_and_absolute_paths_without_io() -> Result {
    let dir = tempfile::tempdir()?;
    assert!(slides::resolve_path(dir.path(), "../escape").is_err());
    assert!(slides::resolve_path(dir.path(), "/absolute").is_err());
    Ok(())
}

#[test]
fn resolver_rejects_absolute_paths_without_io() -> Result {
    let dir = tempfile::tempdir()?;
    assert!(slides::resolve_path(dir.path(), "/absolute").is_err());
    Ok(())
}

#[test]
fn corrupt_images_are_reported_instead_of_embedded() -> Result {
    let dir = tempfile::tempdir()?;
    let mut bytes = vec![0; 24];
    bytes[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
    bytes[16..20].copy_from_slice(&120u32.to_be_bytes());
    bytes[20..24].copy_from_slice(&60u32.to_be_bytes());
    std::fs::write(dir.path().join("broken.png"), bytes)?;
    let output = call(
        dir.path(),
        json!({"op":"create", "output":"broken.pptx", "slides":[{"image":"broken.png"}]}),
    )?;
    assert_eq!(output.warnings.len(), 1);
    assert_eq!(output.warnings[0].code, "image.not_embedded");
    for placeholder in ["{slide}", "{path}", "{detail}"] {
        assert!(!output.warnings[0].message.contains(placeholder));
    }
    let p = parts(&std::fs::read(dir.path().join("broken.pptx"))?)?;
    assert!(!p.keys().any(|k| k.starts_with("ppt/media/")));
    Ok(())
}

#[test]
fn asset_paths_and_size_limits_are_enforced() -> Result {
    let dir = tempfile::tempdir()?;
    for image in ["../outside.png", "/tmp/outside.png"] {
        assert!(
            call(
                dir.path(),
                json!({"op":"create", "output":"bad.pptx", "slides":[{"image":image}]})
            )
            .is_err()
        );
    }
    png(&dir.path().join("large.png"), 20, 10)?;
    let input = serde_json::from_value::<Input>(
        json!({"op":"create", "output":"bad.pptx", "slides":[{"image":"large.png"}]}),
    )?;
    let mut opts = Options::new(dir.path());
    opts.max_input_bytes = 1;
    assert!(matches!(
        run(&input, &opts),
        Err(slides::Error::TooLarge { .. })
    ));
    Ok(())
}

#[cfg(unix)]
#[test]
fn symlinks_cannot_escape_the_base_directory() -> Result {
    let dir = tempfile::tempdir()?;
    let outside = tempfile::tempdir()?;
    std::os::unix::fs::symlink(outside.path(), dir.path().join("escape"))?;
    assert!(slides::resolve_path(dir.path(), "escape/bad.pptx").is_err());
    assert!(!outside.path().join("bad.pptx").exists());
    std::os::unix::fs::symlink(outside.path().join("missing"), dir.path().join("dangling"))?;
    assert!(
        call(
            dir.path(),
            json!({"op":"create", "output":"dangling", "markdown":"# Test"})
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn repeated_edits_keep_existing_image_and_chart_parts() -> Result {
    let dir = tempfile::tempdir()?;
    png(&dir.path().join("first.png"), 20, 10)?;
    png(&dir.path().join("second.png"), 30, 20)?;
    call(
        dir.path(),
        json!({"op":"create", "output":"edit.pptx", "slides":[{"image":"first.png"}]}),
    )?;
    let chart = json!({"type":"bar", "labels":["A"], "series":[[10]]});
    for _ in 0..2 {
        call(
            dir.path(),
            json!({"op":"edit", "input":"edit.pptx", "output":"edit.pptx", "add":[{"image":"second.png"},{"chart":chart}]}),
        )?;
    }
    let p = parts(&std::fs::read(dir.path().join("edit.pptx"))?)?;
    assert_eq!(
        p["ppt/media/image1.png"],
        std::fs::read(dir.path().join("first.png"))?
    );
    assert!(p.contains_key("ppt/media/image3.png"));
    assert!(p.contains_key("ppt/charts/chart2.xml"));
    let output = call(dir.path(), json!({"op":"read", "input":"edit.pptx"}))?;
    assert_eq!(
        output
            .data
            .ok_or("no data")?
            .as_array()
            .ok_or("not array")?
            .len(),
        5
    );
    Ok(())
}

#[test]
fn table_banding_and_dense_text_use_the_design_grid() -> Result {
    let dir = tempfile::tempdir()?;
    let output = call(
        dir.path(),
        json!({"op":"create", "output":"design.pptx", "theme":"corporate", "slides":[
            {"title":"Budget", "table":{"headers":["Team", "Budget"], "rows":[["Product", "120"], ["Growth", "85"]]}},
            {"title":"Dense", "bullets":["word ".repeat(4000)]}
        ]}),
    )?;
    let p = parts(&std::fs::read(dir.path().join("design.pptx"))?)?;
    let table = text(&p, "ppt/slides/slide1.xml")?;
    assert!(table.contains("val=\"accent1\""));
    assert!(table.contains("val=\"lt2\""));
    assert!(table.contains("val=\"bg1\""));
    assert!(table.contains("sz=\"2000\""));
    assert!(text(&p, "ppt/slides/slide2.xml")?.contains("sz=\"1400\""));
    assert!(
        output
            .warnings
            .iter()
            .any(|w| w.code == "slide.text_overflow" && w.params["slide"] == "2")
    );
    Ok(())
}

#[test]
fn custom_pdf_font_is_used_and_polish_text_is_searchable() -> Result {
    let dir = tempfile::tempdir()?;
    let font = [
        "/usr/share/fonts/noto/NotoSans-Regular.ttf",
        "/usr/share/fonts/TTF/DejaVuSans.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    ]
    .iter()
    .find(|p| Path::new(p).is_file())
    .ok_or("missing test font")?;
    let input = serde_json::from_value::<Input>(
        json!({"op":"create", "output":"font.pdf", "theme":"corporate", "markdown":"# Zażółć gęślą jaźń"}),
    )?;
    let mut opts = Options::new(dir.path());
    opts.font_path = Some(font.into());
    run(&input, &opts)?;
    let pdf = std::fs::read(dir.path().join("font.pdf"))?;
    let pdf = String::from_utf8_lossy(&pdf);
    assert!(pdf.contains("NotoSans") || pdf.contains("DejaVuSans"));
    let result = std::process::Command::new("pdftotext")
        .arg(dir.path().join("font.pdf"))
        .arg("-")
        .output()?;
    assert!(result.status.success());
    assert!(String::from_utf8(result.stdout)?.contains("Zażółć gęślą jaźń"));
    call(
        dir.path(),
        json!({"op":"create", "output":"font.pptx", "markdown":"# Zażółć gęślą jaźń"}),
    )?;
    let convert = serde_json::from_value::<Input>(
        json!({"op":"convert", "input":"font.pptx", "output":"font-converted.pdf"}),
    )?;
    run(&convert, &opts)?;
    let converted = std::fs::read(dir.path().join("font-converted.pdf"))?;
    let converted = String::from_utf8_lossy(&converted);
    assert!(converted.contains("NotoSans") || converted.contains("DejaVuSans"));
    opts.font_path = Some(dir.path().join("absent.ttf"));
    let error = run(&input, &opts).err().ok_or("expected font read error")?;
    assert_eq!(error.code(), "pdf.font_read");
    assert!(std::error::Error::source(&error).is_some());
    Ok(())
}

#[test]
fn parallel_pdf_exports_use_independent_profiles() -> Result {
    let dir = tempfile::tempdir()?;
    let results = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..2).map(|i| {
            let path = dir.path();
            scope.spawn(move || call(path, json!({"op":"create", "output":format!("{i}.pdf"), "markdown":format!("# Presentation {i}")})).map(|_| ()).map_err(|e| e.to_string()))
        }).collect();
        handles.into_iter().map(|h| h.join()).collect::<Vec<_>>()
    });
    for result in results {
        assert!(matches!(result, Ok(Ok(()))), "{result:?}");
    }
    Ok(())
}
