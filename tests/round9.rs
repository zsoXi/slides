//! Namespace, semantic QC, pagination, edit feedback and error protocol regressions.
use serde_json::{Value, json};
use slides::{Input, Options, Output, run};
use std::collections::BTreeMap;
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
    let mut parts = BTreeMap::new();
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes)?;
        parts.insert(entry.name()?.to_string(), bytes);
    }
    Ok(parts)
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
fn change(parts: &mut Parts, name: &str, f: impl FnOnce(String) -> String) -> Result {
    let bytes = parts.get_mut(name).ok_or("missing part")?;
    *bytes = f(String::from_utf8(bytes.clone())?).into_bytes();
    Ok(())
}

#[test]
fn namespace_aliases_default_and_scoped_rebinding_roundtrip() -> Result {
    let dir = tempfile::tempdir()?;
    call(
        dir.path(),
        json!({"op":"create","output":"base.pptx","slides":[{"title":"Northstar Analytics","text":"Revenue: 10\n20 customers"}]}),
    )?;
    let baseline = call(dir.path(), json!({"op":"read","input":"base.pptx"}))?;
    let original = parts(&dir.path().join("base.pptx"))?;
    for prefix in ["d", ""] {
        let mut p = original.clone();
        for (name, bytes) in &mut p {
            if name.ends_with(".xml") {
                let text = String::from_utf8(bytes.clone())?;
                *bytes = if prefix.is_empty() {
                    text.replace("xmlns:a=", "xmlns=")
                        .replace("<a:", "<")
                        .replace("</a:", "</")
                } else {
                    text.replace("xmlns:a=", "xmlns:d=")
                        .replace("<a:", "<d:")
                        .replace("</a:", "</d:")
                }
                .into_bytes();
            }
        }
        // Relationship attributes also use namespace identity, independently of elements.
        change(&mut p, "ppt/presentation.xml", |s| {
            s.replace("xmlns:r=", "xmlns:rel=")
                .replace("r:id=", "rel:id=")
        })?;
        pack(&dir.path().join("alias.pptx"), &p)?;
        let read = call(dir.path(), json!({"op":"read","input":"alias.pptx"}))?;
        assert_eq!(read.text, baseline.text, "prefix {prefix}");
        assert_eq!(read.data, baseline.data, "prefix {prefix}");
        call(
            dir.path(),
            json!({"op":"convert","input":"alias.pptx","output":"alias.md"}),
        )?;
        assert!(std::fs::read_to_string(dir.path().join("alias.md"))?.contains("Revenue: 10"));
        call(
            dir.path(),
            json!({"op":"edit","input":"alias.pptx","output":"edited.pptx","replace":{"find":"Northstar","replace":"Clearview"}}),
        )?;
        let edited = parts(&dir.path().join("edited.pptx"))?;
        let xml = std::str::from_utf8(&edited["ppt/slides/slide1.xml"])?;
        assert!(xml.contains(if prefix.is_empty() {
            "<t xml:space=\"preserve\">Clearview"
        } else {
            "<d:t xml:space=\"preserve\">Clearview"
        }));
        assert!(
            call(dir.path(), json!({"op":"read","input":"edited.pptx"}))?
                .text
                .ok_or("no text")?
                .contains("Clearview Analytics")
        );
    }
    let mut p = original;
    change(&mut p, "ppt/slides/slide1.xml", |s| {
        s.replace(
            "<a:p>",
            "<a:p><a:r xmlns:a=\"urn:not-drawingml\"><a:t>INJECTED</a:t></a:r>",
        )
    })?;
    pack(&dir.path().join("rebound.pptx"), &p)?;
    let text = call(dir.path(), json!({"op":"read","input":"rebound.pptx"}))?
        .text
        .ok_or("no text")?;
    assert!(!text.contains("INJECTED"));
    assert!(text.contains("Northstar Analytics"));
    Ok(())
}

#[test]
fn namespace_aliases_keep_new_opc_entries_qualified() -> Result {
    let dir = tempfile::tempdir()?;
    call(
        dir.path(),
        json!({"op":"create","output":"base.pptx","markdown":"# Original"}),
    )?;
    let mut p = parts(&dir.path().join("base.pptx"))?;
    for (path, bytes) in &mut p {
        let tags: &[&str] = if path.ends_with(".rels") {
            &["Relationships", "Relationship"]
        } else if path == "[Content_Types].xml" {
            &["Types", "Default", "Override"]
        } else {
            continue;
        };
        let mut xml = String::from_utf8(bytes.clone())?.replace("xmlns=", "xmlns:ns=");
        for tag in tags {
            xml = xml
                .replace(&format!("<{tag} "), &format!("<ns:{tag} "))
                .replace(&format!("<{tag}>"), &format!("<ns:{tag}>"))
                .replace(&format!("</{tag}>"), &format!("</ns:{tag}>"));
        }
        *bytes = xml.into_bytes();
    }
    pack(&dir.path().join("foreign.pptx"), &p)?;
    call(
        dir.path(),
        json!({"op":"edit","input":"foreign.pptx","output":"edited.pptx","slides":[{"index":0,"notes":"Added note"}]}),
    )?;
    let p = parts(&dir.path().join("edited.pptx"))?;
    for (path, bytes) in &p {
        let expected = if path.ends_with(".rels") {
            "http://schemas.openxmlformats.org/package/2006/relationships"
        } else if path == "[Content_Types].xml" {
            "http://schemas.openxmlformats.org/package/2006/content-types"
        } else {
            continue;
        };
        let mut reader = quick_xml::NsReader::from_reader(bytes.as_slice());
        loop {
            let (namespace, event) = reader.read_resolved_event()?;
            match event {
                quick_xml::events::Event::Start(_) | quick_xml::events::Event::Empty(_) => {
                    assert!(
                        matches!(namespace, quick_xml::name::ResolveResult::Bound(ns) if ns.as_ref() == expected),
                        "unqualified entry in {path}"
                    );
                }
                quick_xml::events::Event::Eof => break,
                _ => {}
            }
        }
    }
    let read = call(dir.path(), json!({"op":"read","input":"edited.pptx"}))?;
    assert!(read.text.ok_or("no text")?.contains("Added note"));
    if let Some(destination) = std::env::var_os("SLIDES_TEST_ARTIFACTS") {
        let destination = std::path::PathBuf::from(destination);
        std::fs::create_dir_all(&destination)?;
        std::fs::copy(
            dir.path().join("edited.pptx"),
            destination.join("alias-opc-edited.pptx"),
        )?;
        std::fs::copy(
            dir.path().join("foreign.pptx"),
            destination.join("alias-opc-before.pptx"),
        )?;
    }
    call(
        dir.path(),
        json!({"op":"convert","input":"edited.pptx","output":"edited.pdf"}),
    )?;
    assert_eq!(parts(&dir.path().join("edited.pptx"))?, p);
    Ok(())
}

#[test]
fn qc_breaks_tabs_and_cell_paragraphs_are_token_boundaries() -> Result {
    let dir = tempfile::tempdir()?;
    call(
        dir.path(),
        json!({"op":"create","output":"text.pptx","slides":[{"title":"Metrics","bullets":["Revenue: 10\n20 customers"]},{"title":"Table","table":{"headers":["Metric","Value"],"rows":[["Count","10\n20"]]}}]}),
    )?;
    let mut p = parts(&dir.path().join("text.pptx"))?;
    // A real semantic break and tab in one paragraph, and separate paragraphs in a cell.
    assert!(std::str::from_utf8(&p["ppt/slides/slide1.xml"])?.contains("<a:br/>"));
    change(&mut p, "ppt/slides/slide1.xml", |s| {
        s.replace("<a:br/>", "<a:br/><a:tab/>")
    })?;
    change(&mut p, "ppt/slides/slide2.xml", |s| {
        s.replace("<a:br/>", "</a:p><a:p>")
    })?;
    call(
        dir.path(),
        json!({"op":"create","output":"model-source.pptx","slides":[{"title":"Model","chart":{"type":"column","labels":["Revenue","Customers"],"series":[{"name":"Value","values":[10,20]}]}}]}),
    )?;
    let model = parts(&dir.path().join("model-source.pptx"))?;
    std::fs::write(
        dir.path().join("model.xlsx"),
        model
            .iter()
            .find(|(p, _)| p.ends_with(".xlsx"))
            .ok_or("no workbook")?
            .1,
    )?;
    pack(&dir.path().join("text.pptx"), &p)?;
    let checked = call(
        dir.path(),
        json!({"op":"check","input":"text.pptx","model":"model.xlsx"}),
    )?;
    assert_eq!(checked.summary_params["numbers"], "4");
    assert_eq!(checked.data, Some(json!([])));
    Ok(())
}

#[test]
fn qc_reassembles_paginated_logical_tables_and_still_finds_bad_totals() -> Result {
    let dir = tempfile::tempdir()?;
    for (name, total, expected_bad) in [("good", 210, false), ("bad", 211, true)] {
        let mut rows: Vec<Value> = (1..=20)
            .map(|i| {
                json!([
                    format!("Region {}", char::from(b'A' + i - 1)),
                    i.to_string()
                ])
            })
            .collect();
        rows.push(json!(["Total", total.to_string()]));
        let percentages: Vec<Value> = (1..=20)
            .map(|i| json!([format!("Region {}", char::from(b'A' + i - 1)), "5%"]))
            .collect();
        call(
            dir.path(),
            json!({"op":"create","output":format!("{name}.pptx"),"slides":[{"title":"Revenue","table":{"headers":["Region","Revenue"],"rows":rows}},{"title":"Shares","table":{"headers":["Region","Share"],"rows":percentages}}]}),
        )?;
        let read = call(
            dir.path(),
            json!({"op":"read","input":format!("{name}.pptx")}),
        )?;
        assert!(
            read.data
                .ok_or("no slides")?
                .as_array()
                .ok_or("not array")?
                .len()
                > 2
        );
        let checked = call(
            dir.path(),
            json!({"op":"check","input":format!("{name}.pptx")}),
        )?;
        let findings = checked.data.ok_or("no findings")?;
        let totals: Vec<_> = findings
            .as_array()
            .ok_or("not array")?
            .iter()
            .filter(|f| f["issue"] == "check.table_total")
            .collect();
        assert_eq!(totals.len(), usize::from(expected_bad), "{findings}");
        if expected_bad {
            assert_eq!(totals[0]["expected"], 210.0);
        }
        assert!(
            !findings
                .as_array()
                .ok_or("not array")?
                .iter()
                .any(|f| f["issue"] == "check.table_percent"),
            "{findings}"
        );
    }
    Ok(())
}

#[test]
fn logical_tables_stay_distinct_across_repeated_and_parallel_additions() -> Result {
    let dir = tempfile::tempdir()?;
    let mut rows: Vec<_> = (1..=20)
        .map(|i| {
            json!([
                format!("Region {}", char::from(b'A' + i - 1)),
                i.to_string()
            ])
        })
        .collect();
    rows.push(json!(["Total", "210"]));
    let slide = json!({"title":"Revenue","table":{"headers":["Region","Revenue"],"rows":rows}});
    call(
        dir.path(),
        json!({"op":"create","output":"tables.pptx","slides":[slide.clone()]}),
    )?;
    let edit = json!({"op":"edit","input":"tables.pptx","output":"tables.pptx","add":[slide]});
    for _ in 0..2 {
        call(dir.path(), edit.clone())?;
    }
    std::thread::scope(|scope| -> Result {
        let mut jobs = Vec::new();
        for _ in 0..2 {
            let edit = edit.clone();
            let base = dir.path();
            jobs.push(scope.spawn(move || {
                let input = serde_json::from_value::<Input>(edit)
                    .map_err(|source| slides::Error::CheckJson { source })?;
                run(&input, &Options::new(base))
            }));
        }
        for job in jobs {
            job.join().map_err(|_| "edit panicked")??;
        }
        Ok(())
    })?;
    let checked = call(dir.path(), json!({"op":"check","input":"tables.pptx"}))?;
    assert_eq!(checked.data, Some(json!([])));
    let p = parts(&dir.path().join("tables.pptx"))?;
    let mut identities = BTreeMap::<String, usize>::new();
    for (path, bytes) in &p {
        if !path.contains("/slides/") || !path.ends_with(".xml") {
            continue;
        }
        let mut reader = quick_xml::Reader::from_reader(bytes.as_slice());
        loop {
            match reader.read_event()? {
                quick_xml::events::Event::Empty(e) if e.name().as_ref() == "sl:table" => {
                    for attr in e.attributes() {
                        let attr = attr?;
                        if attr.key.as_ref() == "id" {
                            *identities.entry(attr.value.into_owned()).or_default() += 1;
                        }
                    }
                }
                quick_xml::events::Event::Eof => break,
                _ => {}
            }
        }
    }
    assert_eq!(identities.len(), 5);
    assert!(identities.values().all(|count| *count == 4));
    Ok(())
}

#[test]
fn invalid_logical_table_offsets_fail_before_writing() -> Result {
    let dir = tempfile::tempdir()?;
    call(
        dir.path(),
        json!({"op":"create","output":"base.pptx","slides":[{"title":"Table","table":{"headers":["Region","Revenue"],"rows":[["North","10"]]}}]}),
    )?;
    let original = parts(&dir.path().join("base.pptx"))?;
    for offset in ["-1", "not-an-offset", "99999999999999999999999999999999"] {
        let mut p = original.clone();
        change(&mut p, "ppt/slides/slide1.xml", |s| {
            s.replace("start=\"0\"", &format!("start=\"{offset}\""))
        })?;
        pack(&dir.path().join("invalid.pptx"), &p)?;
        std::fs::write(dir.path().join("previous.pptx"), b"previous output")?;
        for request in [
            json!({"op":"read","input":"invalid.pptx"}),
            json!({"op":"check","input":"invalid.pptx"}),
            json!({"op":"convert","input":"invalid.pptx","output":"previous.pptx"}),
        ] {
            let error = run(
                &serde_json::from_value::<Input>(request)?,
                &Options::new(dir.path()),
            )
            .err()
            .ok_or("accepted invalid row offset")?;
            assert_eq!(error.code(), "xml.invalid");
            assert!(std::error::Error::source(&error).is_some());
        }
        assert_eq!(
            std::fs::read(dir.path().join("previous.pptx"))?,
            b"previous output"
        );
    }
    Ok(())
}

#[test]
fn edits_report_every_missing_target_with_index_and_field() -> Result {
    let dir = tempfile::tempdir()?;
    call(
        dir.path(),
        json!({"op":"create","output":"foreign.pptx","markdown":"# Northstar Analytics"}),
    )?;
    let mut p = parts(&dir.path().join("foreign.pptx"))?;
    change(&mut p, "ppt/slides/slide1.xml", |s| {
        s.replace("<p:ph type=\"ctrTitle\"/>", "")
            .replace("<p:ph type=\"title\"/>", "")
    })?;
    pack(&dir.path().join("foreign.pptx"), &p)?;
    let edited = call(
        dir.path(),
        json!({"op":"edit","input":"foreign.pptx","output":"edited.pptx","slides":[{"index":0,"title":"Clearview Analytics","bullets":["Absent"]},{"index":90,"title":"Missing"}],"replace":{"find":"unmatched","replace":"unused"},"delete":[91]}),
    )?;
    for field in ["title", "bullets"] {
        assert!(
            edited
                .warnings
                .iter()
                .any(|w| w.code == "edit.field_missing"
                    && w.params["index"] == "0"
                    && w.params["field"] == field),
            "{:?}",
            edited.warnings
        );
    }
    assert!(edited.warnings.iter().any(|w| w.code == "edit.no_match"));
    assert_eq!(
        edited
            .warnings
            .iter()
            .filter(|w| w.code == "edit.slide_missing")
            .count(),
        2
    );
    assert!(
        call(dir.path(), json!({"op":"read","input":"edited.pptx"}))?
            .text
            .ok_or("no text")?
            .contains("Northstar Analytics")
    );
    Ok(())
}

#[test]
fn qualified_attributes_do_not_alias_unqualified_placeholder_fields() -> Result {
    let dir = tempfile::tempdir()?;
    call(
        dir.path(),
        json!({"op":"create","output":"base.pptx","markdown":"# Northstar Analytics"}),
    )?;
    let mut p = parts(&dir.path().join("base.pptx"))?;
    change(&mut p, "ppt/slides/slide1.xml", |s| {
        s.replace("<p:ph type=\"ctrTitle\"/>", "<p:ph xmlns:z=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" z:type=\"ctrTitle\"/>")
    })?;
    pack(&dir.path().join("foreign.pptx"), &p)?;
    let edited = call(
        dir.path(),
        json!({"op":"edit","input":"foreign.pptx","output":"edited.pptx","slides":[{"index":0,"title":"Clearview Analytics"}]}),
    )?;
    assert!(
        edited
            .warnings
            .iter()
            .any(|w| w.code == "edit.field_missing" && w.params["field"] == "title")
    );
    assert!(
        call(dir.path(), json!({"op":"read","input":"edited.pptx"}))?
            .text
            .ok_or("no text")?
            .contains("Northstar Analytics")
    );
    Ok(())
}

#[test]
fn company_chart_style_is_independent_of_requested_theme() -> Result {
    let dir = tempfile::tempdir()?;
    call(
        dir.path(),
        json!({"op":"create","output":"company.pptx","theme":"creative","markdown":"# Orion"}),
    )?;
    let mut styles = Vec::new();
    for theme in ["light", "dark"] {
        let slides: Vec<_> = ["bar", "column", "line", "pie", "area", "scatter"].iter().map(|kind| json!({"title":format!("Revenue {kind}"),"chart":{"type":kind,"labels":if *kind == "scatter" { vec!["1", "2"] } else { vec!["Q1", "Q2"] },"series":[{"name":"Revenue","values":[10,20]}]}})).collect();
        call(
            dir.path(),
            json!({"op":"create","output":format!("{theme}.pptx"),"theme":theme,"template":"company.pptx","slides":slides}),
        )?;
        let p = parts(&dir.path().join(format!("{theme}.pptx")))?;
        let charts: Vec<_> = p
            .iter()
            .filter(|(p, _)| p.contains("/charts/chart") && p.ends_with(".xml"))
            .map(|(_, bytes)| bytes.clone())
            .collect();
        assert_eq!(charts.len(), 6);
        styles.push(charts);
    }
    assert_eq!(styles[0], styles[1]);
    for bytes in &styles[0] {
        let chart = std::str::from_utf8(bytes)?;
        assert!(chart.contains("schemeClr"));
        assert!(!chart.contains("srgbClr"));
        assert!(chart.contains("+mn-lt"));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
#[test]
fn atomic_output_stays_in_pinned_directory_during_long_write() -> Result {
    use std::os::unix::fs::symlink;
    use std::sync::atomic::{AtomicBool, Ordering};
    let root = tempfile::tempdir()?;
    let base = root.path().join("base");
    let output = base.join("output");
    let outside = root.path().join("outside");
    std::fs::create_dir_all(&output)?;
    std::fs::create_dir_all(&outside)?;
    std::fs::write(output.join("result.md"), "old output")?;
    std::fs::write(outside.join("result.md"), "outside sentinel")?;
    // Use a supported conversion pair; the uncompressed fixture stays within the
    // input/inflated limits while producing a long atomic Markdown write.
    call(
        &base,
        json!({"op":"create","output":"source.pptx","markdown":"# Long write\n\nLONG_CONTENT"}),
    )?;
    let mut source = parts(&base.join("source.pptx"))?;
    change(&mut source, "ppt/slides/slide1.xml", |xml| {
        xml.replace("LONG_CONTENT", &"bounded output ".repeat(4_000_000))
    })?;
    pack(&base.join("source.pptx"), &source)?;
    let done = AtomicBool::new(false);
    std::thread::scope(|scope| -> Result {
        let worker = scope.spawn(|| {
            let result = call(
                &base,
                json!({"op":"convert","input":"source.pptx","output":"output/result.md"}),
            )
            .map(|o| o.ok)
            .map_err(|e| e.to_string());
            done.store(true, Ordering::Release);
            result
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        let mut observed = false;
        while !done.load(Ordering::Acquire) && std::time::Instant::now() < deadline {
            let writing = std::fs::read_dir(&output)?
                .filter_map(std::result::Result::ok)
                .any(|e| e.file_name() != "result.md" && e.metadata().is_ok_and(|m| m.len() > 0));
            if writing {
                // The temporary already has content, while the previous file is still intact.
                assert_eq!(
                    std::fs::read_to_string(output.join("result.md"))?,
                    "old output"
                );
                std::fs::rename(&output, base.join("pinned"))?;
                symlink(&outside, &output)?;
                observed = true;
                break;
            }
            std::thread::yield_now();
        }
        let result = worker.join().map_err(|_| "writer panicked")?;
        assert!(observed, "did not observe an in-progress write");
        assert!(result.map_err(|e| format!("pinned write failed: {e}"))?);
        Ok(())
    })?;
    assert_eq!(
        std::fs::read_to_string(outside.join("result.md"))?,
        "outside sentinel"
    );
    let result = std::fs::read_to_string(base.join("pinned/result.md"))?;
    assert!(result.starts_with("# Long write"));
    assert_eq!(result.matches("bounded output").count(), 4_000_000);
    assert_eq!(std::fs::read_dir(base.join("pinned"))?.count(), 1);
    Ok(())
}

#[test]
fn library_errors_serialize_to_the_same_envelope_as_cli() -> Result {
    let error = slides::parse_input(r#"{"op":"read"}"#)
        .err()
        .ok_or("accepted missing input")?;
    let value = serde_json::to_value(&error)?;
    assert_eq!(value, serde_json::to_value(error.output())?);
    assert_eq!(value["summary_params"]["field"], "/input");
    assert_eq!(value["ok"], false);
    assert_eq!(value.as_object().ok_or("not object")?.len(), 5);
    assert!(std::error::Error::source(&error).is_some());
    Ok(())
}

#[test]
fn cli_error_envelope_and_json_locations_are_structured() -> Result {
    let dir = tempfile::tempdir()?;
    for (request, field, syntax) in [
        ("garbage", None, true),
        ("{", None, true),
        (r#"{"op":"read"}"#, Some("/input"), false),
        (
            r#"{"op":"create","output":"x.pptx","slides":[{"table":{"headers":[]}}]}"#,
            Some("/slides/0/table/rows"),
            false,
        ),
    ] {
        let out = Command::new(env!("CARGO_BIN_EXE_slides"))
            .current_dir(dir.path())
            .arg(request)
            .output()?;
        assert_eq!(out.status.code(), Some(1));
        assert!(out.stderr.is_empty());
        let value: Value = serde_json::from_slice(&out.stdout)?;
        let mut keys: Vec<_> = value
            .as_object()
            .ok_or("not object")?
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "ok",
                "summary",
                "summary_code",
                "summary_params",
                "warnings"
            ]
        );
        assert_eq!(value["ok"], false);
        assert_eq!(value["summary_code"], "cli.invalid_json");
        assert_eq!(value["warnings"], json!([]));
        if let Some(field) = field {
            assert_eq!(value["summary_params"]["field"], field);
        }
        if syntax {
            assert!(value["summary_params"]["line"].as_str().is_some());
            assert!(value["summary_params"]["column"].as_str().is_some());
        }
    }
    Ok(())
}
