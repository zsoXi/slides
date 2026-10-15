//! End-to-end tests: every operation goes through the public API and real files.

use std::io::Read;
use std::path::Path;

use serde_json::json;
use slides::{Input, Options, Output, run};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn call(dir: &Path, request: serde_json::Value) -> Result<Output, Box<dyn std::error::Error>> {
    let input: Input = serde_json::from_value(request)?;
    let options = Options::new(dir);
    Ok(run(&input, &options)?)
}

fn data_of(output: &Output) -> Result<&Vec<serde_json::Value>, Box<dyn std::error::Error>> {
    let data = output.data.as_ref().ok_or("read returned no data")?;
    data.as_array().ok_or_else(|| "data is not an array".into())
}

fn read_zip(
    path: &Path,
) -> Result<std::collections::HashMap<String, Vec<u8>>, Box<dyn std::error::Error>> {
    let file = std::fs::File::open(path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    let mut parts = std::collections::HashMap::new();
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        if entry.is_dir() {
            continue;
        }
        let mut data = Vec::new();
        entry.read_to_end(&mut data)?;
        parts.insert(entry.name()?.to_string(), data);
    }
    Ok(parts)
}

/// Test 1: Markdown with a list, a table and notes round-trips through PPTX and `read`.
#[test]
fn markdown_roundtrip_preserves_titles_bullets_and_notes() -> TestResult {
    let dir = tempfile::tempdir()?;
    let markdown = "# Tytul\n\n\
        ## Jeden\n- a\n- b\n\n\
        > n1\n\n\
        ## Dwa\n- c\n\n\
        ## Tabela\n| K | V |\n| --- | --- |\n| 1 | 2 |\n\n\
        ## Ostatni\n- z\n";
    call(
        dir.path(),
        json!({"op": "create", "output": "out.pptx", "markdown": markdown}),
    )?;
    let read = call(dir.path(), json!({"op": "read", "input": "out.pptx"}))?;
    assert!(read.summary.contains('5'), "summary: {}", read.summary);
    let slides = data_of(&read)?;
    assert_eq!(slides.len(), 5);
    assert_eq!(slides[0]["title"], "Tytul");
    assert_eq!(slides[1]["title"], "Jeden");
    assert_eq!(slides[1]["text"], "a\nb");
    assert_eq!(slides[1]["notes"], "n1");
    assert_eq!(slides[3]["text"], "K\tV\n1\t2");
    assert_eq!(slides[4]["title"], "Ostatni");
    Ok(())
}

/// Test 2: `edit` replaces text, deletes a slide and adds slides; `read` sees every change.
#[test]
fn edit_replace_delete_and_add() -> TestResult {
    let dir = tempfile::tempdir()?;
    let markdown = "# Tytul\n\n## Jeden\n- a\n- b\n\n> n1\n\n## Dwa\n- c\n\n## Trzy\n- d\n";
    call(
        dir.path(),
        json!({"op": "create", "output": "in.pptx", "markdown": markdown}),
    )?;
    call(
        dir.path(),
        json!({
            "op": "edit",
            "input": "in.pptx",
            "output": "out.pptx",
            "replace": {"find": "a", "replace": "AAA"},
            "delete": [2],
            "add": [{"layout": "section", "title": "Nowy"}]
        }),
    )?;
    let read = call(dir.path(), json!({"op": "read", "input": "out.pptx"}))?;
    let slides = data_of(&read)?;
    assert_eq!(slides.len(), 4);
    assert_eq!(slides[1]["text"], "AAA\nb");
    assert_eq!(slides[2]["title"], "Trzy");
    assert_eq!(slides[3]["title"], "Nowy");
    Ok(())
}

/// Test 2b: `slides[]` edits a title, body and notes in place.
#[test]
fn edit_slide_fields() -> TestResult {
    let dir = tempfile::tempdir()?;
    call(
        dir.path(),
        json!({"op": "create", "output": "in.pptx", "markdown": "# T\n\n## Jeden\n- a\n"}),
    )?;
    call(
        dir.path(),
        json!({
            "op": "edit",
            "input": "in.pptx",
            "output": "out.pptx",
            "slides": [{"index": 1, "title": "Zmieniony", "bullets": ["nowy", "drugi"], "notes": "notka"}]
        }),
    )?;
    let read = call(dir.path(), json!({"op": "read", "input": "out.pptx"}))?;
    let slides = data_of(&read)?;
    assert_eq!(slides[1]["title"], "Zmieniony");
    assert_eq!(slides[1]["text"], "nowy\ndrugi");
    assert_eq!(slides[1]["notes"], "notka");
    Ok(())
}

/// Test 3: PPTX -> PDF writes one page per slide.
#[test]
fn pptx_to_pdf_has_one_page_per_slide() -> TestResult {
    let dir = tempfile::tempdir()?;
    let markdown = "# T\n\n## A\n- x\n\n## B\n- y\n\n## C\n- z\n";
    call(
        dir.path(),
        json!({"op": "create", "output": "out.pptx", "markdown": markdown}),
    )?;
    call(
        dir.path(),
        json!({"op": "convert", "input": "out.pptx", "output": "out.pdf"}),
    )?;
    let pdf = std::fs::read(dir.path().join("out.pdf"))?;
    let pages = count_pdf_pages(&pdf);
    assert_eq!(pages, 4, "expected 4 PDF pages for 4 slides");
    Ok(())
}

fn count_pdf_pages(pdf: &[u8]) -> usize {
    fn count(haystack: &[u8], needle: &[u8]) -> usize {
        haystack
            .windows(needle.len())
            .filter(|w| *w == needle)
            .count()
    }
    // `/Type/Pages` contains `/Type/Page` as a prefix, so subtract the tree node.
    count(pdf, b"/Type/Page") - count(pdf, b"/Type/Pages")
}

/// Test 4: the PPTX is a valid OOXML package: required parts and parseable XML.
#[test]
fn pptx_package_structure_is_valid() -> TestResult {
    let dir = tempfile::tempdir()?;
    call(
        dir.path(),
        json!({"op": "create", "output": "out.pptx", "markdown": "# T\n\n## A\n- x\n\n> note\n"}),
    )?;
    let parts = read_zip(&dir.path().join("out.pptx"))?;
    for required in [
        "[Content_Types].xml",
        "_rels/.rels",
        "ppt/presentation.xml",
        "ppt/_rels/presentation.xml.rels",
        "ppt/slideMasters/slideMaster1.xml",
        "ppt/slideLayouts/slideLayout1.xml",
        "ppt/theme/theme1.xml",
        "ppt/slides/slide1.xml",
    ] {
        assert!(parts.contains_key(required), "missing part {required}");
        let bytes = parts.get(required).ok_or("part vanished")?;
        parse_xml(bytes)?;
    }
    let content_types = parts.get("[Content_Types].xml").ok_or("no content types")?;
    let text = String::from_utf8_lossy(content_types);
    assert!(text.contains("presentationml.presentation.main+xml"));
    assert!(text.contains("presentationml.slide+xml"));
    Ok(())
}

fn parse_xml(bytes: &[u8]) -> TestResult {
    let mut reader = quick_xml::Reader::from_reader(bytes);
    let mut buffer = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(quick_xml::events::Event::Eof) => break,
            Ok(_) => {}
            Err(error) => return Err(format!("XML error: {error}").into()),
        }
        buffer.clear();
    }
    Ok(())
}

/// Test 5: Polish characters survive in titles and notes.
#[test]
fn polish_characters_are_preserved() -> TestResult {
    let dir = tempfile::tempdir()?;
    call(
        dir.path(),
        json!({"op": "create", "output": "pl.pptx", "markdown": "# Zażółć gęślą jaźń\n\n> Źdźbło żółcią\n"}),
    )?;
    let read = call(dir.path(), json!({"op": "read", "input": "pl.pptx"}))?;
    let slides = data_of(&read)?;
    assert_eq!(slides[0]["title"], "Zażółć gęślą jaźń");
    assert_eq!(slides[0]["notes"], "Źdźbło żółcią");
    Ok(())
}

/// Direct JSON `slides` input with every layout and every theme.
#[test]
fn json_slides_with_layouts_and_themes() -> TestResult {
    let dir = tempfile::tempdir()?;
    let slides: Vec<serde_json::Value> = [
        "title",
        "title_content",
        "two_content",
        "section",
        "image",
        "table",
    ]
    .iter()
    .enumerate()
    .map(|(i, layout)| {
        json!({
            "layout": layout,
            "title": format!("L{i}"),
            "bullets": ["jeden", "dwa"],
            "table": {"headers": ["A", "B"], "rows": [["1", "2"]]}
        })
    })
    .collect();
    for theme in ["light", "dark", "corporate"] {
        let output = format!("{theme}.pptx");
        call(
            dir.path(),
            json!({"op": "create", "output": output, "slides": slides, "theme": theme}),
        )?;
        let read = call(dir.path(), json!({"op": "read", "input": output}))?;
        let parsed = data_of(&read)?;
        assert_eq!(parsed.len(), 6);
    }
    Ok(())
}

/// Negative: unsafe and absolute paths are rejected.
#[test]
fn unsafe_paths_are_rejected() -> TestResult {
    let dir = tempfile::tempdir()?;
    let parent = call(
        dir.path(),
        json!({"op": "create", "output": "../evil.pptx", "markdown": "# T\n"}),
    );
    assert!(parent.is_err(), "`../` must be rejected");
    let absolute = call(
        dir.path(),
        json!({"op": "create", "output": "/tmp/evil.pptx", "markdown": "# T\n"}),
    );
    assert!(absolute.is_err(), "absolute paths must be rejected");
    let read_parent = call(dir.path(), json!({"op": "read", "input": "../x.pptx"}));
    assert!(read_parent.is_err(), "reading `../x.pptx` must be rejected");
    Ok(())
}

/// Negative: unknown JSON fields are rejected by the `deny_unknown_fields` enum.
#[test]
fn unknown_fields_are_rejected() -> TestResult {
    let parsed: Result<Input, _> =
        serde_json::from_str(r#"{"op":"read","input":"x.pptx","unexpected":true}"#);
    assert!(parsed.is_err(), "unknown field must be rejected");
    let bad_op: Result<Input, _> = serde_json::from_str(r#"{"op":"frobnicate"}"#);
    assert!(bad_op.is_err(), "unknown op must be rejected");
    Ok(())
}

/// Negative: an input over `max_input_bytes` fails with a size error.
#[test]
fn oversized_input_is_rejected() -> TestResult {
    let dir = tempfile::tempdir()?;
    call(
        dir.path(),
        json!({"op": "create", "output": "small.pptx", "markdown": "# T\n"}),
    )?;
    let path = dir.path().join("small.pptx");
    let size = std::fs::metadata(&path)?.len();
    let input: Input = serde_json::from_value(json!({"op": "read", "input": "small.pptx"}))?;
    let mut options = Options::new(dir.path());
    options.max_input_bytes = size - 1;
    match run(&input, &options) {
        Err(slides::Error::TooLarge { .. }) => Ok(()),
        other => Err(format!(
            "expected TooLarge, got {:?}",
            other.map(|output| output.summary)
        )
        .into()),
    }
}

/// Long text shrinks the font to the 14 pt minimum and then warns about the slide.
#[test]
fn long_text_reports_a_warning() -> TestResult {
    let dir = tempfile::tempdir()?;
    let long = "wyraz ".repeat(4000);
    let output = call(
        dir.path(),
        json!({
            "op": "create",
            "output": "long.pptx",
            "slides": [{"layout": "title_content", "title": "Dlugie", "bullets": [long]}]
        }),
    )?;
    assert!(
        output
            .warnings
            .iter()
            .any(|w| w.message.contains("does not fit")),
        "expected a fit warning, got {:?}",
        output.warnings
    );
    Ok(())
}

/// The schema is a JSON object and the operation names are present.
#[test]
fn schema_lists_operations() -> TestResult {
    let schema = slides::schema();
    assert!(schema.is_object());
    let text = schema.to_string();
    for op in ["create", "read", "edit", "convert"] {
        assert!(text.contains(op), "schema misses {op}");
    }
    Ok(())
}
