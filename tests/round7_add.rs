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
        zip.start_file(name, zip::write::SimpleFileOptions::default())?;
        zip.write_all(bytes)?;
    }
    std::fs::write(path, zip.finish()?.into_inner())?;
    Ok(())
}
fn foreign_deck(dir: &Path, width: &str, height: &str) -> Result {
    let mut p = BTreeMap::new();
    p.insert(
        "mimetype".into(),
        b"application/vnd.oasis.opendocument.presentation".to_vec(),
    );
    p.insert("META-INF/manifest.xml".into(), br#"<manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0"><manifest:file-entry manifest:full-path="/" manifest:media-type="application/vnd.oasis.opendocument.presentation"/><manifest:file-entry manifest:full-path="content.xml" manifest:media-type="text/xml"/><manifest:file-entry manifest:full-path="styles.xml" manifest:media-type="text/xml"/></manifest:manifest>"#.to_vec());
    let ns = r#"xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0" xmlns:presentation="urn:oasis:names:tc:opendocument:xmlns:presentation:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:svg="urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0" xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0" office:version="1.2""#;
    p.insert("styles.xml".into(),format!(r##"<office:document-styles {ns}><office:styles><style:presentation-page-layout style:name="CompanyContent"><presentation:placeholder presentation:object="title" svg:x="1cm" svg:y="1cm" svg:width="23cm" svg:height="2cm"/><presentation:placeholder presentation:object="outline" svg:x="1cm" svg:y="4cm" svg:width="23cm" svg:height="10cm"/></style:presentation-page-layout></office:styles><office:automatic-styles><style:page-layout style:name="Page"><style:page-layout-properties fo:page-width="{width}cm" fo:page-height="{height}cm" style:print-orientation="landscape"/></style:page-layout><style:style style:name="Background" style:family="drawing-page"><style:drawing-page-properties draw:fill="solid" draw:fill-color="#E8F5E9"/></style:style></office:automatic-styles><office:master-styles><style:master-page style:name="Company" style:page-layout-name="Page" draw:style-name="Background"><draw:frame svg:x="1cm" svg:y="14.5cm" svg:width="6cm" svg:height="0.6cm"><draw:text-box><text:p>ACME</text:p></draw:text-box></draw:frame></style:master-page></office:master-styles></office:document-styles>"##).into_bytes());
    p.insert("content.xml".into(),format!(r#"<office:document-content {ns}><office:body><office:presentation><draw:page draw:name="Original" draw:master-page-name="Company" presentation:presentation-page-layout-name="CompanyContent"><draw:frame presentation:class="title" svg:x="1cm" svg:y="1cm" svg:width="23cm" svg:height="2cm"><draw:text-box><text:p>Original slide</text:p></draw:text-box></draw:frame><draw:frame presentation:class="outline" svg:x="1cm" svg:y="4cm" svg:width="23cm" svg:height="10cm"><draw:text-box><text:p>Original content</text:p></draw:text-box></draw:frame></draw:page></office:presentation></office:body></office:document-content>"#).into_bytes());
    pack(&dir.join("foreign.odp"), &p)?;
    let profile = tempfile::tempdir()?;
    let out = Command::new("soffice")
        .arg(format!(
            "-env:UserInstallation=file://{}",
            profile.path().display()
        ))
        .args(["--headless", "--convert-to", "pptx", "--outdir"])
        .arg(dir)
        .arg(dir.join("foreign.odp"))
        .output()?;
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(dir.join("foreign.pptx").is_file());
    // LibreOffice creates standard layout geometries rather than preserving all ODP layout
    // hints. Pin the imported body geometry explicitly to exercise exact placeholder reuse.
    let mut imported = parts(&dir.join("foreign.pptx"))?;
    for (name, bytes) in &mut imported {
        if !name.contains("/slideLayouts/") || !name.ends_with(".xml") {
            continue;
        }
        let layout = String::from_utf8(bytes.clone())?;
        let mut changed = layout.clone();
        for shape in layout.split("<p:sp>").skip(1) {
            let Some(end) = shape.find("</p:sp>") else {
                continue;
            };
            let shape = &shape[..end];
            if !shape.contains("<p:ph")
                || ["title", "ctrTitle", "subTitle", "dt", "ftr", "sldNum"]
                    .iter()
                    .any(|kind| shape.contains(&format!("type=\"{kind}\"")))
            {
                continue;
            }
            let geometry = "<p:spPr><a:xfrm><a:off x=\"360000\" y=\"1440000\"/><a:ext cx=\"8280000\" cy=\"3600000\"/></a:xfrm></p:spPr>";
            let replacement = if let Some(start) = shape.find("<p:spPr>") {
                let finish =
                    start + shape[start..].find("</p:spPr>").ok_or("spPr end")? + "</p:spPr>".len();
                format!("{}{}{}", &shape[..start], geometry, &shape[finish..])
            } else {
                shape.replace("<p:spPr/>", geometry)
            };
            changed = changed.replace(shape, &replacement);
        }
        *bytes = changed.into_bytes();
    }
    pack(&dir.join("foreign.pptx"), &imported)?;
    Ok(())
}

#[test]
fn additions_fit_foreign_widescreen_and_four_by_three_layouts() -> Result {
    for (width, height) in [("28", "15.75"), ("25.4", "19.05")] {
        let dir = tempfile::tempdir()?;
        foreign_deck(dir.path(), width, height)?;
        let before = parts(&dir.path().join("foreign.pptx"))?;
        let added = call(
            dir.path(),
            json!({"op":"edit","input":"foreign.pptx","output":"added.pptx","add":[
                {"title":"Investment plan","table":{"headers":["Metric","North","South","West","RIGHTMOST HEADER"],"rows":[["Revenue","10","20","30","RIGHTMOST VALUE"]]}},
                {"title":"Revenue chart","chart":{"type":"column","labels":["Q1 2027","Q2 2027"],"series":[{"name":"Revenue","values":[10,20]}]},"notes":"Chart note"},
                {"title":"Priorities","bullets":["Protect the customer","Improve reliability","Invest in people"]},
                {"title":"Long agenda","bullets":(0..60).map(|i|format!("Agenda item {i}: review the customer experience and delivery plan")).collect::<Vec<_>>()}
            ]}),
        )?;
        assert!(added.warnings.iter().any(|w| w.code == "slide.split"));
        let after = parts(&dir.path().join("added.pptx"))?;
        for (name, bytes) in &before {
            if !matches!(
                name.as_str(),
                "[Content_Types].xml" | "ppt/presentation.xml" | "ppt/_rels/presentation.xml.rels"
            ) {
                assert_eq!(after.get(name), Some(bytes), "changed {name}");
            }
        }
        let w = (width.parse::<f64>()? * 360000.0) as i64;
        let h = (height.parse::<f64>()? * 360000.0) as i64;
        let mut frames = 0;
        for (name, data) in &after {
            if !name.contains("/slides/") || !name.ends_with(".xml") || before.contains_key(name) {
                continue;
            }
            let slide = std::str::from_utf8(data)?;
            assert!(
                !slide.contains("Theme accent"),
                "default decoration in company slide"
            );
            for frame in slide.split("<p:graphicFrame>").skip(1) {
                let frame = frame.split("</p:graphicFrame>").next().ok_or("frame end")?;
                let mut reader = quick_xml::Reader::from_str(frame);
                let mut geometry = BTreeMap::new();
                loop {
                    match reader.read_event()? {
                        quick_xml::events::Event::Empty(e) | quick_xml::events::Event::Start(e)
                            if matches!(e.name().as_ref(), "a:off" | "a:ext") =>
                        {
                            for attr in e.attributes() {
                                let attr = attr?;
                                // a:ext also represents extension metadata; only geometry
                                // attributes are numeric dimensions.
                                if matches!(attr.key.as_ref(), "x" | "y" | "cx" | "cy") {
                                    geometry.insert(
                                        attr.key.as_ref().to_string(),
                                        attr.value.parse::<i64>()?,
                                    );
                                }
                            }
                        }
                        quick_xml::events::Event::Eof => break,
                        _ => {}
                    }
                }
                let x = *geometry.get("x").ok_or("x")?;
                let y = *geometry.get("y").ok_or("y")?;
                let cw = *geometry.get("cx").ok_or("cx")?;
                let ch = *geometry.get("cy").ok_or("cy")?;
                assert!(
                    x >= 0 && y >= 0 && cw > 0 && ch > 0 && x + cw <= w && y + ch <= h,
                    "{width}x{height}: {x},{y},{cw},{ch}"
                );
                assert_eq!(
                    (x, y, cw, ch),
                    (360000, 1440000, 8280000, 3600000),
                    "selected company body placeholder"
                );
                frames += 1;
            }
        }
        assert_eq!(frames, 2);
        call(
            dir.path(),
            json!({"op":"convert","input":"added.pptx","output":"added.pdf"}),
        )?;
        let text = Command::new("pdftotext")
            .arg(dir.path().join("added.pdf"))
            .arg("-")
            .output()?;
        assert!(text.status.success());
        let text = String::from_utf8(text.stdout)?;
        let compact: String = text.split_whitespace().collect();
        for expected in [
            "RIGHTMOSTHEADER",
            "RIGHTMOSTVALUE",
            "ACME",
            "Priorities",
            "Q12027",
        ] {
            assert!(compact.contains(expected), "missing {expected}: {text}");
        }
        if let Some(destination) = std::env::var_os("SLIDES_TEST_ARTIFACTS") {
            let dest = std::path::PathBuf::from(destination).join(format!("foreign-{width}"));
            std::fs::create_dir_all(&dest)?;
            std::fs::copy(dir.path().join("added.pptx"), dest.join("added.pptx"))?;
            std::fs::copy(dir.path().join("added.pdf"), dest.join("added.pdf"))?;
            let png = Command::new("pdftoppm")
                .args(["-scale-to", "800", "-png"])
                .arg(dest.join("added.pdf"))
                .arg(dest.join("slide"))
                .output()?;
            assert!(png.status.success());
        }
    }
    Ok(())
}
