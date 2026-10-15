//! Seeded generators for the three invariants, independent of the layout implementation.
use serde_json::{Value, json};
use slides::{Input, Options, run};
use std::io::{Cursor, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;
const SEED: u64 = 0x51_1DE5_2027;
fn next(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *state
}
fn call(base: &Path, request: Value) -> Result<Value> {
    Ok(serde_json::to_value(run(
        &serde_json::from_value::<Input>(request)?,
        &Options::new(base),
    )?)?)
}
fn xml_text(bytes: &[u8], tag: &str) -> Result<Vec<String>> {
    let mut reader = quick_xml::Reader::from_reader(bytes);
    let mut values = Vec::new();
    loop {
        match reader.read_event()? {
            quick_xml::events::Event::Start(e) if e.name().as_ref() == tag => values.push(
                reader
                    .read_text(e.name())?
                    .xml_content(quick_xml::XmlVersion::default())
                    .into_owned(),
            ),
            quick_xml::events::Event::Eof => break,
            _ => {}
        }
    }
    Ok(values)
}
fn numeric_case(base: &Path, state: &mut u64, case: usize) -> Result {
    let n = next(state) % 900 + 100;
    let year = 1900 + next(state) % 301;
    let literal = match case % 6 {
        0 => n.to_string(),
        1 => format!("{n}.125"),
        2 => format!("-{n}.75"),
        3 => format!("{n}.5%"),
        4 => format!("${n}.25"),
        _ => year.to_string(),
    };
    let series = vec![n as f64 + 0.125, -(n as f64) - 0.75, year as f64, 0.025];
    let request = json!({"op":"create","output":"numbers.pptx","slides":[
        {"title":format!("Review {literal}"),"bullets":[format!("Revenue {literal}"),format!("Year {year}")]},
        {"title":"Table","table":{"headers":["Metric","Value"],"rows":[["Reported",literal]]}},
        {"title":"Chart","chart":{"type":if case.is_multiple_of(2) {"bar"} else {"line"},"labels":["Alpha","Beta","Gamma","Delta"],"series":[{"name":"Measure","values":series}]}}
    ]});
    let output = call(base, request)?;
    assert!(output["ok"] == true);
    let mut zip = zip::ZipArchive::new(Cursor::new(std::fs::read(base.join("numbers.pptx"))?))?;
    let mut texts = Vec::new();
    let mut chart = None;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let name = entry.name()?.into_owned();
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes)?;
        if name.contains("/slides/") && name.ends_with(".xml") {
            texts.extend(xml_text(&bytes, "a:t")?);
        }
        if name == "ppt/charts/chart1.xml" {
            chart = Some(bytes);
        }
    }
    for expected in [
        format!("Review {literal}"),
        format!("Revenue {literal}"),
        format!("Year {year}"),
        literal.clone(),
    ] {
        assert!(
            texts.contains(&expected),
            "missing PPTX numeric text {expected}"
        );
    }
    let chart = chart.ok_or("chart missing")?;
    // Select the value cache rather than matching unrelated numeric category labels or indices.
    let chart = std::str::from_utf8(&chart)?;
    let values = chart
        .split("<c:val>")
        .nth(1)
        .ok_or("value cache")?
        .split("</c:val>")
        .next()
        .ok_or("value end")?;
    assert_eq!(
        xml_text(values.as_bytes(), "c:v")?
            .iter()
            .map(|v| v.parse::<f64>())
            .collect::<std::result::Result<Vec<_>, _>>()?,
        series
    );
    call(
        base,
        json!({"op":"convert","input":"numbers.pptx","output":"numbers.pdf"}),
    )?;
    let text = Command::new("pdftotext")
        .arg("-layout")
        .arg(base.join("numbers.pdf"))
        .arg("-")
        .output()?;
    assert!(text.status.success());
    let text = String::from_utf8(text.stdout)?;
    assert!(
        text.matches(&literal).count() >= 3,
        "PDF title, bullet or table literal changed: {literal}: {text}"
    );
    for number in series {
        let number = number.to_string();
        assert!(
            text.split_whitespace().any(|v| v == number),
            "PDF chart value changed: {number}: {text}"
        );
    }
    Ok(())
}
#[test]
fn i1_two_hundred_seeded_decks_preserve_numbers_in_pptx_caches_and_pdf() -> Result {
    eprintln!("I1 seed={SEED:#x}");
    let dir = tempfile::tempdir()?;
    let mut state = SEED;
    for case in 0..200 {
        let result = numeric_case(dir.path(), &mut state, case);
        assert!(result.is_ok(), "I1 seed={SEED:#x} case={case}: {result:?}");
    }
    Ok(())
}
#[test]
fn i2_seeded_requests_have_identical_bytes_and_json_on_repetition() -> Result {
    eprintln!("I2 seed={SEED:#x}");
    let dir = tempfile::tempdir()?;
    let mut state = SEED;
    for case in 0..8 {
        let number = next(&mut state) % 10000;
        for extension in ["pptx", "pdf"] {
            let request = json!({"op":"create","output":format!("same.{extension}"),"theme":(["light","dark","corporate","creative"][case%4]),"slides":[{"title":format!("Review {number}"),"chart":{"type":"line","labels":["A","B"],"series":[{"name":"Value","values":[number as f64,12.125]}]}}]});
            let a = call(dir.path(), request.clone())?;
            let bytes = std::fs::read(dir.path().join(format!("same.{extension}")))?;
            std::thread::sleep(std::time::Duration::from_millis(1050));
            let b = call(dir.path(), request)?;
            assert_eq!(a, b, "I2 JSON seed={SEED:#x} case={case} {extension}");
            assert_eq!(
                bytes,
                std::fs::read(dir.path().join(format!("same.{extension}")))?,
                "I2 bytes seed={SEED:#x} case={case} {extension}"
            );
            if extension == "pdf" {
                let check = Command::new("qpdf")
                    .arg("--check")
                    .arg(dir.path().join("same.pdf"))
                    .output()?;
                assert!(
                    check.status.success(),
                    "canonical PDF invalid: {}",
                    String::from_utf8_lossy(&check.stderr)
                );
            }
        }
        let request = json!({"op":"convert","input":"same.pptx","output":"converted.pdf"});
        let a = call(dir.path(), request.clone())?;
        let bytes = std::fs::read(dir.path().join("converted.pdf"))?;
        std::thread::sleep(std::time::Duration::from_millis(1050));
        assert_eq!(a, call(dir.path(), request)?);
        assert_eq!(
            bytes,
            std::fs::read(dir.path().join("converted.pdf"))?,
            "I2 conversion seed={SEED:#x} case={case}"
        );
    }
    Ok(())
}
fn cli(base: &Path, request: &[u8]) -> Result<Value> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_slides"))
        .current_dir(base)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    child.stdin.take().ok_or("stdin")?.write_all(request)?;
    let start = std::time::Instant::now();
    while child.try_wait()?.is_none() {
        if start.elapsed().as_secs() > 15 {
            child.kill()?;
            child.wait()?;
            return Err("CLI hung".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let output = child.wait_with_output()?;
    assert_eq!(output.status.code(), Some(1));
    assert!(
        output.stderr.is_empty(),
        "unexpected crash stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(value["ok"], false);
    assert!(value["summary_code"].is_string());
    assert!(value["summary_params"].is_object());
    assert_eq!(value["warnings"], json!([]));
    assert!(output.stdout.len() < 2048);
    Ok(value)
}
#[test]
fn i3_seeded_malformed_inputs_return_stable_json_errors_without_crashing() -> Result {
    eprintln!("I3 seed={SEED:#x}");
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("broken.pptx"), b"not a package")?;
    std::fs::write(dir.path().join("bad.md"), [0xff, 0xfe])?;
    let cases = vec![
        (
            json!({"op":"create","output":"out.pptx","slides":[]}),
            "input.no_slides",
        ),
        (
            json!({"op":"create","output":"out.pptx","slides":[{"image":"absent.png"}]}),
            "file.read",
        ),
        (
            json!({"op":"create","output":"out.pptx","template":"broken.pptx","slides":[{"title":"A"}]}),
            "zip.invalid",
        ),
        (
            json!({"op":"convert","input":"bad.md","output":"out.pptx"}),
            "input.markdown_not_utf8",
        ),
        (
            json!({"op":"create","output":"out.pptx","slides":[{"chart":{"type":"line","labels":["A"],"series":[{"name":"Extreme","values":vec![1;10001]}]}}]}),
            "chart.limit",
        ),
    ];
    for (case, code) in cases {
        let a = cli(dir.path(), case.to_string().as_bytes())?;
        let b = cli(dir.path(), case.to_string().as_bytes())?;
        assert_eq!(a, b, "I3 seed={SEED:#x}");
        assert_eq!(a["summary_code"], code);
    }
    let mut state = SEED;
    for case in 0..200 {
        let token = format!("unknown_{}", next(&mut state));
        let request = match case % 5 {
            0 => json!({"op":token}),
            1 => json!({"op":"create","output":"out.pptx","slides":[{token:1}]}),
            2 => json!({"op":"read","input":false}),
            3 => json!({"op":"edit","output":"out.pptx"}),
            _ => json!({"op":"create","output":"out.pptx","slides":[{"title":"bad\u{0}text"}]}),
        };
        let result = cli(dir.path(), request.to_string().as_bytes());
        assert!(result.is_ok(), "I3 seed={SEED:#x} case={case}: {result:?}");
    }
    for request in [b"{".as_slice(), b"garbage", &[0xff, 0xfe]] {
        cli(dir.path(), request)?;
    }
    let mut opts = Options::new(dir.path());
    opts.max_input_bytes = 1024;
    let huge = serde_json::from_value::<Input>(
        json!({"op":"create","output":"out.pptx","slides":[{"text":"large ".repeat(10000)}]}),
    )?;
    let error = run(&huge, &opts).err().ok_or("huge input accepted")?;
    assert_eq!(error.code(), "input.too_large");
    assert_eq!(serde_json::to_value(error)?["ok"], false);
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        let output = Command::new(env!("CARGO_BIN_EXE_slides"))
            .current_dir(dir.path())
            .arg(std::ffi::OsString::from_vec(vec![0xff]))
            .output()?;
        assert_eq!(output.status.code(), Some(1));
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stdout)?["summary_code"],
            "cli.argument_encoding"
        );
    }
    Ok(())
}
