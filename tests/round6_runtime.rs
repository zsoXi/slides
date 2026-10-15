use serde_json::{Value, json};
use slides::{Input, Options, run};
use std::process::{Command, Stdio};
type Result = std::result::Result<(), Box<dyn std::error::Error>>;

#[cfg(unix)]
#[test]
fn soffice_and_font_scan_timeouts_kill_the_child_process() -> Result {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(program) = std::env::var("R6_FAKE_RENDERER") {
        let dir = tempfile::tempdir()?;
        let create: Input = serde_json::from_value(
            json!({"op":"create","output":"deck.pptx","markdown":"# Timeout"}),
        )?;
        let mut options = Options::new(dir.path());
        run(&create, &options)?;
        options.render_timeout = std::time::Duration::from_millis(50);
        if program == "fc-scan" {
            options.font_path = [
                "/usr/share/fonts/noto/NotoSans-Regular.ttf",
                "/usr/share/fonts/TTF/DejaVuSans.ttf",
                "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            ]
            .iter()
            .map(std::path::PathBuf::from)
            .find(|p| p.is_file());
            assert!(options.font_path.is_some());
        }
        let input: Input = serde_json::from_value(
            json!({"op":"convert","input":"deck.pptx","output":"deck.pdf"}),
        )?;
        let error = run(&input, &options).err().ok_or("timeout accepted")?;
        assert_eq!(error.code(), "pdf.timeout");
        assert_eq!(error.params()["program"], program);
        assert!(!dir.path().join("deck.pdf").exists());
        return Ok(());
    }
    for program in ["soffice", "fc-scan"] {
        let dir = tempfile::tempdir()?;
        let executable = dir.path().join(program);
        std::fs::write(
            &executable,
            "#!/bin/sh\necho $$ > \"$R6_PID_FILE\"\nexec /bin/sleep 2\n",
        )?;
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755))?;
        let marker = dir.path().join("pid");
        let mut paths = vec![dir.path().to_path_buf()];
        paths.extend(std::env::split_paths(
            &std::env::var_os("PATH").ok_or("no PATH")?,
        ));
        let start = std::time::Instant::now();
        let output = Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                "soffice_and_font_scan_timeouts_kill_the_child_process",
            ])
            .env("R6_FAKE_RENDERER", program)
            .env("R6_PID_FILE", &marker)
            .env("PATH", std::env::join_paths(paths)?)
            .output()?;
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert!(start.elapsed() < std::time::Duration::from_secs(2));
        let pid = std::fs::read_to_string(marker)?;
        assert!(
            !Command::new("/bin/kill")
                .args(["-0", pid.trim()])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()?
                .success()
        );
    }
    Ok(())
}

#[test]
fn long_qc_findings_are_explicitly_truncated() -> Result {
    let dir = tempfile::tempdir()?;
    let opts = Options::new(dir.path());
    let source = format!("Source: model.xlsx, {}!B1", "MissingSheet".repeat(100));
    let input: Input = serde_json::from_value(
        json!({"op":"create","output":"deck.pptx","slides":[{"title":"Check","text":"Value: 5","sources":[source]}]}),
    )?;
    run(&input, &opts)?;
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    use std::io::Write;
    for (name, content) in [
        ("xl/workbook.xml", "<workbook><sheets/></workbook>"),
        ("xl/_rels/workbook.xml.rels", "<Relationships/>"),
    ] {
        zip.start_file(name, zip::write::SimpleFileOptions::default())?;
        zip.write_all(content.as_bytes())?;
    }
    std::fs::write(dir.path().join("model.xlsx"), zip.finish()?.into_inner())?;
    let input: Input =
        serde_json::from_value(json!({"op":"check","input":"deck.pptx","model":"model.xlsx"}))?;
    let output = run(&input, &opts)?;
    let data = output.data.ok_or("no findings")?;
    let findings = data.as_array().ok_or("not array")?;
    assert!(findings.iter().any(|f| f["text_truncated"] == true
        && f["text"].as_str().is_some_and(|s| s.chars().count() == 512)));
    assert!(
        output
            .warnings
            .iter()
            .any(|w| w.code == "response.truncated")
    );
    Ok(())
}

#[test]
fn parallel_cli_edits_read_the_latest_state_under_one_lock() -> Result {
    let dir = tempfile::tempdir()?;
    run(
        &serde_json::from_value::<Input>(
            json!({"op":"create","output":"deck.pptx","markdown":"# Initial"}),
        )?,
        &Options::new(dir.path()),
    )?;
    let mut children = Vec::new();
    for title in ["First addition", "Second addition"] {
        children.push(Command::new(env!("CARGO_BIN_EXE_slides")).current_dir(dir.path())
            .arg(json!({"op":"edit","input":"deck.pptx","output":"deck.pptx","add":[{"title":title}]}).to_string()).stdout(Stdio::piped()).spawn()?);
    }
    for child in children {
        assert!(child.wait_with_output()?.status.success());
    }
    let output = run(
        &serde_json::from_value::<Input>(json!({"op":"read","input":"deck.pptx"}))?,
        &Options::new(dir.path()),
    )?;
    let data: Value = output.data.ok_or("no data")?;
    let slides = data.as_array().ok_or("not array")?;
    assert_eq!(slides.len(), 3);
    for title in ["First addition", "Second addition"] {
        assert!(slides.iter().any(|s| s["title"] == title));
    }
    Ok(())
}
