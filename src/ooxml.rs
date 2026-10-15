//! Small bounded OPC helpers shared by templates and number checks.
use std::collections::BTreeMap;
use std::io::{Cursor, Read, Write};

use crate::Error;
use crate::messages::{MessageCode, Msg, params};
use crate::xmldom::{self, El, Node};

pub(crate) fn invalid(part: &str) -> Error {
    Error::Pptx(Msg::new(
        MessageCode::PackageInvalid,
        params(&[("part", part)]),
    ))
}

pub(crate) fn slide_list(pres: &El) -> Result<&El, Error> {
    if pres.name != "p:presentation" {
        return Err(invalid("ppt/presentation.xml"));
    }
    pres.child("p:sldIdLst")
        .ok_or_else(|| invalid("p:sldIdLst"))
}

pub(crate) fn slide_tree(slide: &El) -> Result<&El, Error> {
    if slide.name != "p:sld" {
        return Err(invalid("p:sld"));
    }
    slide
        .child("p:cSld")
        .and_then(|c| c.child("p:spTree"))
        .ok_or_else(|| invalid("p:spTree"))
}

pub(crate) fn el(name: &str, attrs: &[(&str, &str)], children: Vec<Node>) -> El {
    El {
        spelling: Default::default(),
        name: name.into(),
        attrs: attrs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        children,
    }
}
pub(crate) fn node(name: &str, attrs: &[(&str, &str)], children: Vec<Node>) -> Node {
    Node::El(el(name, attrs, children))
}
pub(crate) fn text(name: &str, value: &str) -> Node {
    node(name, &[], vec![Node::Text(value.into())])
}
pub(crate) fn descendants<'a>(root: &'a El, name: &str) -> Vec<&'a El> {
    let mut out = Vec::new();
    root.descendants(name, &mut out);
    out
}
pub(crate) fn rels_path(part: &str) -> String {
    match part.rsplit_once('/') {
        Some((dir, file)) => format!("{dir}/_rels/{file}.rels"),
        None => format!("_rels/{part}.rels"),
    }
}
pub(crate) fn relationship_source(path: &str) -> Option<String> {
    if path == "_rels/.rels" {
        return Some(String::new());
    }
    let (dir, file) = path.rsplit_once("/_rels/")?;
    Some(format!("{dir}/{}", file.strip_suffix(".rels")?))
}

pub(crate) fn target(part: &str, raw: &str) -> Result<String, Error> {
    let mut bits: Vec<&str> = if raw.starts_with('/') {
        Vec::new()
    } else {
        part.rsplit_once('/')
            .map(|(d, _)| d.split('/').collect())
            .unwrap_or_default()
    };
    for bit in raw.split('/') {
        match bit {
            "" | "." => {}
            ".." => {
                if bits.pop().is_none() {
                    return Err(invalid(raw));
                }
            }
            s => bits.push(s),
        }
    }
    Ok(bits.join("/"))
}

pub(crate) struct Package(pub BTreeMap<String, Vec<u8>>);
impl Package {
    pub fn open(bytes: &[u8], limit: u64) -> Result<Self, Error> {
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(Error::zip)?;
        if zip.len() > 50_000 {
            return Err(invalid("ZIP entry count"));
        }
        let mut parts = BTreeMap::new();
        let mut total = 0u64;
        for i in 0..zip.len() {
            let mut file = zip.by_index(i).map_err(Error::zip)?;
            if file.is_dir() {
                continue;
            }
            let name = file.name().map_err(Error::zip)?.to_string();
            if name.starts_with('/')
                || name.contains('\\')
                || name.split('/').any(|s| matches!(s, ".." | "." | ""))
            {
                return Err(invalid(&name));
            }
            let cap = limit.saturating_sub(total);
            // Bound CPU/memory amplification as well as total inflated bytes.
            if file.size() > cap || file.size() > file.compressed_size().max(1).saturating_mul(1000)
            {
                return Err(invalid(&name));
            }
            let mut data = Vec::new();
            (&mut file)
                .take(cap.saturating_add(1))
                .read_to_end(&mut data)
                .map_err(Error::zip)?;
            if name.ends_with(".xml") || name.ends_with(".rels") {
                xmldom::validate_nesting(&data)?;
            }
            total = total.saturating_add(data.len() as u64);
            if total > limit || parts.insert(name.clone(), data).is_some() {
                return Err(invalid(&name));
            }
        }
        Ok(Self(parts))
    }
    pub fn xml(&self, part: &str) -> Result<El, Error> {
        xmldom::parse(self.0.get(part).ok_or_else(|| invalid(part))?)
    }
    pub fn set_xml(&mut self, part: &str, dom: &El) {
        self.0.insert(part.into(), xmldom::write(dom));
    }
    pub fn rels(&self, part: &str) -> Result<El, Error> {
        let path = rels_path(part);
        if self.0.contains_key(&path) {
            self.xml(&path)
        } else {
            Ok(el(
                "Relationships",
                &[(
                    "xmlns",
                    "http://schemas.openxmlformats.org/package/2006/relationships",
                )],
                vec![],
            ))
        }
    }
    pub fn related(&self, part: &str, kind: &str) -> Result<Vec<(String, String)>, Error> {
        let rels = self.rels(part)?;
        rels.children_named("Relationship")
            .filter(|r| {
                r.attr("Type").and_then(|t| t.rsplit('/').next()) == Some(kind)
                    && !r
                        .attr("TargetMode")
                        .is_some_and(|s| s.eq_ignore_ascii_case("external"))
            })
            .map(|r| {
                Ok((
                    r.attr("Id").ok_or_else(|| invalid(part))?.to_string(),
                    target(part, r.attr("Target").ok_or_else(|| invalid(part))?)?,
                ))
            })
            .collect()
    }
    pub fn slides(&self) -> Result<Vec<String>, Error> {
        let pres = self.xml("ppt/presentation.xml")?;
        let rels = self.related("ppt/presentation.xml", "slide")?;
        slide_list(&pres)?
            .children_named("p:sldId")
            .map(|id| {
                rels.iter()
                    .find(|(r, _)| Some(r.as_str()) == id.attr("r:id"))
                    .filter(|(_, p)| self.0.contains_key(p))
                    .map(|(_, p)| p.clone())
                    .ok_or_else(|| invalid("p:sldId"))
            })
            .collect()
    }
    /// Reachability starts at the package root; relationship parts belong to their source.
    /// Used after slide removal and template composition; ordinary text edits preserve unrelated parts.
    pub fn prune_unreachable(&mut self) -> Result<(), Error> {
        let mut reachable = std::collections::BTreeSet::from(["[Content_Types].xml".to_string()]);
        let mut pending = vec![String::new()];
        let mut visited = std::collections::BTreeSet::new();
        while let Some(source) = pending.pop() {
            if !visited.insert(source.clone()) {
                continue;
            }
            if !source.is_empty() {
                reachable.insert(source.clone());
            }
            let path = if source.is_empty() {
                "_rels/.rels".into()
            } else {
                rels_path(&source)
            };
            if !self.0.contains_key(&path) {
                continue;
            }
            reachable.insert(path.clone());
            for rel in self.xml(&path)?.children_named("Relationship") {
                if rel
                    .attr("TargetMode")
                    .is_some_and(|s| s.eq_ignore_ascii_case("external"))
                {
                    continue;
                }
                let next = target(&source, rel.attr("Target").ok_or_else(|| invalid(&path))?)?;
                if !self.0.contains_key(&next) {
                    return Err(invalid(&next));
                }
                pending.push(next);
            }
        }
        self.0.retain(|p, _| reachable.contains(p));
        let mut ct = self.xml("[Content_Types].xml")?;
        ct.children.retain(|n| !matches!(n, Node::El(e) if e.name == "Override" && e.attr("PartName").is_some_and(|p| !self.0.contains_key(p.trim_start_matches('/')))));
        self.set_xml("[Content_Types].xml", &ct);
        Ok(())
    }

    pub fn validate_relationships(&self) -> Result<(), Error> {
        self.slides()?;
        for path in self.0.keys().filter(|p| p.ends_with(".rels")) {
            let source = relationship_source(path).ok_or_else(|| invalid(path))?;
            if !source.is_empty() && !self.0.contains_key(&source) {
                return Err(invalid(path));
            }
            for rel in self.xml(path)?.children_named("Relationship") {
                if rel
                    .attr("TargetMode")
                    .is_some_and(|s| s.eq_ignore_ascii_case("external"))
                {
                    continue;
                }
                let next = target(&source, rel.attr("Target").ok_or_else(|| invalid(path))?)?;
                if !self.0.contains_key(&next) {
                    return Err(invalid(&next));
                }
            }
        }
        Ok(())
    }

    pub fn finish(&self) -> Result<Vec<u8>, Error> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default()
            .last_modified_time(zip::DateTime::default())
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, data) in &self.0 {
            zip.start_file(name, options).map_err(Error::zip)?;
            zip.write_all(data).map_err(Error::zip)?;
        }
        Ok(zip.finish().map_err(Error::zip)?.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    type TestResult = Result<(), Box<dyn std::error::Error>>;
    fn archive(names: &[&str], data: &[u8]) -> Result<Vec<u8>, Error> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for name in names {
            zip.start_file(
                *name,
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Deflated),
            )
            .map_err(Error::zip)?;
            zip.write_all(data).map_err(Error::zip)?;
        }
        Ok(zip.finish().map_err(Error::zip)?.into_inner())
    }
    #[test]
    fn package_rejects_unsafe_part_names() -> TestResult {
        for name in [
            "/absolute.xml",
            "../outside.xml",
            "a/../outside.xml",
            "a\\\\b.xml",
        ] {
            assert!(
                Package::open(&archive(&[name], b"x")?, 100).is_err(),
                "accepted {name}"
            );
        }
        Ok(())
    }
    #[test]
    fn package_bounds_expanded_bytes_and_entry_count() -> TestResult {
        let bytes = archive(&["a.xml"], &vec![b'x'; 10000])?;
        assert!(Package::open(&bytes, 9999).is_err());
        assert!(Package::open(&bytes, 10000).is_ok());
        assert!(Package::open(&archive(&["a.xml", "b.xml"], b"1234")?, 7).is_err());
        let names: Vec<_> = (0..50001).map(|i| format!("{i}.xml")).collect();
        let names: Vec<_> = names.iter().map(String::as_str).collect();
        assert!(Package::open(&archive(&names, b"")?, 100).is_err());
        Ok(())
    }
}
