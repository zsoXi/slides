//! A tiny XML writer: enough to emit the OOXML parts we generate.

/// Reject forbidden XML 1.0 characters instead of silently changing user data.
pub(crate) fn validate_text(text: &str, field: &str) -> Result<(), crate::Error> {
    if text.chars().any(|c| !matches!(c, '\u{9}' | '\u{a}' | '\u{d}' | '\u{20}'..='\u{d7ff}' | '\u{e000}'..='\u{fffd}' | '\u{10000}'..='\u{10ffff}')) {
        return Err(crate::input_error(
            crate::messages::MessageCode::XmlCharacter,
            crate::messages::params(&[("field", field)]),
        ));
    }
    Ok(())
}

/// Validate every string in a serialized request, retaining the exact field path.
pub(crate) fn validate_input(value: &serde_json::Value, field: &str) -> Result<(), crate::Error> {
    match value {
        serde_json::Value::String(text) => validate_text(text, field)?,
        serde_json::Value::Array(values) => {
            for (i, value) in values.iter().enumerate() {
                validate_input(value, &format!("{field}[{i}]"))?;
            }
        }
        serde_json::Value::Object(values) => {
            for (key, value) in values {
                let path = if field.is_empty() {
                    key.clone()
                } else {
                    format!("{field}.{key}")
                };
                validate_input(value, &path)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Escape text or an attribute value.
pub fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

/// A string builder that knows how to open, close and self-close XML elements.
#[derive(Default, Clone)]
pub struct Xml {
    s: String,
}

impl Xml {
    pub fn new() -> Self {
        Xml { s: String::new() }
    }

    pub fn decl(&mut self) {
        self.s
            .push_str("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>");
    }

    pub fn open(&mut self, tag: &str, attrs: &[(&str, &str)]) {
        self.s.push('<');
        self.s.push_str(tag);
        self.attrs(attrs);
        self.s.push('>');
    }

    pub fn empty(&mut self, tag: &str, attrs: &[(&str, &str)]) {
        self.s.push('<');
        self.s.push_str(tag);
        self.attrs(attrs);
        self.s.push_str("/>");
    }

    pub fn close(&mut self, tag: &str) {
        self.s.push_str("</");
        self.s.push_str(tag);
        self.s.push('>');
    }

    pub fn text(&mut self, text: &str) {
        self.s.push_str(&esc(text));
    }

    pub fn el(&mut self, tag: &str, attrs: &[(&str, &str)], text: &str) {
        self.open(tag, attrs);
        self.text(text);
        self.close(tag);
    }

    fn attrs(&mut self, attrs: &[(&str, &str)]) {
        for (k, v) in attrs {
            self.s.push(' ');
            self.s.push_str(k);
            self.s.push_str("=\"");
            self.s.push_str(&esc(v));
            self.s.push('"');
        }
    }

    pub fn finish(self) -> Vec<u8> {
        self.s.into_bytes()
    }
}
