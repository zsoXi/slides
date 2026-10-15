//! A minimal XML document model used to read and edit OOXML parts.

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use crate::Error;
use crate::messages::{MessageCode, Msg};
use crate::xmlw::esc;

/// Every external tree is bounded here, before any recursive consumer or derived clone/drop.
/// Internal constructors only add shallow OOXML structures to these parsed trees.
const MAX_DEPTH: usize = 256;

fn check_depth(depth: usize) -> Result<(), Error> {
    if depth > MAX_DEPTH {
        return Err(Error::Xml(Msg::new(
            MessageCode::XmlDepthLimit,
            crate::messages::params(&[("limit", &MAX_DEPTH.to_string())]),
        )));
    }
    Ok(())
}

fn xml_error(source: impl std::error::Error + Send + Sync + 'static) -> Error {
    Error::XmlSource {
        source: Box::new(source),
    }
}

#[derive(Clone, Debug)]
pub enum Node {
    El(El),
    Text(String),
    Raw(String),
}

/// Namespace-resolved names are canonical internal aliases, never input prefixes.
/// Keep lexical names and their scope separately so unchanged content retains its spelling.
#[derive(Clone, Debug, Default)]
pub struct Spelling {
    scope: Vec<(String, String)>,
    names: Vec<(String, String)>,
}

const NAMESPACES: &[(&str, &str)] = &[
    ("a", "http://schemas.openxmlformats.org/drawingml/2006/main"),
    (
        "p",
        "http://schemas.openxmlformats.org/presentationml/2006/main",
    ),
    (
        "c",
        "http://schemas.openxmlformats.org/drawingml/2006/chart",
    ),
    (
        "r",
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships",
    ),
    // ISO/IEC 29500 Strict uses different URIs for the same supported vocabularies.
    ("a", "http://purl.oclc.org/ooxml/drawingml/main"),
    ("p", "http://purl.oclc.org/ooxml/presentationml/main"),
    ("c", "http://purl.oclc.org/ooxml/drawingml/chart"),
    (
        "r",
        "http://purl.oclc.org/ooxml/officeDocument/relationships",
    ),
    ("", "http://purl.oclc.org/ooxml/spreadsheetml/main"),
    ("sl", "urn:slides:logical-table:v1"),
    ("sb", "urn:slides:block:v1"),
    (
        "mc",
        "http://schemas.openxmlformats.org/markup-compatibility/2006",
    ),
    ("xml", "http://www.w3.org/XML/1998/namespace"),
    (
        "",
        "http://schemas.openxmlformats.org/spreadsheetml/2006/main",
    ),
    (
        "",
        "http://schemas.openxmlformats.org/package/2006/relationships",
    ),
    (
        "",
        "http://schemas.openxmlformats.org/package/2006/content-types",
    ),
];

fn binding<'a>(scope: &'a [(String, String)], prefix: &str) -> Option<&'a str> {
    scope
        .iter()
        .rev()
        .find(|(p, _)| p == prefix)
        .map(|(_, uri)| uri.as_str())
}

fn expanded(name: &str, scope: &[(String, String)], attribute: bool) -> String {
    if name == "xmlns" || name.starts_with("xmlns:") {
        return name.into();
    }
    let (prefix, local) = name.split_once(':').unwrap_or(("", name));
    if attribute && prefix.is_empty() {
        return name.into(); // Default namespaces never qualify attributes.
    }
    let uri = if prefix == "xml" {
        Some("http://www.w3.org/XML/1998/namespace")
    } else {
        binding(scope, prefix)
    };
    match uri {
        Some(uri) if !uri.is_empty() => match NAMESPACES.iter().find(|(_, u)| *u == uri) {
            Some(("", _)) if attribute => format!("{{{uri}}}{local}"),
            Some(("", _)) => local.into(),
            Some((alias, _)) => format!("{alias}:{local}"),
            None => format!("{{{uri}}}{local}"),
        },
        // Unbound prefixes must not impersonate a recognized namespace.
        _ if !prefix.is_empty() => format!("{{unbound:{prefix}}}{local}"),
        _ => local.into(),
    }
}

#[derive(Clone, Debug)]
pub struct El {
    pub spelling: Spelling,
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Node>,
}

impl El {
    /// LibreOffice's OPC loader requires default namespaces for package manifests/relationships.
    /// Use only for its temporary render copy, never for an edited source package.
    pub(crate) fn renderer_opc_namespace(&mut self) -> bool {
        let uri = match self.name.as_str() {
            "Types" => "http://schemas.openxmlformats.org/package/2006/content-types",
            "Relationships" => "http://schemas.openxmlformats.org/package/2006/relationships",
            _ => return false,
        };
        if !self
            .spelling
            .names
            .first()
            .is_some_and(|(_, original)| original.contains(':'))
        {
            return false;
        }
        self.set_attr("xmlns", uri);
        self.visit_mut(|el| {
            // Retain attribute spellings and foreign extension namespaces.
            if matches!(
                el.name.as_str(),
                "Types" | "Default" | "Override" | "Relationships" | "Relationship"
            ) {
                el.spelling
                    .names
                    .retain(|(canonical, _)| canonical != &el.name);
            }
            true
        });
        true
    }

    /// Select a compatibility branch by namespace URI, not the spelling of Requires.
    /// Only core presentation, drawing and chart namespaces are implemented here.
    pub(crate) fn compatibility_branch(&self) -> Option<&El> {
        self.children_named("mc:Choice")
            .find(|choice| {
                choice.attr("Requires").is_some_and(|requires| {
                    !requires.trim().is_empty()
                        && requires.split_whitespace().all(|prefix| {
                            binding(&choice.spelling.scope, prefix).is_some_and(|uri| {
                                NAMESPACES.iter().any(|(alias, supported)| {
                                    matches!(*alias, "p" | "a" | "c") && *supported == uri
                                })
                            })
                        })
                })
            })
            .or_else(|| self.child("mc:Fallback"))
    }

    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    pub fn set_attr(&mut self, name: &str, value: &str) {
        match self.attrs.iter_mut().find(|(k, _)| k == name) {
            Some((_, v)) => *v = value.to_string(),
            None => self.attrs.push((name.to_string(), value.to_string())),
        }
    }

    pub fn elements(&self) -> impl Iterator<Item = &El> {
        self.children.iter().filter_map(|n| match n {
            Node::El(e) => Some(e),
            _ => None,
        })
    }

    pub fn elements_mut(&mut self) -> impl Iterator<Item = &mut El> {
        self.children.iter_mut().filter_map(|n| match n {
            Node::El(e) => Some(e),
            _ => None,
        })
    }

    /// Iterative pre-order mutation. Return false to skip the visited element's children.
    /// This also keeps internal transformations independent of the process stack size.
    pub fn visit_mut(&mut self, mut visit: impl FnMut(&mut El) -> bool) {
        let mut pending = vec![self];
        while let Some(el) = pending.pop() {
            if visit(el) {
                let children: Vec<_> = el.elements_mut().collect();
                pending.extend(children.into_iter().rev());
            }
        }
    }

    pub fn child_mut(&mut self, name: &str) -> Option<&mut El> {
        self.children.iter_mut().find_map(|n| match n {
            Node::El(e) if e.name == name => Some(e),
            _ => None,
        })
    }

    pub fn child(&self, name: &str) -> Option<&El> {
        self.elements().find(|e| e.name == name)
    }

    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a El> + 'a {
        self.elements().filter(move |e| e.name == name)
    }

    /// All descendant elements with the given name, in document order.
    pub fn descendants<'a>(&'a self, name: &str, out: &mut Vec<&'a El>) {
        let mut pending: Vec<_> = self.elements().collect();
        pending.reverse();
        while let Some(child) = pending.pop() {
            if child.name == name {
                out.push(child);
            }
            pending.extend(child.elements().collect::<Vec<_>>().into_iter().rev());
        }
    }

    /// Concatenated text of this element and its descendants.
    pub fn text(&self) -> String {
        let mut out = String::new();
        collect_text(self, &mut out);
        out
    }
}

fn collect_text(el: &El, out: &mut String) {
    let mut pending: Vec<_> = el.children.iter().rev().collect();
    while let Some(node) = pending.pop() {
        match node {
            Node::Text(t) => out.push_str(t),
            Node::El(e) => pending.extend(e.children.iter().rev()),
            Node::Raw(_) => {}
        }
    }
}

fn start_to_el(e: &BytesStart<'_>, parent: Option<&El>) -> Result<El, Error> {
    let original_name = e.name().as_ref().to_string();
    let mut attrs = Vec::new();
    for attr in e.attributes() {
        let attr = attr.map_err(xml_error)?;
        let key = attr.key.as_ref().to_string();
        let value = attr
            .normalized_value(quick_xml::XmlVersion::default())
            .map_err(xml_error)?
            .into_owned();
        attrs.push((key, value));
    }
    let mut scope = parent.map(|p| p.spelling.scope.clone()).unwrap_or_default();
    for (key, value) in &attrs {
        if key == "xmlns" {
            scope.push((String::new(), value.clone()));
        } else if let Some(prefix) = key.strip_prefix("xmlns:") {
            scope.push((prefix.into(), value.clone()));
        }
    }
    let name = expanded(&original_name, &scope, false);
    let mut names = vec![(name.clone(), original_name)];
    for (key, _) in &mut attrs {
        let canonical = expanded(key, &scope, true);
        names.push((canonical.clone(), key.clone()));
        *key = canonical;
    }
    Ok(El {
        spelling: Spelling { scope, names },
        name,
        attrs,
        children: Vec::new(),
    })
}

fn attach(stack: &mut [El], root: &mut Option<El>, node: Node) -> Result<(), Error> {
    match stack.last_mut() {
        Some(parent) => {
            parent.children.push(node);
            Ok(())
        }
        None => match node {
            Node::El(el) => {
                if root.is_some() {
                    return Err(Error::Xml(Msg::new(
                        MessageCode::XmlMultipleRoots,
                        Vec::new(),
                    )));
                }
                *root = Some(el);
                Ok(())
            }
            _ => Ok(()),
        },
    }
}

fn attach_text(stack: &mut [El], text: &str) {
    if let Some(parent) = stack.last_mut() {
        if let Some(Node::Text(previous)) = parent.children.last_mut() {
            previous.push_str(text);
        } else {
            parent.children.push(Node::Text(text.to_string()));
        }
    }
}

/// Check nesting without allocating a tree. Package loading uses this before optional readers
/// can skip a malformed ancillary part; every later recursive OOXML consumer shares this gate.
pub(crate) fn validate_nesting(bytes: &[u8]) -> Result<(), Error> {
    let mut reader = Reader::from_reader(bytes);
    let mut depth = 0usize;
    loop {
        match reader.read_event().map_err(xml_error)? {
            Event::Start(_) => {
                depth += 1;
                check_depth(depth)?;
            }
            Event::Empty(_) => check_depth(depth + 1)?,
            Event::End(_) => depth = depth.saturating_sub(1),
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(())
}

/// Parse an XML part into an element tree. No recursive consumer is given an unbounded tree.
pub fn parse(bytes: &[u8]) -> Result<El, Error> {
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(false);
    let mut buf = Vec::new();
    let mut stack: Vec<El> = Vec::new();
    let mut root: Option<El> = None;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                check_depth(stack.len() + 1)?;
                stack.push(start_to_el(&e, stack.last())?);
            }
            Ok(Event::Empty(e)) => {
                check_depth(stack.len() + 1)?;
                let el = start_to_el(&e, stack.last())?;
                attach(&mut stack, &mut root, Node::El(el))?;
            }
            Ok(Event::End(_)) => {
                if let Some(el) = stack.pop() {
                    attach(&mut stack, &mut root, Node::El(el))?;
                }
            }
            Ok(Event::Text(e)) => {
                let raw = e.into_inner();
                let text = quick_xml::escape::unescape(&raw).map_err(xml_error)?;
                attach_text(&mut stack, &text);
            }
            Ok(Event::GeneralRef(e)) => {
                let reference = format!("&{};", e.as_ref());
                let text = quick_xml::escape::unescape(&reference).map_err(xml_error)?;
                attach_text(&mut stack, &text);
            }
            Ok(Event::CData(e)) => {
                // CDATA changes escaping, not semantic content. Merge adjacent text so that
                // replacement also works across plain-text / CDATA boundaries.
                attach_text(&mut stack, &e.into_inner());
            }
            Ok(Event::Comment(e)) => {
                let raw = e.as_ref().to_string();
                let node = Node::Raw(format!("<!--{raw}-->"));
                attach(&mut stack, &mut root, node)?;
            }
            Ok(Event::PI(e)) => {
                let raw = e.as_ref().to_string();
                let node = Node::Raw(format!("<?{raw}?>"));
                attach(&mut stack, &mut root, node)?;
            }
            Ok(Event::DocType(e)) => {
                let raw = e.as_ref().to_string();
                let node = Node::Raw(format!("<!DOCTYPE {raw}>"));
                attach(&mut stack, &mut root, node)?;
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(e) => return Err(xml_error(e)),
        }
        buf.clear();
    }
    root.ok_or_else(|| Error::Xml(Msg::new(MessageCode::XmlEmpty, Vec::new())))
}

/// Serialize an element tree back to an XML part.
pub fn write(el: &El) -> Vec<u8> {
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>");
    write_el(el, &mut out);
    out.into_bytes()
}

fn write_el(el: &El, out: &mut String) {
    enum Task<'a> {
        Element(&'a El, Vec<(String, String)>),
        Child(&'a Node, Vec<(String, String)>),
        Close(String),
    }
    let mut pending = vec![Task::Element(el, Vec::new())];
    while let Some(task) = pending.pop() {
        let (el, mut scope) = match task {
            Task::Element(el, scope) | Task::Child(Node::El(el), scope) => (el, scope),
            Task::Child(Node::Text(text), _) => {
                out.push_str(&esc(text));
                continue;
            }
            Task::Child(Node::Raw(raw), _) => {
                out.push_str(raw);
                continue;
            }
            Task::Close(name) => {
                out.push_str("</");
                out.push_str(&name);
                out.push('>');
                continue;
            }
        };
        let mut attrs = el.attrs.clone();
        for (key, value) in &attrs {
            if key == "xmlns" {
                scope.push((String::new(), value.clone()));
            } else if let Some(prefix) = key.strip_prefix("xmlns:") {
                scope.push((prefix.into(), value.clone()));
            }
        }
        let mut lexical = |canonical: &str, attribute: bool| {
            let original = el
                .spelling
                .names
                .iter()
                .find(|(c, _)| c == canonical)
                .map(|(_, o)| o.as_str());
            let name = original.unwrap_or(canonical).to_string();
            if name == "xmlns" || name.starts_with("xmlns:") || (attribute && !name.contains(':')) {
                return name;
            }
            let prefix = name.split_once(':').map(|(p, _)| p).unwrap_or("");
            let uri = if original.is_some() {
                binding(&el.spelling.scope, prefix)
            } else {
                NAMESPACES
                    .iter()
                    .find(|(p, _)| *p == prefix && !p.is_empty())
                    .map(|(_, u)| *u)
                    .or_else(|| {
                        if prefix.is_empty() {
                            // Newly created OPC entries still need their namespace when the
                            // containing document uses a prefix rather than a default binding.
                            match canonical {
                                "Relationships" | "Relationship" => Some(
                                    "http://schemas.openxmlformats.org/package/2006/relationships",
                                ),
                                "Types" | "Default" | "Override" => Some(
                                    "http://schemas.openxmlformats.org/package/2006/content-types",
                                ),
                                _ => binding(&scope, ""),
                            }
                        } else {
                            None
                        }
                    })
            }
            .unwrap_or("");
            if prefix != "xml" && binding(&scope, prefix).unwrap_or("") != uri {
                let key = if prefix.is_empty() {
                    "xmlns".into()
                } else {
                    format!("xmlns:{prefix}")
                };
                attrs.push((key, uri.into()));
                scope.push((prefix.into(), uri.into()));
            }
            name
        };
        let name = lexical(&el.name, false);
        let keys: Vec<_> = el.attrs.iter().map(|(key, _)| lexical(key, true)).collect();
        out.push('<');
        out.push_str(&name);
        for (i, (key, value)) in attrs.iter().enumerate() {
            out.push(' ');
            out.push_str(keys.get(i).unwrap_or(key));
            out.push_str("=\"");
            out.push_str(&esc(value));
            out.push('"');
        }
        if el.children.is_empty() {
            out.push_str("/>");
        } else {
            out.push('>');
            pending.push(Task::Close(name));
            pending.extend(
                el.children
                    .iter()
                    .rev()
                    .map(|n| Task::Child(n, scope.clone())),
            );
        }
    }
}
