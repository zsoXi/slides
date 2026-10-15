//! Company templates and financial QC through the public API; fixtures are generated locally.
use serde_json::{Value, json};
use slides::{Input, Options, Output, run};
use std::collections::BTreeMap;
use std::io::{Cursor, Read, Write};
use std::path::Path;
use std::process::Command;
type TestResult = Result<(), Box<dyn std::error::Error>>;
fn call(dir: &Path, value: Value) -> Result<Output, Box<dyn std::error::Error>> {
    Ok(run(
        &serde_json::from_value::<Input>(value)?,
        &Options::new(dir),
    )?)
}
fn pack(parts: &BTreeMap<String, Vec<u8>>) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, data) in parts {
        zip.start_file(name, zip::write::SimpleFileOptions::default())?;
        zip.write_all(data)?;
    }
    Ok(zip.finish()?.into_inner())
}
fn unpack(bytes: &[u8]) -> Result<BTreeMap<String, Vec<u8>>, Box<dyn std::error::Error>> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes))?;
    let mut out = BTreeMap::new();
    for i in 0..zip.len() {
        let mut f = zip.by_index(i)?;
        let mut bytes = Vec::new();
        f.read_to_end(&mut bytes)?;
        out.insert(f.name()?.into(), bytes);
    }
    Ok(out)
}
fn lo_template(dir: &Path) -> TestResult {
    let mut parts = BTreeMap::new();
    parts.insert(
        "mimetype".into(),
        b"application/vnd.oasis.opendocument.presentation".to_vec(),
    );
    parts.insert("META-INF/manifest.xml".into(),br#"<?xml version="1.0"?><manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0" manifest:version="1.2"><manifest:file-entry manifest:full-path="/" manifest:media-type="application/vnd.oasis.opendocument.presentation"/><manifest:file-entry manifest:full-path="content.xml" manifest:media-type="text/xml"/><manifest:file-entry manifest:full-path="styles.xml" manifest:media-type="text/xml"/></manifest:manifest>"#.to_vec());
    let ns = r#"xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0" xmlns:presentation="urn:oasis:names:tc:opendocument:xmlns:presentation:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:svg="urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0" xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0" office:version="1.2""#;
    let styles = format!(
        r##"<?xml version="1.0"?><office:document-styles {ns}><office:styles><style:default-style style:family="paragraph"><style:text-properties style:font-name="Liberation Serif" fo:font-family="Liberation Serif" fo:color="#512680"/></style:default-style><style:style style:name="Logo" style:family="graphic"><style:graphic-properties draw:fill="solid" draw:fill-color="#512680"/></style:style></office:styles><office:automatic-styles><style:page-layout style:name="AcmePage"><style:page-layout-properties fo:page-width="28cm" fo:page-height="15.75cm" style:print-orientation="landscape"/></style:page-layout><style:style style:name="AcmeBackground" style:family="drawing-page"><style:drawing-page-properties draw:fill="solid" draw:fill-color="#E8F5E9"/></style:style></office:automatic-styles><office:master-styles><style:master-page style:name="Acme" style:page-layout-name="AcmePage" draw:style-name="AcmeBackground"><draw:frame draw:style-name="Logo" svg:x="23cm" svg:y="14.5cm" svg:width="4cm" svg:height="0.8cm"><draw:text-box><text:p>ACME</text:p></draw:text-box></draw:frame></style:master-page></office:master-styles></office:document-styles>"##
    );
    let mut custom_layouts = String::new();
    for (name, objects) in [
        ("title", vec!["title", "subtitle"]),
        ("outline", vec!["title", "outline"]),
        ("two", vec!["title", "outline", "outline"]),
        ("section", vec!["title"]),
        ("blank", vec![]),
    ] {
        custom_layouts.push_str(&format!(
            r#"<style:presentation-page-layout style:name="Acme-{name}">"#
        ));
        for (i, object) in objects.iter().enumerate() {
            let (x, y, w, h) = if i == 0 {
                ("1cm", "1cm", "25cm", "2cm")
            } else if i == 1 {
                ("1cm", "4cm", "12cm", "9cm")
            } else {
                ("15cm", "4cm", "12cm", "9cm")
            };
            custom_layouts.push_str(&format!(r#"<presentation:placeholder presentation:object="{object}" svg:x="{x}" svg:y="{y}" svg:width="{w}" svg:height="{h}"/>"#));
        }
        custom_layouts.push_str("</style:presentation-page-layout>");
    }
    let custom_text = r##"<style:style style:name="Acme-title" style:family="presentation"><style:text-properties fo:font-family="Liberation Serif" fo:font-size="32pt" fo:color="#512680"/></style:style><style:style style:name="Acme-outline" style:family="presentation"><style:text-properties fo:font-family="Liberation Serif" fo:font-size="22pt" fo:color="#512680"/></style:style>"##;
    let styles = styles.replace(
        "<office:styles>",
        &format!("<office:styles>{custom_layouts}{custom_text}"),
    );
    parts.insert("styles.xml".into(), styles.into_bytes());
    let mut pages = String::new();
    for (i, kind) in ["title", "outline", "two", "section", "blank"]
        .iter()
        .enumerate()
    {
        pages.push_str(&format!(
            r#"<draw:page draw:name="Example{i}" draw:master-page-name="Acme" presentation:presentation-page-layout-name="Acme-{kind}">"#
        ));
        if *kind != "blank" {
            pages.push_str(r#"<draw:frame presentation:class="title" presentation:style-name="Acme-title" svg:x="1cm" svg:y="1cm" svg:width="24cm" svg:height="2cm"><draw:text-box><text:p>REMOVE SAMPLE TITLE</text:p></draw:text-box></draw:frame>"#);
        }
        if matches!(*kind, "outline" | "two") {
            pages.push_str(r#"<draw:frame presentation:class="outline" presentation:style-name="Acme-outline" svg:x="1cm" svg:y="4cm" svg:width="12cm" svg:height="9cm"><draw:text-box><text:p>REMOVE SAMPLE BODY</text:p></draw:text-box></draw:frame>"#);
        }
        if *kind == "two" {
            pages.push_str(r#"<draw:frame presentation:class="outline" presentation:style-name="Acme-outline" svg:x="15cm" svg:y="4cm" svg:width="12cm" svg:height="9cm"><draw:text-box><text:p>REMOVE SECOND COLUMN</text:p></draw:text-box></draw:frame>"#);
        }
        pages.push_str("</draw:page>");
    }
    parts.insert("content.xml".into(),format!(r#"<?xml version="1.0"?><office:document-content {ns}><office:body><office:presentation>{pages}</office:presentation></office:body></office:document-content>"#).into_bytes());
    std::fs::write(dir.join("company.odp"), pack(&parts)?)?;
    let profile = tempfile::tempdir()?;
    let output = Command::new("soffice")
        .arg(format!(
            "-env:UserInstallation=file://{}",
            profile.path().display()
        ))
        .args(["--headless", "--convert-to", "pptx", "--outdir"])
        .arg(dir)
        .arg(dir.join("company.odp"))
        .output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        dir.join("company.pptx").exists(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    Ok(())
}
#[test]
fn libreoffice_company_template_preserves_design_and_removes_samples() -> TestResult {
    let dir = tempfile::tempdir()?;
    lo_template(dir.path())?;
    let original = unpack(&std::fs::read(dir.path().join("company.pptx"))?)?;
    let layouts = call(
        dir.path(),
        json!({"op":"read","input":"company.pptx","layouts":true}),
    )?
    .data
    .ok_or("no layouts")?;
    let list = layouts["layouts"].as_array().ok_or("no layout array")?;
    assert!(list.len() > 1);
    assert!(
        list.iter()
            .any(|l| l["placeholders"].as_array().is_some_and(|p| !p.is_empty()))
    );
    let title_name = list
        .iter()
        .find(|l| l["type"] == "title")
        .or(list.first())
        .ok_or("no layout")?["name"]
        .as_str()
        .ok_or("no name")?;
    let result = call(
        dir.path(),
        json!({"op":"create","output":"deck.pptx","template":"company.pptx","slides":[
            {"layout":title_name,"title":"Company update","subtitle":"Board presentation","sources":["Source: model.xlsx, DCF!C25"]},
            {"layout":"title_content","title":"Operating performance","bullets":["Revenue increased","Costs remain stable"],"sources":["Source: internal reporting"],"notes":"Review with the board"},
            {"layout":"two_content","title":"Two perspectives","bullets":["Revenue","Margin","Growth","Cash"]},
            {"layout":"section","title":"Outlook"},
            {"layout":"blank"},
            {"layout":"Missing company layout","title":"Fallback","text":"A nearest layout is selected"}
        ]}),
    )?;
    assert!(
        result
            .warnings
            .iter()
            .any(|w| w.code == "template.layout_fallback")
    );
    let out = unpack(&std::fs::read(dir.path().join("deck.pptx"))?)?;
    for (name, bytes) in &original {
        if name.starts_with("ppt/slideMasters/")
            || name.starts_with("ppt/slideLayouts/")
            || name.starts_with("ppt/theme/")
        {
            assert_eq!(out.get(name), Some(bytes), "changed {name}");
        }
    }
    let columns = out
        .iter()
        .find(|(name, _)| name.ends_with("ppt/slides/slide3.xml"))
        .ok_or("no two-column slide")?
        .1;
    assert_eq!(
        String::from_utf8_lossy(columns).matches("<p:ph").count(),
        3,
        "both company body placeholders must be populated"
    );
    assert!(
        !out.values()
            .any(|b| String::from_utf8_lossy(b).contains("REMOVE SAMPLE"))
    );
    let read = call(dir.path(), json!({"op":"read","input":"deck.pptx"}))?;
    let slides = read.data.ok_or("no slides")?;
    assert_eq!(slides.as_array().ok_or("no array")?.len(), 6);
    assert_eq!(slides[0]["title"], "Company update");
    assert_eq!(slides[0]["sources"][0], "Source: model.xlsx, DCF!C25");
    assert_eq!(slides[1]["notes"], "Review with the board");
    call(
        dir.path(),
        json!({"op":"convert","input":"deck.pptx","output":"deck.pdf"}),
    )?;
    let render = Command::new("pdftoppm")
        .args(["-f", "1", "-singlefile", "-scale-to", "1000", "-png"])
        .arg(dir.path().join("deck.pdf"))
        .arg(dir.path().join("preview"))
        .output()?;
    assert!(render.status.success());
    let text = Command::new("pdftotext")
        .arg(dir.path().join("deck.pdf"))
        .arg("-")
        .output()?;
    assert!(text.status.success());
    let text = String::from_utf8(text.stdout)?;
    assert!(text.contains("ACME"));
    assert!(text.contains("Source:"));
    assert!(text.contains("Company update"));
    if let Some(path) = std::env::var_os("SLIDES_ARTIFACT_DIR") {
        std::fs::create_dir_all(&path)?;
        for name in [
            "company.odp",
            "company.pptx",
            "deck.pptx",
            "deck.pdf",
            "preview.png",
        ] {
            std::fs::copy(dir.path().join(name), Path::new(&path).join(name))?;
        }
    }
    Ok(())
}
#[test]
fn potx_markdown_conversion_sources_and_layout_listing() -> TestResult {
    let dir = tempfile::tempdir()?;
    call(
        dir.path(),
        json!({"op":"create","output":"company.pptx","markdown":"# Template sample"}),
    )?;
    let mut pkg = unpack(&std::fs::read(dir.path().join("company.pptx"))?)?;
    let ct = String::from_utf8(pkg.remove("[Content_Types].xml").ok_or("no CT")?)?.replace(
        "presentationml.presentation.main+xml",
        "presentationml.template.main+xml",
    );
    pkg.insert("[Content_Types].xml".into(), ct.into_bytes());
    std::fs::write(dir.path().join("company.potx"), pack(&pkg)?)?;
    call(
        dir.path(),
        json!({"op":"read","input":"company.potx","layouts":true}),
    )?;
    std::fs::write(
        dir.path().join("deck.md"),
        "## Revenue\nRevenue: $25 mln\n\n<!-- source: Source: model.xlsx, DCF!C25 -->\n",
    )?;
    call(
        dir.path(),
        json!({"op":"convert","input":"deck.md","output":"deck.pptx","template":"company.potx"}),
    )?;
    let read = call(dir.path(), json!({"op":"read","input":"deck.pptx"}))?
        .data
        .ok_or("no data")?;
    assert_eq!(read[0]["sources"][0], "Source: model.xlsx, DCF!C25");
    assert!(
        !String::from_utf8_lossy(
            unpack(&std::fs::read(dir.path().join("deck.pptx"))?)?
                .get("[Content_Types].xml")
                .ok_or("no CT")?
        )
        .contains("template.main")
    );
    Ok(())
}
fn model(dir: &Path) -> TestResult {
    let entries = [
        (
            "[Content_Types].xml",
            r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="xml" ContentType="application/xml"/></Types>"#,
        ),
        (
            "xl/workbook.xml",
            r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="DCF" sheetId="1" r:id="rId1"/></sheets><definedNames><definedName name="Revenue">'DCF'!$C$25</definedName><definedName name="Margin">DCF!C26</definedName><definedName name="OperatingMetrics">DCF!$C$25:$C$26</definedName></definedNames></workbook>"#,
        ),
        (
            "xl/_rels/workbook.xml.rels",
            r#"<Relationships><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#,
        ),
        (
            "xl/worksheets/sheet1.xml",
            r#"<worksheet><sheetData><row r="25"><c r="A25" t="inlineStr"><is><t>Revenue</t></is></c><c r="C25"><v>25000000</v></c></row><row r="26"><c r="A26" t="inlineStr"><is><t>Margin</t></is></c><c r="C26"><f>1/4</f><v>0.25</v></c></row></sheetData></worksheet>"#,
        ),
    ];
    std::fs::write(
        dir.join("model.xlsx"),
        pack(
            &entries
                .into_iter()
                .map(|(n, s)| (n.into(), s.as_bytes().to_vec()))
                .collect(),
        )?,
    )?;
    Ok(())
}
#[test]
fn check_finds_injected_metrics_totals_percentages_and_model_mismatches() -> TestResult {
    let dir = tempfile::tempdir()?;
    model(dir.path())?;
    call(
        dir.path(),
        json!({"op":"create","output":"bad.pptx","slides":[
            {"title":"Overview","text":"Revenue: $25 mln","sources":["Source: model.xlsx, DCF!C25"]},
            {"title":"Detail","text":"Revenue: $26 mln","sources":["Source: model.xlsx, Revenue"]},
            {"title":"Totals","table":{"headers":["Segment","Revenue","Share"],"rows":[["A","40","30%"],["B","50","50%"],["Total","100","100%"]]}},
            {"title":"Margin","text":"Margin: 30%"},
            {"title":"Chart","chart":{"type":"bar","labels":["North","South"],"series":[{"name":"Sales","values":[10,20]}]}}
        ]}),
    )?;
    let output = call(
        dir.path(),
        json!({"op":"check","input":"bad.pptx","model":"model.xlsx"}),
    )?;
    let data = output.data.ok_or("no findings")?;
    let findings = data.as_array().ok_or("not array")?;
    for (slide, issue, value, expected) in [
        (2, "check.metric_mismatch", 26e6, 25e6),
        (2, "check.model_mismatch", 26e6, 25e6),
        (3, "check.table_total", 100., 90.),
        (3, "check.table_percent", 0.8, 1.),
        (4, "check.model_mismatch", 0.3, 0.25),
    ] {
        assert!(
            findings.iter().any(|f| f["slide"] == slide
                && f["issue"] == issue
                && f["value"] == value
                && f["expected"] == expected),
            "missing {issue} on slide {slide}: {findings:?}"
        );
    }
    assert!(
        output.summary_params["numbers"]
            .as_str()
            .ok_or("no number count")?
            .parse::<usize>()?
            > 10
    );
    assert!(!findings.iter().any(|f| f["slide"] == 1));
    Ok(())
}
#[test]
fn check_tolerance_and_equivalent_units_have_no_false_mismatch() -> TestResult {
    let dir = tempfile::tempdir()?;
    model(dir.path())?;
    call(
        dir.path(),
        json!({"op":"create","output":"good.pptx","slides":[
            {"title":"Summary","text":"Revenue: $25 mln"},
            {"title":"Detail","text":"Revenue: $0.025 bn"},
            {"title":"Margin","text":"Margin: 25.01%"},
            {"title":"Mix","table":{"headers":["Segment","Share"],"rows":[["A","40%"],["B","60%"],["Total","100%"]]}}
        ]}),
    )?;
    let output = call(
        dir.path(),
        json!({"op":"check","input":"good.pptx","model":"model.xlsx","tolerance":0.0002}),
    )?;
    assert_eq!(output.data, Some(json!([])));
    let strict = call(
        dir.path(),
        json!({"op":"check","input":"good.pptx","model":"model.xlsx","tolerance":0}),
    )?;
    assert!(
        strict
            .data
            .ok_or("no findings")?
            .as_array()
            .ok_or("no array")?
            .iter()
            .any(|f| f["issue"] == "check.model_mismatch")
    );
    Ok(())
}
#[test]
fn company_template_keeps_native_objects_and_reports_unmapped_content() -> TestResult {
    let dir = tempfile::tempdir()?;
    call(
        dir.path(),
        json!({"op":"create","output":"company.pptx","markdown":"# Sample"}),
    )?;
    let result = call(
        dir.path(),
        json!({"op":"create","template":"company.pptx","output":"objects.pptx","slides":[
            {"title":"Native objects","table":{"headers":["Segment","Sales"],"rows":[["North","10"]]},"chart":{"type":"bar","labels":["North","South"],"series":[{"name":"Sales","values":[10,20]}]},"notes":"Confirm both objects"},
            {"layout":"title","title":"Additional content","subtitle":"Company subtitle","text":"No third company text placeholder","sources":["First source","Second source","Third source","Fourth source"]}
        ]}),
    )?;
    for code in [
        "template.content_review",
        "template.placeholder_fallback",
        "source.overflow",
    ] {
        assert!(
            result.warnings.iter().any(|w| w.code == code),
            "missing warning {code}"
        );
    }
    let read = call(dir.path(), json!({"op":"read","input":"objects.pptx"}))?
        .data
        .ok_or("no data")?;
    assert_eq!(read[0]["notes"], "Confirm both objects");
    let text = read[0]["text"].as_str().ok_or("no object text")?;
    assert!(text.contains("Segment\tSales"));
    assert!(text.contains("Sales\t10\t20"));
    call(
        dir.path(),
        json!({"op":"convert","input":"objects.pptx","output":"objects.pdf"}),
    )?;
    Ok(())
}

#[test]
fn named_range_chart_labels_and_unresolved_sources_are_checked() -> TestResult {
    let dir = tempfile::tempdir()?;
    model(dir.path())?;
    call(
        dir.path(),
        json!({"op":"create","output":"linked.pptx","slides":[
            {"title":"Range","text":"Revenue: $26 mln\nMargin: 30%","sources":["Source: model.xlsx, OperatingMetrics"]},
            {"title":"Unresolved","text":"Cash: 50","sources":["Source: model.xlsx, DCF!X999"]},
            {"title":"Chart A","chart":{"type":"bar","labels":["North"],"series":[{"name":"Sales","values":[10]}]}},
            {"title":"Chart B","chart":{"type":"bar","labels":["North"],"series":[{"name":"Sales","values":[12]}]}}
        ]}),
    )?;
    let output = call(
        dir.path(),
        json!({"op":"check","input":"linked.pptx","model":"model.xlsx"}),
    )?;
    let findings = output.data.ok_or("no findings")?;
    assert_eq!(
        findings
            .as_array()
            .ok_or("not array")?
            .iter()
            .filter(|f| f["slide"] == 1 && f["issue"] == "check.model_mismatch")
            .count(),
        2
    );
    assert!(
        findings
            .as_array()
            .ok_or("not array")?
            .iter()
            .any(|f| f["slide"] == 2 && f["issue"] == "check.source_unresolved")
    );
    assert!(
        findings
            .as_array()
            .ok_or("not array")?
            .iter()
            .any(|f| f["slide"] == 4
                && f["value"].as_f64() == Some(12.0)
                && f["expected"].as_f64() == Some(10.0)
                && f["issue"] == "check.metric_mismatch")
    );
    assert!(
        output
            .warnings
            .iter()
            .any(|w| w.code == "check.model_unlinked" && w.params["slide"] == "2")
    );
    Ok(())
}

#[test]
fn template_and_model_paths_limits_unknown_fields_and_tolerance_are_rejected() -> TestResult {
    let dir = tempfile::tempdir()?;
    call(
        dir.path(),
        json!({"op":"create","output":"deck.pptx","markdown":"# Control"}),
    )?;
    for path in ["../outside.pptx", "/outside.pptx"] {
        let input: Input = serde_json::from_value(
            json!({"op":"create","output":"out.pptx","markdown":"# Test","template":path}),
        )?;
        assert_eq!(
            run(&input, &Options::new(dir.path()))
                .err()
                .ok_or("accepted template path")?
                .code(),
            "path.unsafe"
        );
        let input: Input =
            serde_json::from_value(json!({"op":"check","input":"deck.pptx","model":path}))?;
        assert_eq!(
            run(&input, &Options::new(dir.path()))
                .err()
                .ok_or("accepted model path")?
                .code(),
            "path.unsafe"
        );
    }
    let mut opts = Options::new(dir.path());
    opts.max_input_bytes = 1;
    let input: Input = serde_json::from_value(
        json!({"op":"create","output":"out.pptx","markdown":"# Test","template":"deck.pptx"}),
    )?;
    assert_eq!(
        run(&input, &opts)
            .err()
            .ok_or("accepted oversized template")?
            .code(),
        "input.too_large"
    );
    let size: u64 = unpack(&std::fs::read(dir.path().join("deck.pptx"))?)?
        .values()
        .map(|b| b.len() as u64)
        .sum();
    std::fs::write(
        dir.path().join("large.xlsx"),
        vec![0u8; usize::try_from(size + 1)?],
    )?;
    opts.max_input_bytes = size;
    let input: Input =
        serde_json::from_value(json!({"op":"check","input":"deck.pptx","model":"large.xlsx"}))?;
    assert_eq!(
        run(&input, &opts)
            .err()
            .ok_or("accepted oversized model")?
            .code(),
        "input.too_large"
    );
    for value in [
        json!({"op":"check","input":"deck.pptx","extra":1}),
        json!({"op":"create","output":"x.pptx","markdown":"# X","templates":"x.pptx"}),
        json!({"op":"read","input":"x.pptx","layout":true}),
    ] {
        assert!(serde_json::from_value::<Input>(value).is_err());
    }
    let input: Input =
        serde_json::from_value(json!({"op":"check","input":"deck.pptx","tolerance":-1}))?;
    assert_eq!(
        run(&input, &Options::new(dir.path()))
            .err()
            .ok_or("accepted negative tolerance")?
            .code(),
        "check.invalid_tolerance"
    );
    Ok(())
}
