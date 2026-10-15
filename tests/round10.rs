//! Inline fidelity, compatibility branches, source references and period dimensions.
use serde_json::{Value, json};
use slides::{Input, Options, Output, run};
use std::collections::BTreeMap;
use std::io::{Cursor, Read, Write};
use std::path::Path;
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
fn change(p: &mut Parts, name: &str, f: impl FnOnce(String) -> String) -> Result {
    let bytes = p.get_mut(name).ok_or("missing part")?;
    *bytes = f(String::from_utf8(bytes.clone())?).into_bytes();
    Ok(())
}
fn artifacts(dir: &Path, names: &[&str]) -> Result {
    if let Some(destination) = std::env::var_os("SLIDES_TEST_ARTIFACTS") {
        let destination = std::path::PathBuf::from(destination);
        std::fs::create_dir_all(&destination)?;
        for name in names {
            std::fs::copy(dir.join(name), destination.join(name))?;
        }
    }
    Ok(())
}

#[test]
fn commonmark_inline_text_survives_create_read_and_markdown_export() -> Result {
    let dir = tempfile::tempdir()?;
    let cases = [
        ("North**star** Analytics", "Northstar Analytics"),
        ("North*star* Analytics", "Northstar Analytics"),
        ("North`star` Analytics", "Northstar Analytics"),
        (
            "North[star](https://example.invalid) Analytics",
            "Northstar Analytics",
        ),
        ("Margin: 2**5**%", "Margin: 25%"),
        ("Margin: 2*5*%", "Margin: 25%"),
        ("Margin: 2`5`%", "Margin: 25%"),
        ("**Revenue**, net.", "Revenue, net."),
        ("Revenue *net*.", "Revenue net."),
        ("**25**%", "25%"),
        ("$**25** mln", "$25 mln"),
        ("1,**250**.5", "1,250.5"),
        ("**4**.2x", "4.2x"),
        ("(**3.5**) bn", "(3.5) bn"),
        ("pre**fix**post", "prefixpost"),
        ("a***b***c", "abc"),
        ("**bold**/**next**", "bold/next"),
        ("**one**-**two**", "one-two"),
        ("**word**!", "word!"),
        ("**word**?", "word?"),
        ("(**word**)", "(word)"),
        ("[**word**](https://example.invalid).", "word."),
        ("a\\*b", "a*b"),
        ("a&amp;b", "a&b"),
        ("a&#37;b", "a%b"),
        ("foo_bar_baz", "foo_bar_baz"),
        ("foo**bar**baz", "foobarbaz"),
        ("`a b`c", "a bc"),
        ("**a**  **b**", "a  b"),
        ("a **b** c", "a b c"),
        ("a\\_b", "a_b"),
        ("**a**:**b**", "a:b"),
    ];
    for (input, expected) in cases {
        call(
            dir.path(),
            json!({"op":"create","output":"inline.pptx","markdown":format!("## {input}\n\n{input}\n")}),
        )?;
        let read = call(dir.path(), json!({"op":"read","input":"inline.pptx"}))?;
        let data = read.data.ok_or("no data")?;
        assert_eq!(data[0]["title"], expected, "heading: {input}");
        assert_eq!(data[0]["text"], expected, "body: {input}");
        call(
            dir.path(),
            json!({"op":"convert","input":"inline.pptx","output":"inline.md"}),
        )?;
        assert!(
            std::fs::read_to_string(dir.path().join("inline.md"))?.contains(expected),
            "{input}"
        );
    }
    call(
        dir.path(),
        json!({"op":"create","output":"inline-word.pptx","markdown":"# North**star** Analytics\n\nMargin: 2**5**%\n\n## Results\nRevenue **net**, $**25** mln.\n"}),
    )?;
    call(
        dir.path(),
        json!({"op":"create","output":"inline-word.pdf","markdown":"# North**star** Analytics\n\nMargin: 2**5**%\n\n## Results\nRevenue **net**, $**25** mln.\n"}),
    )?;
    let text = std::process::Command::new("pdftotext")
        .arg(dir.path().join("inline-word.pdf"))
        .arg("-")
        .output()?;
    assert!(text.status.success());
    let text = String::from_utf8(text.stdout)?;
    assert!(
        text.contains("Northstar Analytics") && text.contains("25%") && text.contains("net,"),
        "{text}"
    );
    artifacts(dir.path(), &["inline-word.pptx", "inline-word.pdf"])?;
    Ok(())
}

#[test]
fn compatibility_branches_are_selected_once_at_every_group_depth() -> Result {
    let dir = tempfile::tempdir()?;
    let image = printpdf::RawImage {
        pixels: printpdf::RawImageData::U8(vec![60; 120 * 60 * 3]),
        width: 120,
        height: 60,
        data_format: printpdf::RawImageFormat::RGB8,
        tag: Vec::new(),
    };
    std::fs::write(
        dir.path().join("image.png"),
        image
            .encode_to_bytes(&[printpdf::OutputImageFormat::Png])?
            .0,
    )?;
    call(
        dir.path(),
        json!({"op":"create","output":"base.pptx","slides":[{"title":"Northstar Analytics","text":"Revenue: 25","image":"image.png","table":{"headers":["Metric","Value"],"rows":[["Count","25"]]}}]}),
    )?;
    let original = parts(&dir.path().join("base.pptx"))?;
    let base_read = call(dir.path(), json!({"op":"read","input":"base.pptx"}))?;
    for (name, supported, fallback, nested) in [
        ("fallback", false, true, false),
        ("choice", true, true, false),
        ("nested", false, true, true),
        ("all-required", false, true, false),
        ("unbound-required", false, true, false),
        ("choice-without-fallback", true, false, true),
        ("unavailable", false, false, false),
    ] {
        let mut p = original.clone();
        change(&mut p, "ppt/slides/slide1.xml", |s| {
            let s = s.replace("<p:sld ", "<p:sld xmlns:compat=\"http://schemas.openxmlformats.org/markup-compatibility/2006\" xmlns:d=\"http://schemas.openxmlformats.org/drawingml/2006/main\" xmlns:future=\"urn:unsupported\" ");
            let start = s.find("<p:sp>").unwrap_or(s.len());
            let end = s.rfind("</p:spTree>").unwrap_or(s.len());
            let objects = &s[start..end];
            let alternate = format!(
                "<compat:AlternateContent><compat:Choice Requires=\"{}\">{}</compat:Choice>{}</compat:AlternateContent>",
                if supported {
                    "d"
                } else if name == "all-required" {
                    "d future"
                } else if name == "unbound-required" {
                    "unbound"
                } else {
                    "future"
                },
                if supported {
                    objects.to_string()
                } else {
                    objects.replace("Northstar Analytics", "WRONG BRANCH")
                },
                if fallback {
                    format!(
                        "<compat:Fallback>{}</compat:Fallback>",
                        if supported {
                            objects.replace("Northstar Analytics", "WRONG FALLBACK")
                        } else {
                            objects.to_string()
                        }
                    )
                } else {
                    String::new()
                }
            );
            let wrapper = if nested {
                format!(
                    "<p:grpSp><compat:AlternateContent><compat:Choice Requires=\"future\"/><compat:Fallback>{alternate}</compat:Fallback></compat:AlternateContent></p:grpSp>"
                )
            } else {
                alternate
            };
            format!("{}{}{}", &s[..start], wrapper, &s[end..])
        })?;
        pack(&dir.path().join("compatibility.pptx"), &p)?;
        let read = call(
            dir.path(),
            json!({"op":"read","input":"compatibility.pptx"}),
        )?;
        let exported = call(
            dir.path(),
            json!({"op":"convert","input":"compatibility.pptx","output":"compatibility.md"}),
        )?;
        let converted = std::fs::read_to_string(dir.path().join("compatibility.md"))?;
        if name == "unavailable" {
            assert!(
                read.warnings
                    .iter()
                    .any(|w| w.code == "read.compatibility_unsupported")
            );
            assert!(
                exported
                    .warnings
                    .iter()
                    .any(|w| w.code == "read.compatibility_unsupported")
            );
        } else {
            assert!(read.warnings.is_empty(), "{name}: {:?}", read.warnings);
            assert!(
                exported.warnings.is_empty(),
                "{name}: {:?}",
                exported.warnings
            );
            assert_eq!(read.data, base_read.data, "{name}");
            let checked = call(
                dir.path(),
                json!({"op":"check","input":"compatibility.pptx"}),
            )?;
            let baseline = call(dir.path(), json!({"op":"check","input":"base.pptx"}))?;
            assert_eq!(
                checked.summary_params["numbers"], baseline.summary_params["numbers"],
                "branches counted twice"
            );
            assert_eq!(read.text, base_read.text, "{name}");
            assert!(!converted.contains("WRONG"));
            assert!(converted.contains("Northstar Analytics"));
        }
        if name == "fallback" {
            artifacts(dir.path(), &["compatibility.pptx", "compatibility.md"])?;
        }
    }
    Ok(())
}

fn workbook(dir: &Path, sheet: &str, defined: &str) -> Result {
    let mut p = Parts::new();
    p.insert("xl/workbook.xml".into(), format!("<workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"><sheets><sheet name=\"{sheet}\" sheetId=\"1\" r:id=\"rId1\"/></sheets><definedNames>{defined}</definedNames></workbook>").into_bytes());
    p.insert("xl/_rels/workbook.xml.rels".into(), b"<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet1.xml\"/></Relationships>".to_vec());
    p.insert("xl/worksheets/sheet1.xml".into(), b"<worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><sheetData><row r=\"2\"><c r=\"A2\" t=\"inlineStr\"><is><t>Revenue</t></is></c><c r=\"B2\"><v>25000000</v></c></row><row r=\"3\"><c r=\"A3\" t=\"inlineStr\"><is><t>Margin</t></is></c><c r=\"B3\"><v>0.25</v></c></row></sheetData></worksheet>".to_vec());
    pack(&dir.join("finance.xlsx"), &p)
}
fn check_source(dir: &Path, source: &str) -> Result<Output> {
    call(
        dir,
        json!({"op":"create","output":"sources.pptx","slides":[{"title":"Performance","text":"Revenue: $26 mln","sources":[source]}]}),
    )?;
    call(
        dir,
        json!({"op":"check","input":"sources.pptx","model":"finance.xlsx"}),
    )
}
#[test]
fn each_unresolved_reference_is_reported_beside_valid_sources() -> Result {
    let dir = tempfile::tempdir()?;
    workbook(dir.path(), "DCF", "")?;
    let checked = check_source(
        dir.path(),
        "Source: finance.xlsx, DCF!B2, DCF!Z99, DCF!Z100",
    )?;
    let data = checked.data.ok_or("no findings")?;
    let unresolved: Vec<_> = data
        .as_array()
        .ok_or("not array")?
        .iter()
        .filter(|f| f["issue"] == "check.source_unresolved")
        .collect();
    assert_eq!(unresolved.len(), 2, "{data}");
    assert!(
        unresolved
            .iter()
            .any(|f| f["text"].as_str().is_some_and(|s| s.contains("Z99")))
    );
    assert!(
        unresolved
            .iter()
            .any(|f| f["text"].as_str().is_some_and(|s| s.contains("Z100")))
    );
    assert!(
        data.as_array()
            .ok_or("not array")?
            .iter()
            .any(|f| f["issue"] == "check.model_mismatch")
    );
    Ok(())
}
#[test]
fn reference_lexer_handles_quoted_commas_apostrophes_ranges_and_defined_unions() -> Result {
    let dir = tempfile::tempdir()?;
    for (sheet, quoted) in [
        ("Sales, US", "'Sales, US'"),
        ("O'Brien, US", "'O''Brien, US'"),
        ("Sales: US", "'Sales: US'"),
    ] {
        workbook(
            dir.path(),
            sheet,
            &format!(
                "<definedName name=\"RevenueRange\">{quoted}!$B$2,{quoted}!$B$3</definedName>"
            ),
        )?;
        for reference in [
            format!("{quoted}!$B$2"),
            format!("{quoted}!$B$2:$B$3"),
            "RevenueRange".into(),
        ] {
            let checked = check_source(dir.path(), &format!("Source: finance.xlsx, {reference}"))?;
            let data = checked.data.ok_or("no findings")?;
            assert!(
                data.as_array()
                    .ok_or("not array")?
                    .iter()
                    .any(|f| f["issue"] == "check.model_mismatch" && f["expected"] == 25_000_000.0),
                "{sheet}: {reference}: {data}"
            );
            assert!(
                !data
                    .as_array()
                    .ok_or("not array")?
                    .iter()
                    .any(|f| f["issue"] == "check.source_unresolved"),
                "{data}"
            );
            assert!(
                !checked
                    .warnings
                    .iter()
                    .any(|w| w.code == "check.model_unlinked")
            );
        }
    }
    workbook(
        dir.path(),
        "Sales, US",
        "<definedName name=\"Partial\">'Sales, US'!B2,'Sales, US'!Z99</definedName>",
    )?;
    let checked = check_source(dir.path(), "Source: finance.xlsx, Partial")?;
    assert!(
        checked
            .data
            .ok_or("no findings")?
            .as_array()
            .ok_or("not array")?
            .iter()
            .any(|f| f["issue"] == "check.source_unresolved")
    );
    Ok(())
}
#[test]
fn years_quarters_months_and_dates_are_dimensions_not_measures() -> Result {
    let dir = tempfile::tempdir()?;
    for (first, second) in [
        ("2027", "2028"),
        ("Q1 2027", "Q2 2027"),
        ("January 2027", "February 2027"),
        ("01/2027", "02/2027"),
        ("2027-01-31", "2027-02-28"),
        ("FY2027", "FY2028"),
    ] {
        for same in [false, true] {
            let second = if same { first } else { second };
            call(
                dir.path(),
                json!({"op":"create","output":"periods.pptx","slides":[{"title":"First period","text":format!("Revenue {first}: $25 mln")},{"title":"Second period","text":format!("Revenue {second}: $26 mln")}]}),
            )?;
            let checked = call(dir.path(), json!({"op":"check","input":"periods.pptx"}))?;
            let data = checked.data.ok_or("no findings")?;
            let findings = data.as_array().ok_or("not array")?;
            assert_eq!(
                findings.len(),
                usize::from(same),
                "{first}/{second}: {data}"
            );
            if same {
                assert_eq!(findings[0]["value"], 26_000_000.0);
                assert_eq!(findings[0]["expected"], 25_000_000.0);
            }
            assert_eq!(
                checked.summary_params["numbers"], "2",
                "periods counted as measurements"
            );
        }
    }
    Ok(())
}
