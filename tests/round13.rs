//! Cross-representation facts, combination charts, portable Markdown and JSON locations.
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
    let mut out = Parts::new();
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
fn chart(metric: &str, category: &str, value: i64) -> Value {
    json!({"title":"Chart","chart":{"type":"column","labels":[category],"series":[{"name":metric,"values":[value]}]}})
}
fn check_pair(dir: &Path, first: Value, second: Value) -> Result<Output> {
    call(
        dir,
        json!({"op":"create","output":"facts.pptx","slides":[first,second]}),
    )?;
    call(dir, json!({"op":"check","input":"facts.pptx"}))
}
fn mismatches(out: &Output) -> Result<usize> {
    Ok(out
        .data
        .as_ref()
        .and_then(Value::as_array)
        .ok_or("no findings")?
        .iter()
        .filter(|v| v["issue"] == "check.metric_mismatch")
        .count())
}
#[test]
fn numeric_first_column_is_a_measure_with_header_units() -> Result {
    let dir = tempfile::tempdir()?;
    let make = |n| json!({"title":"Results","table":{"headers":["Revenue ($m)","Cost ($m)"],"rows":[[n,"10"]]}});
    let out = check_pair(dir.path(), make("25"), make("26"))?;
    assert_eq!(out.summary_params["numbers"], "4");
    assert_eq!(mismatches(&out)?, 1);
    assert_eq!(
        out.data.as_ref().ok_or("no findings")?[0]["expected"],
        25_000_000.0
    );
    Ok(())
}
#[test]
fn explicit_text_table_and_chart_metrics_share_identity() -> Result {
    let dir = tempfile::tempdir()?;
    for (first, second) in [
        (
            json!({"text":"Revenue: 25"}),
            json!({"table":{"headers":["Metric","Value"],"rows":[["Revenue","26"]]}}),
        ),
        (
            json!({"text":"Revenue Q1 2027: 25"}),
            json!({"table":{"headers":["Metric","Q1 2027"],"rows":[["Revenue","26"]]}}),
        ),
        (
            json!({"text":"Revenue North: 25"}),
            chart("Revenue", "North", 26),
        ),
        (
            json!({"table":{"headers":["Region","Revenue"],"rows":[["North","25"]]}}),
            chart("Revenue", "North", 26),
        ),
        (
            json!({"text":"Enterprise Value: 25"}),
            json!({"table":{"headers":["Metric","Value"],"rows":[["Enterprise Value","26"]]}}),
        ),
        (
            json!({"text":"Revenue 2027: 25"}),
            json!({"table":{"headers":["Metric","Value"],"rows":[["Revenue 2027","26"]]}}),
        ),
        (
            json!({"text":"Net Revenue Q1 2027: $25 mln"}),
            json!({"table":{"headers":["Metric","Q1 2027 ($m)"],"rows":[["Net Revenue","26"]]}}),
        ),
        (
            json!({"text":"Net Revenue Q1 2027: $25 mln"}),
            chart("Net Revenue ($m)", "Q1 2027", 26),
        ),
    ] {
        let out = check_pair(dir.path(), first.clone(), second.clone())?;
        assert_eq!(mismatches(&out)?, 1, "{first} / {second}: {out:?}");
    }
    for (a, b) in [
        ("Revenue 2027: $25 mln", "Revenue 2028: $26 mln"),
        ("Revenue: $25 mln", "Revenue: €26 mln"),
        ("Revenue North: 25", "Revenue South: 26"),
        ("Enterprise Value: 25", "Enterprise: 26"),
    ] {
        let out = check_pair(dir.path(), json!({"text":a}), json!({"text":b}))?;
        assert_eq!(mismatches(&out)?, 0, "{a} / {b}");
    }
    Ok(())
}
#[test]
fn uncertain_numeric_tables_warn_instead_of_disappearing() -> Result {
    let dir = tempfile::tempdir()?;
    call(
        dir.path(),
        json!({"op":"create","output":"facts.pptx","slides":[{"table":{"headers":["Value","Amount"],"rows":[["25","10"],["26","11"]]}}]}),
    )?;
    let out = call(dir.path(), json!({"op":"check","input":"facts.pptx"}))?;
    assert_eq!(out.summary_params["numbers"], "4");
    let warning = out
        .warnings
        .iter()
        .find(|w| w.code == "check.unclassified_numbers")
        .ok_or("no unclassified warning")?;
    assert_eq!(warning.params["slide"], "1");
    assert_eq!(warning.params["count"], "4");
    assert!(warning.message.contains("unclassified numbers"));
    assert_eq!(out.data, Some(json!([])));
    Ok(())
}
fn combination(dir: &Path) -> Result {
    call(
        dir,
        json!({"op":"create","output":"combo.pptx","slides":[
            {"title":"Combination","chart":{"type":"column","labels":["North","South"],"series":[{"name":"Revenue","values":[10,20]},{"name":"Margin","values":[30,40]}]}},
            {"title":"Control","chart":{"type":"line","labels":["North","South"],"series":[{"name":"Margin","values":[31,40]}]}}
        ]}),
    )?;
    let mut p = parts(&dir.join("combo.pptx"))?;
    let name = "ppt/charts/chart1.xml";
    let mut xml = String::from_utf8(p.get(name).ok_or("missing chart")?.clone())?;
    let start = xml.find("<c:ser><c:idx val=\"1\"").ok_or("second series")?;
    let end = start + xml[start..].find("</c:ser>").ok_or("series end")? + "</c:ser>".len();
    let line = format!(
        "<c:lineChart><c:grouping val=\"standard\"/>{}<c:axId val=\"1\"/><c:axId val=\"2\"/></c:lineChart>",
        &xml[start..end]
    );
    xml.replace_range(start..end, "");
    xml = xml.replace("</c:barChart>", &format!("</c:barChart>{line}"));
    p.insert(name.into(), xml.into_bytes());
    pack(&dir.join("combo.pptx"), &p)
}
#[test]
fn combination_charts_read_export_and_check_every_group() -> Result {
    let dir = tempfile::tempdir()?;
    combination(dir.path())?;
    let read = call(dir.path(), json!({"op":"read","input":"combo.pptx"}))?;
    let data = read.data.ok_or("no data")?;
    assert!(
        data[0]["text"]
            .as_str()
            .ok_or("no text")?
            .contains("Margin"),
        "{data}"
    );
    call(
        dir.path(),
        json!({"op":"convert","input":"combo.pptx","output":"combo.md"}),
    )?;
    let md = std::fs::read_to_string(dir.path().join("combo.md"))?;
    assert!(md.contains("Margin | 30 | 40"), "{md}");
    let out = call(dir.path(), json!({"op":"check","input":"combo.pptx"}))?;
    assert_eq!(out.summary_params["numbers"], "6");
    assert_eq!(mismatches(&out)?, 1);
    Ok(())
}
// A fixed local PRNG keeps the generator reproducible without a new dependency.
#[test]
fn generated_differential_facts_across_200_decks() -> Result {
    const SEED: u64 = 0x13_cafe_2027;
    eprintln!("differential seed={SEED:#x}; decks=200");
    let mut state = SEED;
    let mut random = || {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        state >> 32
    };
    let dir = tempfile::tempdir()?;
    for case in 0..200 {
        let value = 1 + (random() % 5000) as i64;
        let conflict = random() % 2 == 0;
        let period = format!("Q{} 2027", 1 + random() % 4);
        let metric = ["Revenue", "Cost", "Headcount"][random() as usize % 3];
        let make = |kind, n: i64| match kind {
            0 => json!({"title":"Text","text":format!("{metric} {period}: {n}")}),
            1 => {
                json!({"title":"Rows","table":{"headers":["Metric",period.clone()],"rows":[[metric,n.to_string()]]}})
            }
            2 => {
                json!({"title":"Columns","table":{"headers":["Period",metric],"rows":[[period.clone(),n.to_string()]]}})
            }
            _ => chart(metric, &period, n),
        };
        let first = make(random() % 4, value);
        let second = make(random() % 4, value + i64::from(conflict));
        let out = check_pair(dir.path(), first.clone(), second.clone())?;
        assert_eq!(
            mismatches(&out)?,
            usize::from(conflict),
            "seed={SEED:#x} case={case}: {first} / {second}; {out:?}"
        );
        assert_eq!(
            out.data
                .as_ref()
                .and_then(Value::as_array)
                .ok_or("no findings")?
                .len(),
            usize::from(conflict)
        );
    }
    Ok(())
}
fn png() -> Result<Vec<u8>> {
    let mut image = printpdf::RawImage::empty(12, 8, printpdf::RawImageFormat::RGB8);
    image.pixels = printpdf::RawImageData::U8([220, 40, 90].repeat(12 * 8));
    image
        .encode_to_bytes(&[printpdf::OutputImageFormat::Png])
        .map(|v| v.0)
        .map_err(Into::into)
}
#[test]
fn markdown_media_are_relative_portable_and_roundtrip_bytes() -> Result {
    let dir = tempfile::tempdir()?;
    let image = png()?;
    std::fs::write(dir.path().join("source.png"), &image)?;
    call(
        dir.path(),
        json!({"op":"create","output":"images.pptx","slides":[{"title":"Image","image":"source.png"},{"title":"Again","image":"source.png"}]}),
    )?;
    for stem in [
        "deck",
        "deck",
        "annual report",
        "results(1)",
        "café",
        "draft>100%",
    ] {
        let output = format!("nested/{stem}.md");
        let media = format!("{stem}_media");
        call(
            dir.path(),
            json!({"op":"convert","input":"images.pptx","output":output}),
        )?;
        let md = std::fs::read_to_string(dir.path().join(&output))?;
        let encoded_media = media.replace('%', "%25").replace('>', "%3E");
        assert!(md.contains(&format!("![](<{encoded_media}/")), "{md}");
        let mut count = 0;
        for link in md.split("![](<").skip(1) {
            let path = link.split(">)").next().ok_or("no image link")?;
            let path = path.replace("%3E", ">").replace("%25", "%");
            assert_eq!(std::fs::read(dir.path().join("nested").join(path))?, image);
            count += 1;
        }
        assert_eq!(count, 2);
        let portable = tempfile::tempdir()?;
        std::fs::create_dir(portable.path().join(&media))?;
        std::fs::copy(dir.path().join(&output), portable.path().join("deck.md"))?;
        for entry in std::fs::read_dir(dir.path().join("nested").join(&media))? {
            let entry = entry?;
            std::fs::copy(
                entry.path(),
                portable.path().join(&media).join(entry.file_name()),
            )?;
        }
        let out = call(
            portable.path(),
            json!({"op":"convert","input":"deck.md","output":"again.pptx"}),
        )?;
        assert!(
            !out.warnings.iter().any(|w| w.code.starts_with("image.")),
            "{out:?}"
        );
        let p = parts(&portable.path().join("again.pptx"))?;
        assert!(
            p.iter()
                .any(|(name, bytes)| name.contains("/media/") && bytes == &image)
        );
    }
    Ok(())
}
#[test]
fn parallel_markdown_exports_publish_complete_media_sets() -> Result {
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("source.png"), png()?)?;
    call(
        dir.path(),
        json!({"op":"create","output":"images.pptx","slides":[{"image":"source.png"}]}),
    )?;
    std::thread::scope(|scope| -> Result {
        let mut workers = Vec::new();
        for _ in 0..4 {
            workers.push(scope.spawn(|| -> std::result::Result<(), String> {
                call(
                    dir.path(),
                    json!({"op":"convert","input":"images.pptx","output":"parallel.md"}),
                )
                .map(|_| ())
                .map_err(|e| e.to_string())
            }));
        }
        for worker in workers {
            worker.join().map_err(|_| "export thread panicked")??;
        }
        Ok(())
    })?;
    let out = call(
        dir.path(),
        json!({"op":"convert","input":"parallel.md","output":"parallel.pptx"}),
    )?;
    assert!(!out.warnings.iter().any(|w| w.code.starts_with("image.")));
    let image = png()?;
    assert!(
        parts(&dir.path().join("parallel.pptx"))?
            .iter()
            .any(|(n, b)| n.contains("/media/") && b == &image)
    );
    Ok(())
}
#[cfg(unix)]
#[test]
fn markdown_media_reject_symlinks_and_preserve_previous_markdown() -> Result {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("source.png"), png()?)?;
    call(
        dir.path(),
        json!({"op":"create","output":"images.pptx","slides":[{"image":"source.png"}]}),
    )?;
    std::fs::write(dir.path().join("deck.md"), "previous")?;
    symlink("missing-target", dir.path().join("deck_media"))?;
    let error = call(
        dir.path(),
        json!({"op":"convert","input":"images.pptx","output":"deck.md"}),
    )
    .err()
    .ok_or("accepted media symlink")?;
    assert_eq!(
        error
            .downcast_ref::<slides::Error>()
            .ok_or("wrong error")?
            .code(),
        "path.symlink"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("deck.md"))?,
        "previous"
    );
    assert!(!dir.path().join("missing-target").exists());
    Ok(())
}
#[test]
fn decoded_markdown_links_still_enforce_the_path_boundary() -> Result {
    let dir = tempfile::tempdir()?;
    for link in [
        "../outside.png",
        "/outside.png",
        "%2e%2e%2foutside.png",
        "%2foutside.png",
    ] {
        std::fs::write(
            dir.path().join("source.md"),
            format!("## Image\n\n![](<{link}>)\n"),
        )?;
        std::fs::write(dir.path().join("previous.pptx"), "previous")?;
        let request = json!({"op":"convert","input":"source.md","output":"previous.pptx"});
        let error = call(dir.path(), request.clone())
            .err()
            .ok_or("accepted unsafe decoded link")?;
        assert_eq!(
            error
                .downcast_ref::<slides::Error>()
                .ok_or("wrong error")?
                .code(),
            "path.unsafe"
        );
        let result = Command::new(env!("CARGO_BIN_EXE_slides"))
            .current_dir(dir.path())
            .arg(request.to_string())
            .output()?;
        assert_eq!(result.status.code(), Some(1));
        let output: Value = serde_json::from_slice(&result.stdout)?;
        assert_eq!(output["summary_code"], "path.unsafe");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("previous.pptx"))?,
            "previous"
        );
    }
    Ok(())
}
#[test]
fn invalid_nested_json_reports_the_selected_source_and_pointer() -> Result {
    let dir = tempfile::tempdir()?;
    for (request, pointer) in [
        (
            json!({"op":"create","output":"x.pptx","slides":[{"chart":{"type":"line","labels":["Q1"],"series":[{"name":"Revenue","values":[1],"unknown":true}]}}]}),
            "/slides/0/chart/series/0/unknown",
        ),
        (
            json!({"op":"create","output":"x.pptx","slides":[{"chart":{"type":"line","labels":["Q1"],"series":[{"name":"Revenue","values":["bad"]}]}}]}),
            "/slides/0/chart/series/0/values/0",
        ),
        (
            json!({"op":"create","output":"x.pptx","markdown":42}),
            "/markdown",
        ),
        (
            json!({"op":"create","output":"x.pptx","slides":[{"chart":{"type":"line","labels":["Q1"],"series":["bad"]}}]}),
            "/slides/0/chart/series/0",
        ),
        (
            json!({"op":"create","output":"x.pptx","slides":[{"a/b~c":true}]}),
            "/slides/0/a~1b~0c",
        ),
        (
            json!({"op":"read","input":"x.pptx","representation":"invalid"}),
            "/representation",
        ),
        (json!({"op":"read","input":"x.pptx","offset":-1}), "/offset"),
        (json!({"op":"invalid"}), "/op"),
        (json!({}), "/op"),
    ] {
        let error = slides::parse_input(&request.to_string())
            .err()
            .ok_or("accepted invalid JSON")?;
        assert_eq!(error.params()["field"], pointer, "{error:?}");
        let result = Command::new(env!("CARGO_BIN_EXE_slides"))
            .current_dir(dir.path())
            .arg(request.to_string())
            .output()?;
        assert_eq!(result.status.code(), Some(1));
        let output: Value = serde_json::from_slice(&result.stdout)?;
        assert_eq!(output["summary_params"]["field"], pointer);
        assert_eq!(output["summary_code"], "cli.invalid_json");
        assert_eq!(output.as_object().ok_or("not object")?.len(), 5);
    }
    Ok(())
}
