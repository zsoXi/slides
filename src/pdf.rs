//! PDF export uses an isolated local LibreOffice process for faithful Office rendering.

use std::process::Command;

use crate::deck::{Deck, Theme};
use crate::messages::{MessageCode, Msg, Warning};
use crate::{Error, Options};

fn io_error(source: std::io::Error) -> Error {
    Error::PdfIo { source }
}

/// Render the original package, not a lossy text-only reconstruction of its slides.
pub fn from_pptx(bytes: &[u8], opts: &Options) -> Result<Vec<u8>, Error> {
    // Some LibreOffice versions reject prefixed OPC roots even though the package is valid.
    // Normalize only those manifests in a temporary copy; slide XML and source bytes stay intact.
    let mut package = crate::ooxml::Package::open(bytes, opts.max_input_bytes)?;
    let manifests: Vec<_> = package
        .0
        .keys()
        .filter(|p| p.as_str() == "[Content_Types].xml" || p.ends_with(".rels"))
        .cloned()
        .collect();
    let mut changed = false;
    for part in manifests {
        let mut dom = package.xml(&part)?;
        if dom.renderer_opc_namespace() {
            package.set_xml(&part, &dom);
            changed = true;
        }
    }
    let normalized = if changed {
        Some(package.finish()?)
    } else {
        None
    };
    let bytes = normalized.as_deref().unwrap_or(bytes);
    if let Some(path) = &opts.font_path {
        let font = std::fs::read(path).map_err(|source| Error::FontRead {
            path: path.display().to_string(),
            source,
        })?;
        if printpdf::ParsedFont::from_bytes(&font, 0, &mut Vec::new()).is_none() {
            return Err(Error::Pdf(Msg::new(MessageCode::PdfFontParse, Vec::new())));
        }
        render(bytes, Some(&font), opts.render_timeout)
    } else {
        render(bytes, None, opts.render_timeout)
    }
}

fn bounded_output(
    command: &mut Command,
    program: &str,
    timeout: std::time::Duration,
) -> Result<std::process::Output, Error> {
    use std::io::{Read, Seek};
    use std::process::Stdio;
    let mut stdout = tempfile::tempfile().map_err(io_error)?;
    command
        .stdout(Stdio::from(stdout.try_clone().map_err(io_error)?))
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command.spawn().map_err(io_error)?;
    let start = std::time::Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(io_error)? {
            break status;
        }
        if start.elapsed() >= timeout {
            // LibreOffice launchers may fork: terminate the isolated group, then reap our child.
            #[cfg(unix)]
            {
                let _ = Command::new("/bin/kill")
                    .args(["-KILL", "--", &format!("-{}", child.id())])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
            if let Err(source) = child.kill()
                && child.try_wait().map_err(io_error)?.is_none()
            {
                return Err(io_error(source));
            }
            child.wait().map_err(io_error)?;
            return Err(Error::Pdf(Msg::new(
                MessageCode::PdfTimeout,
                crate::messages::params(&[
                    ("program", program),
                    ("seconds", &timeout.as_secs_f64().to_string()),
                ]),
            )));
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    stdout.rewind().map_err(io_error)?;
    let mut bytes = Vec::new();
    stdout
        .take(4096)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    Ok(std::process::Output {
        status,
        stdout: bytes,
        stderr: Vec::new(),
    })
}

fn render(
    pptx_bytes: &[u8],
    font: Option<&[u8]>,
    timeout: std::time::Duration,
) -> Result<Vec<u8>, Error> {
    let work = tempfile::tempdir().map_err(io_error)?;
    let input = work.path().join("slides.pptx");
    std::fs::write(&input, pptx_bytes).map_err(io_error)?;
    let profile = work.path().join("profile");
    // The temporary directory is generated locally, never from an untrusted request path.
    let profile_url = format!("file://{}", profile.display());
    let mut command = Command::new("soffice");
    if let Some(bytes) = font {
        let dir = work.path().join("fonts");
        std::fs::create_dir(&dir).map_err(io_error)?;
        let path = dir.join("regular.ttf");
        std::fs::write(&path, bytes).map_err(io_error)?;
        let scan = bounded_output(
            Command::new("fc-scan")
                .args(["--format", "%{family[0]}"])
                .arg(&path),
            "fc-scan",
            timeout,
        )?;
        let family =
            String::from_utf8(scan.stdout).map_err(|source| Error::FontNameUtf8 { source })?;
        if !scan.status.success() || family.is_empty() {
            return Err(Error::Pdf(Msg::new(MessageCode::PdfFontParse, Vec::new())));
        }
        let package = crate::pptx::with_font(pptx_bytes, &family)?;
        std::fs::write(&input, package).map_err(io_error)?;
        let config = work.path().join("fonts.conf");
        let mut xml = crate::xmlw::Xml::new();
        xml.decl();
        xml.open("fontconfig", &[]);
        xml.el(
            "include",
            &[("ignore_missing", "yes")],
            "/etc/fonts/fonts.conf",
        );
        xml.el("dir", &[], &dir.to_string_lossy());
        for name in ["Arial", "Calibri", "Calibri Light"] {
            xml.open("alias", &[("binding", "strong")]);
            xml.el("family", &[], name);
            xml.open("prefer", &[]);
            xml.el("family", &[], &family);
            xml.close("prefer");
            xml.close("alias");
        }
        xml.close("fontconfig");
        std::fs::write(&config, xml.finish()).map_err(io_error)?;
        command.env("FONTCONFIG_FILE", config);
    }
    let result = bounded_output(
        command
            .arg(format!("-env:UserInstallation={profile_url}"))
            .args(["--headless", "--convert-to", "pdf", "--outdir"])
            .arg(work.path())
            .arg(&input),
        "soffice",
        timeout,
    )?;
    let output = work.path().join("slides.pdf");
    if !result.status.success() || !output.is_file() {
        return Err(Error::Pdf(Msg::new(MessageCode::PdfRender, Vec::new())));
    }
    let mut pdf = std::fs::read(output).map_err(io_error)?;
    if !pdf.starts_with(b"%PDF-") {
        return Err(Error::Pdf(Msg::new(MessageCode::PdfRender, Vec::new())));
    }
    canonical_metadata(&mut pdf)?;
    Ok(pdf)
}

// LibreOffice's controlled export uses uncompressed Info/XMP metadata and a classic trailer.
// Replace only metadata values, at the same byte lengths, leaving streams and xref offsets intact.
fn canonical_metadata(pdf: &mut [u8]) -> Result<(), Error> {
    let invalid = || Error::Pdf(Msg::new(MessageCode::PdfRender, Vec::new()));
    let locate =
        |bytes: &[u8], needle: &[u8]| bytes.windows(needle.len()).position(|v| v == needle);
    let trailer = pdf
        .windows(7)
        .rposition(|v| v == b"trailer")
        .ok_or_else(invalid)?;
    let id = trailer + locate(&pdf[trailer..], b"/ID").ok_or_else(invalid)?;
    let end = id + locate(&pdf[id..], b"]").ok_or_else(invalid)?;
    let mut cursor = id;
    while cursor < end {
        if pdf[cursor] == b'<' {
            let close = cursor + locate(&pdf[cursor..end], b">").ok_or_else(invalid)?;
            if !pdf[cursor + 1..close].iter().all(u8::is_ascii_hexdigit) {
                return Err(invalid());
            }
            pdf[cursor + 1..close].fill(b'0');
            cursor = close;
        }
        cursor += 1;
    }
    // DocChecksum is LibreOffice-specific integrity metadata, not a PDF content checksum.
    if let Some(offset) = locate(&pdf[trailer..], b"/DocChecksum /") {
        let start = trailer + offset;
        let end = start + b"/DocChecksum /".len() + 32;
        let value = pdf.get_mut(start..end).ok_or_else(invalid)?;
        value.fill(b' ');
    }
    let marker = b"/CreationDate(D:";
    let date = locate(pdf, marker).ok_or_else(invalid)? + marker.len();
    let close = date + locate(&pdf[date..], b")").ok_or_else(invalid)?;
    let original = pdf[date..close].to_vec();
    if original.len() < 14 || !original[..14].iter().all(u8::is_ascii_digit) {
        return Err(invalid());
    }
    let mut fixed = original.clone();
    fixed[..14].copy_from_slice(b"20000101000000");
    // Replace this exact date only inside the Info entry.
    pdf[date..close].copy_from_slice(&fixed);
    if let Some(metadata) =
        locate(pdf, b"/Type/Metadata").or_else(|| locate(pdf, b"/Type /Metadata"))
    {
        let start = metadata + locate(&pdf[metadata..], b"stream\n").ok_or_else(invalid)? + 7;
        let end = start + locate(&pdf[start..], b"endstream").ok_or_else(invalid)?;
        // XMP uses ISO 8601 dates, always in the metadata stream rather than slide text.
        let mut cursor = start;
        while cursor + 19 <= end {
            if pdf[cursor..cursor + 19]
                .iter()
                .enumerate()
                .all(|(i, v)| match i {
                    4 | 7 => *v == b'-',
                    10 => *v == b'T',
                    13 | 16 => *v == b':',
                    _ => v.is_ascii_digit(),
                })
            {
                pdf[cursor..cursor + 19].copy_from_slice(b"2000-01-01T00:00:00");
                cursor += 19;
            } else {
                cursor += 1;
            }
        }
    }
    // Derive a stable document identity from the canonical bytes, not random renderer state.
    use std::hash::{Hash, Hasher};
    let mut hash = std::hash::DefaultHasher::new();
    pdf.hash(&mut hash);
    let identity = format!("{:016X}{:016X}", hash.finish(), pdf.len());
    let mut cursor = id;
    while cursor < end {
        if pdf[cursor] == b'<' {
            let close = cursor + locate(&pdf[cursor..end], b">").ok_or_else(invalid)?;
            if close - cursor != 33 {
                return Err(invalid());
            }
            pdf[cursor + 1..close].copy_from_slice(identity.as_bytes());
            cursor = close;
        }
        cursor += 1;
    }
    Ok(())
}

/// Use the very same package and layout engine for direct PDF and PPTX output.
pub fn write(deck: &Deck, theme: Theme, opts: &Options) -> Result<(Vec<u8>, Vec<Warning>), Error> {
    let (pptx, warnings) = crate::pptx::write(deck, theme, &opts.base_dir, &opts.locale)?;
    let font = opts
        .font_path
        .clone()
        .or_else(|| {
            [
                "/usr/share/fonts/noto/NotoSans-Regular.ttf",
                "/usr/share/fonts/TTF/DejaVuSans.ttf",
                "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            ]
            .iter()
            .map(std::path::PathBuf::from)
            .find(|p| p.is_file())
        })
        .ok_or_else(|| Error::Pdf(Msg::new(MessageCode::PdfFontMissing, Vec::new())))?;
    let bytes = std::fs::read(&font).map_err(|source| Error::FontRead {
        path: font.display().to_string(),
        source,
    })?;
    if printpdf::ParsedFont::from_bytes(&bytes, 0, &mut Vec::new()).is_none() {
        return Err(Error::Pdf(Msg::new(MessageCode::PdfFontParse, Vec::new())));
    }
    Ok((render(&pptx, Some(&bytes), opts.render_timeout)?, warnings))
}
