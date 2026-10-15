//! PPTX reading, writing and editing (Office Open XML, PresentationML).

use std::collections::{BTreeMap, HashMap};
use std::io::{Cursor, Write};
use std::path::Path;

use zip::write::SimpleFileOptions;

use crate::deck::{Block, Deck, Layout, Slide, Table, Theme};
use crate::messages::{MessageCode, Msg, Warning, params};
use crate::xmldom::{self, El, Node};
use crate::xmlw::Xml;
use crate::{EditInput, Error, SlideEdit};

fn pptx_error(code: MessageCode, p: crate::messages::Params) -> Error {
    Error::Pptx(Msg::new(code, p))
}

const NS_A: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
const NS_R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const NS_P: &str = "http://schemas.openxmlformats.org/presentationml/2006/main";
const RT_BASE: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/";
const RT_DOC: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument";
const RT_CORE: &str =
    "http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties";

const CT_RELS: &str = "application/vnd.openxmlformats-package.relationships+xml";
const CT_XML: &str = "application/xml";
const CT_PRES: &str =
    "application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml";
const CT_SLIDE: &str = "application/vnd.openxmlformats-officedocument.presentationml.slide+xml";
const CT_LAYOUT: &str =
    "application/vnd.openxmlformats-officedocument.presentationml.slideLayout+xml";
const CT_MASTER: &str =
    "application/vnd.openxmlformats-officedocument.presentationml.slideMaster+xml";
const CT_NOTES: &str =
    "application/vnd.openxmlformats-officedocument.presentationml.notesSlide+xml";
const CT_NOTES_MASTER: &str =
    "application/vnd.openxmlformats-officedocument.presentationml.notesMaster+xml";
const CT_THEME: &str = "application/vnd.openxmlformats-officedocument.theme+xml";
const CT_PRES_PROPS: &str =
    "application/vnd.openxmlformats-officedocument.presentationml.presProps+xml";
const CT_VIEW_PROPS: &str =
    "application/vnd.openxmlformats-officedocument.presentationml.viewProps+xml";
const CT_TABLE_STYLES: &str =
    "application/vnd.openxmlformats-officedocument.presentationml.tableStyles+xml";
const CT_CHART: &str = "application/vnd.openxmlformats-officedocument.drawingml.chart+xml";
const CT_XLSX: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";
const NS_C: &str = "http://schemas.openxmlformats.org/drawingml/2006/chart";
const CT_CORE: &str = "application/vnd.openxmlformats-package.core-properties+xml";
const CT_APP: &str = "application/vnd.openxmlformats-officedocument.extended-properties+xml";

const EMU_PER_PT: i64 = 12700;
const SLIDE_W: i64 = 12192000;
const SLIDE_H: i64 = 6858000;

#[derive(Clone, Copy)]
pub(crate) struct Rect {
    pub(crate) x: i64,
    pub(crate) y: i64,
    pub(crate) w: i64,
    pub(crate) h: i64,
}

fn solid_shape(x: &mut Xml, id: u32, rect: Rect, color: &str) {
    x.open("p:sp", &[]);
    x.open("p:nvSpPr", &[]);
    x.empty(
        "p:cNvPr",
        &[("id", &id.to_string()), ("name", "Theme accent")],
    );
    x.empty("p:cNvSpPr", &[]);
    x.empty("p:nvPr", &[]);
    x.close("p:nvSpPr");
    x.open("p:spPr", &[]);
    write_xfrm(x, rect);
    x.open("a:prstGeom", &[("prst", "rect")]);
    x.empty("a:avLst", &[]);
    x.close("a:prstGeom");
    x.open("a:solidFill", &[]);
    x.empty("a:srgbClr", &[("val", color)]);
    x.close("a:solidFill");
    x.open("a:ln", &[]);
    x.empty("a:noFill", &[]);
    x.close("a:ln");
    x.close("p:spPr");
    x.close("p:sp");
}

// The design grid, in EMU (914400 per inch) on a 13.333 x 7.5 inch slide. The PDF writer
// mirrors the same numbers in points, so both formats show the same layout.
const MARGIN: i64 = 594360; // 0.65 in
const BAR_H: i64 = 128016; // 0.14 in accent bar at the top of content slides
const TITLE_RECT: Rect = Rect {
    x: MARGIN,
    y: 502920, // 0.55 in; keep visible title-to-content spacing within 40 px at 800 px wide
    w: SLIDE_W - 2 * MARGIN,
    h: 914400, // 1.00 in; ends above the body area
};
const BODY_RECT: Rect = Rect {
    x: MARGIN,
    y: 1508760, // 1.65 in
    w: SLIDE_W - 2 * MARGIN,
    h: 4480560, // 4.90 in; the footer sits below
};
const COL_W: i64 = 5334000; // 5.83 in, with a 0.5 in gutter
const BODY1_RECT: Rect = Rect {
    x: MARGIN,
    y: BODY_RECT.y,
    w: COL_W,
    h: BODY_RECT.h,
};
const BODY2_RECT: Rect = Rect {
    x: MARGIN + COL_W + 457200,
    y: BODY_RECT.y,
    w: COL_W,
    h: BODY_RECT.h,
};
const CTR_RECT: Rect = Rect {
    x: 1097280, // 1.20 in
    y: 1554480, // 1.70 in
    w: SLIDE_W - 2 * 1097280,
    h: 2286000, // 2.50 in; two conservative lines at 44 pt
};
const SUB_RECT: Rect = Rect {
    x: 1097280,
    y: 4023360, // 4.40 in
    w: SLIDE_W - 2 * 1097280,
    h: 914400,
};
const SECTION_RECT: Rect = Rect {
    x: 1097280,
    y: 2743200, // 3.00 in
    w: SLIDE_W - 2 * 1097280,
    h: 914400, // 1.00 in; leaves room for the subtitle below
};
const QUOTE_RECT: Rect = Rect {
    x: 1645920, // 1.80 in
    y: 2194560, // 2.40 in
    w: SLIDE_W - 2 * 1645920,
    h: 2011680, // 2.20 in
};
const ATTR_RECT: Rect = Rect {
    y: 5486400,
    h: 548640,
    ..SUB_RECT
};
const FOOTER_RECT: Rect = Rect {
    x: SLIDE_W - MARGIN - 914400,
    y: 6400800, // 7.00 in
    w: 914400,
    h: 320040,
};
const FULL_RECT: Rect = Rect {
    x: 0,
    y: 0,
    w: SLIDE_W,
    h: SLIDE_H,
};

struct Ph {
    kind: &'static str,
    idx: i64,
    rect: Rect,
}

fn layout_phs(layout: Layout) -> Vec<Ph> {
    let title = |kind| Ph {
        kind,
        idx: 0,
        rect: TITLE_RECT,
    };
    match layout {
        Layout::Title => vec![
            Ph {
                kind: "ctrTitle",
                idx: 0,
                rect: CTR_RECT,
            },
            Ph {
                kind: "subTitle",
                idx: 1,
                rect: SUB_RECT,
            },
        ],
        Layout::Quote => vec![Ph {
            kind: "body",
            idx: 1,
            rect: QUOTE_RECT,
        }],
        Layout::TitleContent | Layout::Image | Layout::Table | Layout::Chart => vec![
            title("title"),
            Ph {
                kind: "body",
                idx: 1,
                rect: BODY_RECT,
            },
        ],
        Layout::TwoContent => vec![
            title("title"),
            Ph {
                kind: "body",
                idx: 1,
                rect: BODY1_RECT,
            },
            Ph {
                kind: "body",
                idx: 2,
                rect: BODY2_RECT,
            },
        ],
        Layout::Section => vec![Ph {
            kind: "ctrTitle",
            idx: 0,
            rect: SECTION_RECT,
        }],
    }
}

fn layout_index(layout: Layout) -> usize {
    match layout {
        Layout::Title => 1,
        Layout::TitleContent => 2,
        Layout::TwoContent => 3,
        Layout::Section => 4,
        Layout::Image => 5,
        Layout::Table => 6,
        Layout::Quote => 7,
        Layout::Chart => 8,
    }
}

const LAYOUTS: [Layout; 8] = [
    Layout::Title,
    Layout::TitleContent,
    Layout::TwoContent,
    Layout::Section,
    Layout::Image,
    Layout::Table,
    Layout::Quote,
    Layout::Chart,
];

// -------------------------------------------------------------------------------------------
// Relationships and package

#[derive(Default, Clone)]
struct Rels {
    items: Vec<(String, String, String, bool)>,
}

impl Rels {
    fn add(&mut self, kind: &str, target: &str, external: bool) -> String {
        let ty = if kind.starts_with("http") {
            kind.to_string()
        } else {
            format!("{RT_BASE}{kind}")
        };
        if let Some((id, ..)) = self
            .items
            .iter()
            .find(|(_, t, tg, e)| *t == ty && tg == target && *e == external)
        {
            return id.clone();
        }
        let id = format!("rId{}", self.items.len() + 1);
        self.items
            .push((id.clone(), ty, target.to_string(), external));
        id
    }

    fn to_xml(&self) -> Vec<u8> {
        let mut x = Xml::new();
        x.decl();
        x.open(
            "Relationships",
            &[(
                "xmlns",
                "http://schemas.openxmlformats.org/package/2006/relationships",
            )],
        );
        for (id, ty, target, external) in &self.items {
            let mut attrs = vec![
                ("Id", id.as_str()),
                ("Type", ty.as_str()),
                ("Target", target.as_str()),
            ];
            if *external {
                attrs.push(("TargetMode", "External"));
            }
            x.empty("Relationship", &attrs);
        }
        x.close("Relationships");
        x.finish()
    }
}

fn dir_of(part: &str) -> String {
    let p = part.trim_start_matches('/');
    match p.rsplit_once('/') {
        Some((dir, _)) => dir.to_string(),
        None => String::new(),
    }
}

fn file_of(part: &str) -> &str {
    part.rsplit('/').next().unwrap_or(part)
}

fn rels_path(part: &str) -> String {
    let p = part.trim_start_matches('/');
    let dir = dir_of(p);
    let file = file_of(p);
    if dir.is_empty() {
        format!("_rels/{file}.rels")
    } else {
        format!("{dir}/_rels/{file}.rels")
    }
}

/// Relative target from the directory of `from` to part `to`.
fn rel_path(from: &str, to: &str) -> String {
    let from_dir = dir_of(from);
    let to_dir = dir_of(to);
    let fd: Vec<&str> = from_dir.split('/').filter(|s| !s.is_empty()).collect();
    let td: Vec<&str> = to_dir.split('/').filter(|s| !s.is_empty()).collect();
    let common = fd.iter().zip(td.iter()).take_while(|(a, b)| a == b).count();
    let mut out = String::new();
    for _ in common..fd.len() {
        out.push_str("../");
    }
    for seg in td.iter().skip(common) {
        out.push_str(seg);
        out.push('/');
    }
    out.push_str(file_of(to));
    out
}

/// Resolve a relative relationship target against a base directory into a part name.
fn resolve_part(base: &str, target: &str) -> String {
    let target = target.split('#').next().unwrap_or("");
    let mut segs: Vec<&str> = if target.starts_with('/') {
        Vec::new()
    } else {
        base.split('/').filter(|s| !s.is_empty()).collect()
    };
    for seg in target.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                segs.pop();
            }
            seg => segs.push(seg),
        }
    }
    segs.join("/")
}

/// A package being written.
struct Pkg {
    files: Vec<(String, Vec<u8>)>,
    defaults: Vec<(String, String)>,
    overrides: Vec<(String, String)>,
}

impl Pkg {
    fn new() -> Self {
        Pkg {
            files: Vec::new(),
            defaults: vec![
                ("rels".to_string(), CT_RELS.to_string()),
                ("xml".to_string(), CT_XML.to_string()),
            ],
            overrides: Vec::new(),
        }
    }

    fn xml_part(&mut self, name: &str, content_type: &str, data: Vec<u8>) {
        self.overrides
            .push((format!("/{name}"), content_type.to_string()));
        self.files.push((name.to_string(), data));
    }

    fn binary(&mut self, name: &str, content_type: &str, data: Vec<u8>) {
        let ext = name
            .rsplit('.')
            .next()
            .unwrap_or("bin")
            .to_ascii_lowercase();
        if !self.defaults.iter().any(|(e, _)| *e == ext) {
            self.defaults.push((ext, content_type.to_string()));
        }
        self.files.push((name.to_string(), data));
    }

    fn rels_part(&mut self, part: &str, rels: &Rels) {
        if rels.items.is_empty() {
            return;
        }
        self.files.push((rels_path(part), rels.to_xml()));
    }

    fn finish(self) -> Result<Vec<u8>, Error> {
        let mut content = Xml::new();
        content.decl();
        content.open(
            "Types",
            &[(
                "xmlns",
                "http://schemas.openxmlformats.org/package/2006/content-types",
            )],
        );
        for (ext, ct) in &self.defaults {
            content.empty("Default", &[("Extension", ext), ("ContentType", ct)]);
        }
        for (part, ct) in &self.overrides {
            content.empty("Override", &[("PartName", part), ("ContentType", ct)]);
        }
        content.close("Types");

        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default()
            .last_modified_time(zip::DateTime::default())
            .compression_method(zip::CompressionMethod::Deflated);
        writer
            .start_file("[Content_Types].xml", options)
            .map_err(Error::zip)?;
        writer.write_all(&content.finish()).map_err(Error::zip)?;
        for (name, data) in &self.files {
            writer
                .start_file(name.as_str(), options)
                .map_err(Error::zip)?;
            writer.write_all(data).map_err(Error::zip)?;
        }
        let cursor = writer.finish().map_err(Error::zip)?;
        Ok(cursor.into_inner())
    }
}

// -------------------------------------------------------------------------------------------
// Theme, master, layouts

fn theme_xml(theme: Theme) -> Vec<u8> {
    let mut x = Xml::new();
    x.decl();
    x.open("a:theme", &[("xmlns:a", NS_A), ("name", theme.name)]);
    x.open("a:themeElements", &[]);
    x.open("a:clrScheme", &[("name", theme.name)]);
    for (tag, value) in [
        ("a:dk1", theme.dk1),
        ("a:lt1", theme.lt1),
        ("a:dk2", theme.dk2),
        ("a:lt2", theme.lt2),
    ] {
        x.open(tag, &[]);
        x.empty("a:srgbClr", &[("val", value)]);
        x.close(tag);
    }
    for (i, value) in theme.accent.iter().enumerate() {
        let tag = format!("a:accent{}", i + 1);
        x.open(&tag, &[]);
        x.empty("a:srgbClr", &[("val", value)]);
        x.close(&tag);
    }
    for (tag, value) in [("a:hlink", theme.hlink), ("a:folHlink", theme.fol_hlink)] {
        x.open(tag, &[]);
        x.empty("a:srgbClr", &[("val", value)]);
        x.close(tag);
    }
    x.close("a:clrScheme");

    x.open("a:fontScheme", &[("name", theme.name)]);
    font_block(&mut x, "a:majorFont", theme.major_font);
    font_block(&mut x, "a:minorFont", theme.minor_font);
    x.close("a:fontScheme");

    x.open("a:fmtScheme", &[("name", theme.name)]);
    x.open("a:fillStyleLst", &[]);
    for _ in 0..3 {
        solid_phclr(&mut x);
    }
    x.close("a:fillStyleLst");
    x.open("a:lnStyleLst", &[]);
    for width in [6350, 12700, 19050] {
        let w = width.to_string();
        x.open(
            "a:ln",
            &[
                ("w", w.as_str()),
                ("cap", "flat"),
                ("cmpd", "sng"),
                ("algn", "ctr"),
            ],
        );
        solid_phclr(&mut x);
        x.empty("a:prstDash", &[("val", "solid")]);
        x.empty("a:miter", &[("lim", "800000")]);
        x.close("a:ln");
    }
    x.close("a:lnStyleLst");
    x.open("a:effectStyleLst", &[]);
    for _ in 0..3 {
        x.open("a:effectStyle", &[]);
        x.empty("a:effectLst", &[]);
        x.close("a:effectStyle");
    }
    x.close("a:effectStyleLst");
    x.open("a:bgFillStyleLst", &[]);
    for _ in 0..3 {
        solid_phclr(&mut x);
    }
    x.close("a:bgFillStyleLst");
    x.close("a:fmtScheme");
    x.close("a:themeElements");
    x.empty("a:objectDefaults", &[]);
    x.empty("a:extraClrSchemeLst", &[]);
    x.close("a:theme");
    x.finish()
}

fn font_block(x: &mut Xml, tag: &str, latin: &str) {
    x.open(tag, &[]);
    x.empty("a:latin", &[("typeface", latin)]);
    x.empty("a:ea", &[("typeface", "")]);
    x.empty("a:cs", &[("typeface", "")]);
    x.close(tag);
}

fn solid_phclr(x: &mut Xml) {
    x.open("a:solidFill", &[]);
    x.empty("a:schemeClr", &[("val", "phClr")]);
    x.close("a:solidFill");
}

fn clr_map(x: &mut Xml) {
    x.empty(
        "p:clrMap",
        &[
            ("bg1", "lt1"),
            ("tx1", "dk1"),
            ("bg2", "lt2"),
            ("tx2", "dk2"),
            ("accent1", "accent1"),
            ("accent2", "accent2"),
            ("accent3", "accent3"),
            ("accent4", "accent4"),
            ("accent5", "accent5"),
            ("accent6", "accent6"),
            ("hlink", "hlink"),
            ("folHlink", "folHlink"),
        ],
    );
}

fn tree_open(x: &mut Xml) {
    x.open("p:spTree", &[]);
    x.open("p:nvGrpSpPr", &[]);
    x.empty("p:cNvPr", &[("id", "1"), ("name", "")]);
    x.empty("p:cNvGrpSpPr", &[]);
    x.empty("p:nvPr", &[]);
    x.close("p:nvGrpSpPr");
    x.open("p:grpSpPr", &[]);
    x.open("a:xfrm", &[]);
    x.empty("a:off", &[("x", "0"), ("y", "0")]);
    x.empty("a:ext", &[("cx", "0"), ("cy", "0")]);
    x.empty("a:chOff", &[("x", "0"), ("y", "0")]);
    x.empty("a:chExt", &[("cx", "0"), ("cy", "0")]);
    x.close("a:xfrm");
    x.close("p:grpSpPr");
}

fn empty_ph_shape(x: &mut Xml, id: u32, name: &str, ph: &Ph, anchor: &str) {
    let id_s = id.to_string();
    x.open("p:sp", &[]);
    x.open("p:nvSpPr", &[]);
    x.empty("p:cNvPr", &[("id", id_s.as_str()), ("name", name)]);
    x.open("p:cNvSpPr", &[]);
    x.empty("a:spLocks", &[("noGrp", "1")]);
    x.close("p:cNvSpPr");
    x.open("p:nvPr", &[]);
    write_ph(x, ph.kind, ph.idx);
    x.close("p:nvPr");
    x.close("p:nvSpPr");
    x.open("p:spPr", &[]);
    write_xfrm(x, ph.rect);
    x.open("a:prstGeom", &[("prst", "rect")]);
    x.empty("a:avLst", &[]);
    x.close("a:prstGeom");
    x.close("p:spPr");
    x.open("p:txBody", &[]);
    x.empty("a:bodyPr", &[("anchor", anchor)]);
    x.empty("a:lstStyle", &[]);
    x.empty("a:p", &[]);
    x.close("p:txBody");
    x.close("p:sp");
}

fn master_xml(theme: Theme, layout_rids: &[(String, String)]) -> Vec<u8> {
    let mut x = Xml::new();
    x.decl();
    x.open(
        "p:sldMaster",
        &[("xmlns:a", NS_A), ("xmlns:r", NS_R), ("xmlns:p", NS_P)],
    );
    x.open("p:cSld", &[]);
    x.open("p:bg", &[]);
    x.open("p:bgPr", &[]);
    x.open("a:solidFill", &[]);
    x.empty("a:schemeClr", &[("val", "bg1")]);
    x.close("a:solidFill");
    x.empty("a:effectLst", &[]);
    x.close("p:bgPr");
    x.close("p:bg");
    tree_open(&mut x);
    empty_ph_shape(
        &mut x,
        2,
        "Title Placeholder 1",
        &layout_phs(Layout::Title)[0],
        "ctr",
    );
    empty_ph_shape(
        &mut x,
        3,
        "Text Placeholder 2",
        &layout_phs(Layout::TitleContent)[1],
        "t",
    );
    x.close("p:spTree");
    x.close("p:cSld");
    clr_map(&mut x);
    x.open("p:sldLayoutIdLst", &[]);
    for (id, (rid, _)) in (2147483649u64..).zip(layout_rids.iter()) {
        let id_s = id.to_string();
        x.empty(
            "p:sldLayoutId",
            &[("id", id_s.as_str()), ("r:id", rid.as_str())],
        );
    }
    x.close("p:sldLayoutIdLst");
    x.open("p:txStyles", &[]);
    x.open("p:titleStyle", &[]);
    x.open("a:lvl1pPr", &[("algn", "l")]);
    x.empty("a:defRPr", &[("sz", "3200"), ("b", "1")]);
    x.close("a:lvl1pPr");
    x.close("p:titleStyle");
    x.open("p:bodyStyle", &[]);
    x.open("a:lvl1pPr", &[("marL", "342900"), ("indent", "-342900")]);
    x.empty("a:buFont", &[("typeface", "Arial")]);
    x.empty("a:buChar", &[("char", "•")]);
    x.empty("a:defRPr", &[("sz", "2000")]);
    x.close("a:lvl1pPr");
    x.close("p:bodyStyle");
    x.open("p:otherStyle", &[]);
    x.open("a:lvl1pPr", &[]);
    x.empty("a:defRPr", &[("sz", "1800")]);
    x.close("a:lvl1pPr");
    x.close("p:otherStyle");
    x.close("p:txStyles");
    let _ = theme;
    x.close("p:sldMaster");
    x.finish()
}

fn layout_xml(layout: Layout) -> Vec<u8> {
    let mut x = Xml::new();
    x.decl();
    x.open(
        "p:sldLayout",
        &[
            ("xmlns:a", NS_A),
            ("xmlns:r", NS_R),
            ("xmlns:p", NS_P),
            ("type", layout.ooxml_type()),
            ("preserve", "1"),
        ],
    );
    x.open("p:cSld", &[("name", layout.as_str())]);
    tree_open(&mut x);
    for (i, ph) in layout_phs(layout).iter().enumerate() {
        let anchor = if ph.kind == "ctrTitle" || ph.kind == "subTitle" {
            "ctr"
        } else {
            "t"
        };
        let name = if ph.kind == "ctrTitle" || ph.kind == "title" {
            "Title".to_string()
        } else {
            format!("Body {}", i)
        };
        empty_ph_shape(&mut x, (i + 2) as u32, &name, ph, anchor);
    }
    x.close("p:spTree");
    x.close("p:cSld");
    x.open("p:clrMapOvr", &[]);
    x.empty("a:masterClrMapping", &[]);
    x.close("p:clrMapOvr");
    x.close("p:sldLayout");
    x.finish()
}

fn notes_master_xml() -> Vec<u8> {
    let mut x = Xml::new();
    x.decl();
    x.open(
        "p:notesMaster",
        &[("xmlns:a", NS_A), ("xmlns:r", NS_R), ("xmlns:p", NS_P)],
    );
    x.open("p:cSld", &[]);
    x.open("p:bg", &[]);
    x.open("p:bgPr", &[]);
    x.open("a:solidFill", &[]);
    x.empty("a:schemeClr", &[("val", "bg1")]);
    x.close("a:solidFill");
    x.empty("a:effectLst", &[]);
    x.close("p:bgPr");
    x.close("p:bg");
    tree_open(&mut x);
    let img = Ph {
        kind: "sldImg",
        idx: 0,
        rect: Rect {
            x: 1524000,
            y: 1524000,
            w: 9144000,
            h: 3429000,
        },
    };
    empty_ph_shape(&mut x, 2, "Slide Image Placeholder 1", &img, "ctr");
    let body = Ph {
        kind: "body",
        idx: 1,
        rect: Rect {
            x: 914400,
            y: 5143500,
            w: 10363200,
            h: 1143000,
        },
    };
    empty_ph_shape(&mut x, 3, "Notes Placeholder 2", &body, "t");
    x.close("p:spTree");
    x.close("p:cSld");
    clr_map(&mut x);
    x.open("p:notesStyle", &[]);
    x.open("a:lvl1pPr", &[]);
    x.empty("a:defRPr", &[("sz", "1200")]);
    x.close("a:lvl1pPr");
    x.close("p:notesStyle");
    x.close("p:notesMaster");
    x.finish()
}

fn notes_slide_xml(notes: &str) -> Vec<u8> {
    let mut x = Xml::new();
    x.decl();
    x.open(
        "p:notes",
        &[("xmlns:a", NS_A), ("xmlns:r", NS_R), ("xmlns:p", NS_P)],
    );
    x.open("p:cSld", &[]);
    tree_open(&mut x);
    let img = Ph {
        kind: "sldImg",
        idx: 0,
        rect: Rect {
            x: 0,
            y: 0,
            w: 0,
            h: 0,
        },
    };
    empty_ph_shape(&mut x, 2, "Slide Image Placeholder 1", &img, "ctr");
    let body = Ph {
        kind: "body",
        idx: 1,
        rect: Rect {
            x: 0,
            y: 0,
            w: 0,
            h: 0,
        },
    };
    let paras: Vec<Para> = notes
        .lines()
        .map(|line| Para {
            text: line.to_string(),
            bullet: false,
            level: 0,
            size: 12.0,
            bold: false,
            align: None,
            clr: Clr::Scheme("tx1"),
        })
        .collect();
    let paras = if paras.is_empty() {
        vec![Para {
            text: String::new(),
            bullet: false,
            level: 0,
            size: 12.0,
            bold: false,
            align: None,
            clr: Clr::Scheme("tx1"),
        }]
    } else {
        paras
    };
    text_shape(
        &mut x,
        3,
        &TextBox::ph("Notes Placeholder 2", &body, "t"),
        body.rect,
        &paras,
    );
    let _ = img;
    x.close("p:spTree");
    x.close("p:cSld");
    x.open("p:clrMapOvr", &[]);
    x.empty("a:masterClrMapping", &[]);
    x.close("p:clrMapOvr");
    x.close("p:notes");
    x.finish()
}

fn presentation_xml(
    master_rid: &str,
    slide_rids: &[String],
    notes_master_rid: Option<&str>,
) -> Vec<u8> {
    let mut x = Xml::new();
    x.decl();
    x.open(
        "p:presentation",
        &[
            ("xmlns:a", NS_A),
            ("xmlns:r", NS_R),
            ("xmlns:p", NS_P),
            ("saveSubsetFonts", "1"),
        ],
    );
    x.open("p:sldMasterIdLst", &[]);
    x.empty(
        "p:sldMasterId",
        &[("id", "2147483648"), ("r:id", master_rid)],
    );
    x.close("p:sldMasterIdLst");
    if let Some(rid) = notes_master_rid {
        x.open("p:notesMasterIdLst", &[]);
        x.empty("p:notesMasterId", &[("r:id", rid)]);
        x.close("p:notesMasterIdLst");
    }
    if !slide_rids.is_empty() {
        x.open("p:sldIdLst", &[]);
        for (id, rid) in (256u64..).zip(slide_rids.iter()) {
            let id_s = id.to_string();
            x.empty("p:sldId", &[("id", id_s.as_str()), ("r:id", rid.as_str())]);
        }
        x.close("p:sldIdLst");
    }
    let cx = SLIDE_W.to_string();
    let cy = SLIDE_H.to_string();
    x.empty("p:sldSz", &[("cx", cx.as_str()), ("cy", cy.as_str())]);
    x.empty("p:notesSz", &[("cx", "6858000"), ("cy", "9144000")]);
    x.close("p:presentation");
    x.finish()
}

fn pres_props_xml() -> Vec<u8> {
    let mut x = Xml::new();
    x.decl();
    x.open(
        "p:presentationPr",
        &[("xmlns:a", NS_A), ("xmlns:r", NS_R), ("xmlns:p", NS_P)],
    );
    x.close("p:presentationPr");
    x.finish()
}

fn view_props_xml() -> Vec<u8> {
    let mut x = Xml::new();
    x.decl();
    x.open(
        "p:viewPr",
        &[("xmlns:a", NS_A), ("xmlns:r", NS_R), ("xmlns:p", NS_P)],
    );
    x.open("p:normalViewPr", &[]);
    x.empty("p:restoredLeft", &[("sz", "15620")]);
    x.empty("p:restoredTop", &[("sz", "94660")]);
    x.close("p:normalViewPr");
    x.empty("p:gridSpacing", &[("cx", "76200"), ("cy", "76200")]);
    x.close("p:viewPr");
    x.finish()
}

fn table_styles_xml() -> Vec<u8> {
    let mut x = Xml::new();
    x.decl();
    x.open(
        "a:tblStyleLst",
        &[
            ("xmlns:a", NS_A),
            ("def", "{5C22544A-7EE6-4342-B048-85BDC9FD1C3A}"),
        ],
    );
    x.close("a:tblStyleLst");
    x.finish()
}

fn core_xml(title: &str) -> Vec<u8> {
    let mut x = Xml::new();
    x.decl();
    x.open(
        "cp:coreProperties",
        &[
            (
                "xmlns:cp",
                "http://schemas.openxmlformats.org/package/2006/metadata/core-properties",
            ),
            ("xmlns:dc", "http://purl.org/dc/elements/1.1/"),
            ("xmlns:dcterms", "http://purl.org/dc/terms/"),
            ("xmlns:xsi", "http://www.w3.org/2001/XMLSchema-instance"),
        ],
    );
    x.el("dc:title", &[], title);
    x.el("dc:creator", &[], "slides");
    x.close("cp:coreProperties");
    x.finish()
}

fn app_xml(slides: usize) -> Vec<u8> {
    let mut x = Xml::new();
    x.decl();
    x.open(
        "Properties",
        &[
            (
                "xmlns",
                "http://schemas.openxmlformats.org/officeDocument/2006/extended-properties",
            ),
            (
                "xmlns:vt",
                "http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes",
            ),
        ],
    );
    x.el("Application", &[], "slides");
    x.el("Slides", &[], &slides.to_string());
    x.close("Properties");
    x.finish()
}

// -------------------------------------------------------------------------------------------
// Shapes

#[derive(Clone)]
enum Clr {
    Scheme(&'static str),
    Rgb(&'static str),
}

#[derive(Clone)]
struct Para {
    text: String,
    bullet: bool,
    level: u8,
    size: f64,
    bold: bool,
    align: Option<&'static str>,
    clr: Clr,
}

struct TextBox<'a> {
    name: &'a str,
    ph: Option<(&'static str, i64)>,
    anchor: &'static str,
    txbox: bool,
}

impl<'a> TextBox<'a> {
    fn ph(name: &'a str, ph: &Ph, anchor: &'static str) -> Self {
        TextBox {
            name,
            ph: Some((ph.kind, ph.idx)),
            anchor,
            txbox: false,
        }
    }

    fn plain(name: &'a str) -> Self {
        TextBox {
            name,
            ph: None,
            anchor: "t",
            txbox: true,
        }
    }
}

fn write_ph(x: &mut Xml, kind: &str, idx: i64) {
    let mut attrs = vec![("type", kind)];
    let idx_s = idx.to_string();
    if idx != 0 {
        attrs.push(("idx", idx_s.as_str()));
    }
    x.empty("p:ph", &attrs);
}

fn write_xfrm(x: &mut Xml, rect: Rect) {
    let xs = rect.x.to_string();
    let ys = rect.y.to_string();
    let ws = rect.w.to_string();
    let hs = rect.h.to_string();
    x.open("a:xfrm", &[]);
    x.empty("a:off", &[("x", xs.as_str()), ("y", ys.as_str())]);
    x.empty("a:ext", &[("cx", ws.as_str()), ("cy", hs.as_str())]);
    x.close("a:xfrm");
}

fn write_clr(x: &mut Xml, clr: &Clr) {
    x.open("a:solidFill", &[]);
    match clr {
        Clr::Scheme(v) => x.empty("a:schemeClr", &[("val", v)]),
        Clr::Rgb(v) => x.empty("a:srgbClr", &[("val", v)]),
    }
    x.close("a:solidFill");
}

fn write_run(x: &mut Xml, para: &Para, code: bool) {
    let size = (para.size * 100.0).round() as i64;
    let size_s = size.to_string();
    let mut attrs = vec![("lang", "en-US"), ("sz", size_s.as_str()), ("dirty", "0")];
    if para.bold {
        attrs.push(("b", "1"));
    }
    // Only source line breaks become semantic a:br; Office wraps lines within fixed bounds.
    for (i, line) in para.text.split('\n').enumerate() {
        if i > 0 {
            x.empty("a:br", &[]);
        }
        x.open("a:r", &[]);
        x.open("a:rPr", &attrs);
        write_clr(x, &para.clr);
        x.empty(
            "a:latin",
            &[("typeface", if code { "DejaVu Sans Mono" } else { "+mn-lt" })],
        );
        x.empty("a:ea", &[("typeface", "+mn-ea")]);
        x.empty("a:cs", &[("typeface", "+mn-cs")]);
        x.close("a:rPr");
        x.el("a:t", &[("xml:space", "preserve")], line);
        x.close("a:r");
    }
}

fn write_para(x: &mut Xml, para: &Para, code: bool) {
    let lvl = para.level.to_string();
    let margin = ((i64::from(para.level) + i64::from(para.bullet)) * 228600).to_string();
    let mut attrs = vec![("lvl", lvl.as_str()), ("marL", margin.as_str())];
    if para.bullet {
        attrs.push(("indent", "-171450"));
    }
    if let Some(align) = para.align {
        attrs.push(("algn", align));
    }
    x.open("a:p", &[]);
    x.open("a:pPr", &attrs);
    x.open("a:lnSpc", &[]);
    x.empty("a:spcPct", &[("val", "120000")]);
    x.close("a:lnSpc");
    x.open("a:spcAft", &[]);
    x.empty(
        "a:spcPts",
        &[("val", if para.bullet { "1000" } else { "400" })],
    );
    x.close("a:spcAft");
    if para.bullet {
        x.empty("a:buFont", &[("typeface", "Arial")]);
        x.empty("a:buChar", &[("char", "•")]);
    } else {
        x.empty("a:buNone", &[]);
    }
    x.close("a:pPr");
    write_run(x, para, code);
    x.close("a:p");
}

fn block_metadata(x: &mut Xml, kind: &str) {
    x.open("p:extLst", &[]);
    x.open("p:ext", &[("uri", "urn:slides:block:v1")]);
    x.empty(
        "sb:block",
        &[("xmlns:sb", "urn:slides:block:v1"), ("kind", kind)],
    );
    x.close("p:ext");
    x.close("p:extLst");
}

pub(crate) fn block_kind(shape: &El) -> Option<&str> {
    crate::ooxml::descendants(shape, "p:nvPr")
        .first()?
        .child("p:extLst")?
        .children_named("p:ext")
        .find_map(|ext| ext.child("sb:block"))?
        .attr("kind")
}

/// Retain only our semantic extension; company shape names and other extensions stay theirs.
pub(crate) fn copy_block_metadata(source: &El, destination: &mut El) {
    let metadata = crate::ooxml::descendants(source, "p:nvPr")
        .first()
        .and_then(|nv| nv.child("p:extLst"))
        .and_then(|list| {
            list.children_named("p:ext")
                .find(|ext| ext.child("sb:block").is_some())
        })
        .cloned();
    if let Some(metadata) = metadata {
        destination.visit_mut(|el| {
            if el.name == "p:nvPr" {
                if el.child("p:extLst").is_none() {
                    el.children.push(Node::El(El {
                        spelling: Default::default(),
                        name: "p:extLst".into(),
                        attrs: Vec::new(),
                        children: Vec::new(),
                    }));
                }
                if let Some(list) = el.child_mut("p:extLst") {
                    list.children
                        .retain(|n| !matches!(n, Node::El(ext) if ext.child("sb:block").is_some()));
                    list.children.push(Node::El(metadata.clone()));
                }
                false
            } else {
                true
            }
        });
    }
}

fn text_shape(x: &mut Xml, id: u32, tb: &TextBox<'_>, rect: Rect, paras: &[Para]) {
    let id_s = id.to_string();
    x.open("p:sp", &[]);
    x.open("p:nvSpPr", &[]);
    x.empty("p:cNvPr", &[("id", id_s.as_str()), ("name", tb.name)]);
    if tb.txbox {
        x.open("p:cNvSpPr", &[("txBox", "1")]);
        x.empty("a:spLocks", &[]);
        x.close("p:cNvSpPr");
    } else if tb.ph.is_some() {
        x.open("p:cNvSpPr", &[]);
        x.empty("a:spLocks", &[("noGrp", "1")]);
        x.close("p:cNvSpPr");
    } else {
        x.empty("p:cNvSpPr", &[]);
    }
    x.open("p:nvPr", &[]);
    if let Some((kind, idx)) = tb.ph {
        write_ph(x, kind, idx);
    }
    if tb.name == "Code" {
        block_metadata(x, "code");
    }
    x.close("p:nvPr");
    x.close("p:nvSpPr");
    x.open("p:spPr", &[]);
    write_xfrm(x, rect);
    x.open("a:prstGeom", &[("prst", "rect")]);
    x.empty("a:avLst", &[]);
    x.close("a:prstGeom");
    x.close("p:spPr");
    x.open("p:txBody", &[]);
    x.empty(
        "a:bodyPr",
        &[
            ("wrap", "square"),
            ("anchor", tb.anchor),
            ("lIns", "0"),
            ("rIns", "0"),
            ("tIns", "0"),
            ("bIns", "0"),
        ],
    );
    x.empty("a:lstStyle", &[]);
    if paras.is_empty() {
        x.empty("a:p", &[]);
    } else {
        for para in paras {
            write_para(x, para, tb.name == "Code");
        }
    }
    x.close("p:txBody");
    x.close("p:sp");
}

fn picture_shape(x: &mut Xml, id: u32, rid: &str, rect: Rect) {
    let id_s = id.to_string();
    let name = format!("Picture {id}");
    x.open("p:pic", &[]);
    x.open("p:nvPicPr", &[]);
    x.empty("p:cNvPr", &[("id", id_s.as_str()), ("name", name.as_str())]);
    x.open("p:cNvPicPr", &[]);
    x.empty("a:picLocks", &[("noChangeAspect", "1")]);
    x.close("p:cNvPicPr");
    x.empty("p:nvPr", &[]);
    x.close("p:nvPicPr");
    x.open("p:blipFill", &[]);
    x.empty("a:blip", &[("r:embed", rid)]);
    x.open("a:stretch", &[]);
    x.empty("a:fillRect", &[]);
    x.close("a:stretch");
    x.close("p:blipFill");
    x.open("p:spPr", &[]);
    write_xfrm(x, rect);
    x.open("a:prstGeom", &[("prst", "rect")]);
    x.empty("a:avLst", &[]);
    x.close("a:prstGeom");
    x.close("p:spPr");
    x.close("p:pic");
}

fn table_shape(x: &mut Xml, id: u32, table: &Table, rect: Rect) -> bool {
    let rows = table.rows.len() + usize::from(!table.headers.is_empty());
    let height = (rows as i64 * 584200).min(rect.h);
    let height = if table_fits(table, Rect { h: height, ..rect }) {
        height
    } else {
        rect.h
    };
    let rect = Rect {
        x: rect.x,
        y: rect.y,
        w: rect.w,
        h: height,
    };
    let cols = table
        .headers
        .len()
        .max(table.rows.iter().map(Vec::len).max().unwrap_or(0))
        .max(1);
    let col_w = rect.w / cols as i64;
    let row_h = height / rows.max(1) as i64;
    let mut size: f64 = 20.0;
    let mut fits = true;
    for cell in table.headers.iter().chain(table.rows.iter().flatten()) {
        let paras = text_paras(cell, 20.0);
        let (cell_size, cell_fits) = fit_size(
            Rect {
                w: col_w - 182880,
                h: row_h - 91440,
                ..rect
            },
            &paras,
            20.0,
            14.0,
        );
        size = size.min(cell_size);
        fits &= cell_fits;
    }
    let id_s = id.to_string();
    let name = format!("Table {id}");
    x.open("p:graphicFrame", &[]);
    x.open("p:nvGraphicFramePr", &[]);
    x.empty("p:cNvPr", &[("id", &id_s), ("name", &name)]);
    x.open("p:cNvGraphicFramePr", &[]);
    x.empty("a:graphicFrameLocks", &[("noGrp", "1")]);
    x.close("p:cNvGraphicFramePr");
    x.open("p:nvPr", &[]);
    block_metadata(x, "table");
    x.close("p:nvPr");
    x.close("p:nvGraphicFramePr");
    x.open("p:xfrm", &[]);
    let xs = rect.x.to_string();
    let ys = rect.y.to_string();
    let cx = rect.w.to_string();
    let cy = rect.h.to_string();
    x.empty("a:off", &[("x", xs.as_str()), ("y", ys.as_str())]);
    x.empty("a:ext", &[("cx", cx.as_str()), ("cy", cy.as_str())]);
    x.close("p:xfrm");
    x.open("a:graphic", &[]);
    x.open(
        "a:graphicData",
        &[(
            "uri",
            "http://schemas.openxmlformats.org/drawingml/2006/table",
        )],
    );
    x.open("a:tbl", &[]);
    x.open("a:tblPr", &[("firstRow", "1"), ("bandRow", "1")]);
    x.close("a:tblPr");
    x.open("a:tblGrid", &[]);
    let cw = col_w.to_string();
    for _ in 0..cols {
        x.empty("a:gridCol", &[("w", cw.as_str())]);
    }
    x.close("a:tblGrid");
    if !table.headers.is_empty() {
        table_row(x, &table.headers, cols, true, false, (col_w, row_h), size);
    }
    for (i, row) in table.rows.iter().enumerate() {
        table_row(x, row, cols, false, i % 2 == 0, (col_w, row_h), size);
    }
    if let Some(id) = &table.logical_id {
        x.open("a:extLst", &[]);
        x.open("a:ext", &[("uri", "urn:slides:logical-table:v1")]);
        x.empty(
            "sl:table",
            &[
                ("xmlns:sl", "urn:slides:logical-table:v1"),
                ("id", id),
                ("start", &table.row_start.to_string()),
            ],
        );
        x.close("a:ext");
        x.close("a:extLst");
    }
    x.close("a:tbl");
    x.close("a:graphicData");
    x.close("a:graphic");
    x.close("p:graphicFrame");
    fits
}

fn table_row(
    x: &mut Xml,
    cells: &[String],
    cols: usize,
    header: bool,
    band: bool,
    bounds: (i64, i64),
    size: f64,
) {
    let (_, height) = bounds;
    x.open("a:tr", &[("h", &height.to_string())]);
    for col in 0..cols {
        let text = cells.get(col).map(String::as_str).unwrap_or("");
        let color = if header {
            Clr::Scheme("bg1")
        } else {
            Clr::Scheme("tx1")
        };
        let fill = if header {
            Clr::Scheme("accent1")
        } else if band {
            Clr::Scheme("lt2")
        } else {
            Clr::Scheme("bg1")
        };
        let para = Para {
            text: text.to_string(),
            bullet: false,
            level: 0,
            size,
            bold: header,
            align: None,
            clr: color,
        };
        x.open("a:tc", &[]);
        x.open("a:txBody", &[]);
        x.empty("a:bodyPr", &[]);
        x.empty("a:lstStyle", &[]);
        write_para(x, &para, false);
        x.close("a:txBody");
        x.open(
            "a:tcPr",
            &[
                ("marL", "91440"),
                ("marR", "91440"),
                ("marT", "45720"),
                ("marB", "45720"),
                ("anchor", "ctr"),
            ],
        );
        for tag in ["a:lnL", "a:lnR", "a:lnT", "a:lnB"] {
            x.open(
                tag,
                &[
                    ("w", "12700"),
                    ("cap", "flat"),
                    ("cmpd", "sng"),
                    ("algn", "ctr"),
                ],
            );
            x.open("a:solidFill", &[]);
            x.empty("a:srgbClr", &[("val", "BFBFBF")]);
            x.close("a:solidFill");
            x.empty("a:prstDash", &[("val", "solid")]);
            x.close(tag);
        }
        write_clr(x, &fill);
        x.close("a:tcPr");
        x.close("a:tc");
    }
    x.close("a:tr");
}

// -------------------------------------------------------------------------------------------
// Writing

#[derive(Default)]
struct Media {
    next: u32,
    chart_next: u32,
    by_path: BTreeMap<String, String>,
    parts: Vec<(String, Vec<u8>, &'static str)>,
}

struct Ctx<'a> {
    deck: &'a Deck,
    base_dir: &'a Path,
    media: &'a mut Media,
    warnings: &'a mut Vec<Warning>,
    slide_no: usize,
    position: usize,
    locale: &'a str,
    theme: Theme,
}

fn image_kind(data: &[u8]) -> Option<(&'static str, &'static str)> {
    if data.starts_with(b"\x89PNG") {
        return Some(("png", "image/png"));
    }
    if data.starts_with(&[0xFF, 0xD8]) {
        return Some(("jpeg", "image/jpeg"));
    }
    None
}

fn image_dims(data: &[u8]) -> Option<(u32, u32)> {
    if data.len() >= 24 && data.starts_with(b"\x89PNG") {
        let w = u32::from_be_bytes([data[16], data[17], data[18], data[19]]);
        let h = u32::from_be_bytes([data[20], data[21], data[22], data[23]]);
        return Some((w, h));
    }
    if data.len() < 4 || data[0] != 0xFF || data[1] != 0xD8 {
        return None;
    }
    let mut i = 2;
    while i + 1 < data.len() {
        if data[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = data[i + 1];
        i += 2;
        if marker == 0xD8 || marker == 0xD9 {
            continue;
        }
        if i + 1 >= data.len() {
            return None;
        }
        let len = ((data[i] as usize) << 8) | data[i + 1] as usize;
        if len < 2 || i + len > data.len() {
            return None;
        }
        if (0xC0..=0xCF).contains(&marker) && marker != 0xC4 && marker != 0xC8 && marker != 0xCC {
            if len < 7 {
                return None;
            }
            let h = ((data[i + 3] as u32) << 8) | data[i + 4] as u32;
            let w = ((data[i + 5] as u32) << 8) | data[i + 6] as u32;
            return Some((w, h));
        }
        i += len;
    }
    None
}

fn add_media(ctx: &mut Ctx<'_>, path: &str) -> Option<String> {
    let media = &mut *ctx.media;
    if let Some(part) = media.by_path.get(path) {
        return Some(part.clone());
    }
    let data = match ctx.deck.assets.get(path) {
        Some(bytes) => bytes.clone(),
        None => {
            let _full = match crate::resolve_path(ctx.base_dir, path) {
                Ok(p) => p,
                Err(_) => {
                    ctx.warnings.push(Warning::new(
                        MessageCode::ImageUnsafePath,
                        params(&[("slide", &ctx.slide_no.to_string()), ("path", path)]),
                        ctx.locale,
                    ));
                    return None;
                }
            };
            match crate::read_request(ctx.base_dir, path, &crate::Options::new(ctx.base_dir)) {
                Ok(bytes) => bytes,
                Err(e) => {
                    ctx.warnings.push(Warning::new(
                        MessageCode::ImageUnreadable,
                        params(&[
                            ("slide", &ctx.slide_no.to_string()),
                            ("path", path),
                            ("detail", &e.to_string()),
                        ]),
                        ctx.locale,
                    ));
                    return None;
                }
            }
        }
    };
    let Some((ext, ct)) = image_kind(&data) else {
        ctx.warnings.push(Warning::new(
            MessageCode::ImageUnsupported,
            params(&[("slide", &ctx.slide_no.to_string()), ("path", path)]),
            ctx.locale,
        ));
        return None;
    };
    if let Err(detail) = printpdf::RawImage::decode_from_bytes(&data, &mut Vec::new()) {
        ctx.warnings.push(Warning::new(
            MessageCode::ImageNotEmbedded,
            params(&[
                ("slide", &ctx.slide_no.to_string()),
                ("path", path),
                ("detail", &detail),
            ]),
            ctx.locale,
        ));
        return None;
    }
    if image_dims(&data).is_none_or(|(w, h)| w == 0 || h == 0) {
        ctx.warnings.push(Warning::new(
            MessageCode::ImageNoSize,
            params(&[("slide", &ctx.slide_no.to_string()), ("path", path)]),
            ctx.locale,
        ));
        return None;
    }
    media.next += 1;
    let part = format!("ppt/media/image{}.{ext}", media.next);
    media.parts.push((part.clone(), data, ct));
    media.by_path.insert(path.to_string(), part.clone());
    Some(part)
}

// Conservative advances reserve capacity for font substitutions and wide glyphs.
// Measurement must not inject semantic breaks into the serialized source text.
pub(crate) fn wrap_text(text: &str, rect: Rect, size: f64, level: u8) -> String {
    let width = (rect.w - i64::from(level) * 228600).max(1) as f64 / EMU_PER_PT as f64;
    let limit = ((width - size) / size).max(1.0);
    let mut lines = Vec::new();
    for line in text.split('\n') {
        let mut rest = line;
        loop {
            let mut used = 0.0;
            let end = rest.char_indices().find_map(|(i, c)| {
                used += if c.is_whitespace() {
                    0.4
                } else if "MW@%mw".contains(c) {
                    1.0
                } else if c.is_ascii() || ('\u{00c0}'..='\u{024f}').contains(&c) {
                    0.72
                } else {
                    1.15
                };
                (used > limit && i > 0).then_some(i)
            });
            let Some(end) = end else {
                break;
            };
            let boundary = rest[..end]
                .rfind(char::is_whitespace)
                .filter(|i| *i > 0)
                .map(|i| i + rest[i..].chars().next().map(char::len_utf8).unwrap_or(0))
                .unwrap_or(end);
            lines.push(rest[..boundary].to_string());
            rest = &rest[boundary..];
        }
        lines.push(rest.to_string());
    }
    lines.join("\n")
}

fn fit_size(rect: Rect, paras: &[Para], start: f64, min: f64) -> (f64, bool) {
    let height_pt = rect.h as f64 / EMU_PER_PT as f64;
    let mut size = start;
    while size >= min - 0.01 {
        let needed: f64 = paras
            .iter()
            .map(|p| {
                let lines = wrap_text(&p.text, rect, size, p.level).split('\n').count();
                // Office applies 120% spacing to the font's intrinsic line height, not its
                // nominal point size. Reserve room for Noto/DejaVu substitutions as well.
                lines as f64 * size * 1.7 + if p.bullet { 10.0 } else { 4.0 }
            })
            .sum();
        if needed <= height_pt && rect.w as f64 / EMU_PER_PT as f64 >= size * 2.0 {
            return (size, true);
        }
        size -= 2.0;
    }
    (min, false)
}

fn table_fits(table: &Table, rect: Rect) -> bool {
    let rows = table.rows.len() + usize::from(!table.headers.is_empty());
    let cols = table
        .headers
        .len()
        .max(table.rows.iter().map(Vec::len).max().unwrap_or(0))
        .max(1);
    let cell = Rect {
        w: rect.w / cols as i64 - 182880,
        h: rect.h / rows.max(1) as i64 - 91440,
        ..rect
    };
    table
        .headers
        .iter()
        .chain(table.rows.iter().flatten())
        .all(|s| fit_size(cell, &text_paras(s, 14.0), 14.0, 14.0).1)
}

fn fit_image(rect: Rect, width: u32, height: u32) -> Rect {
    if width == 0 || height == 0 {
        return rect;
    }
    let target = rect.w as f64 / rect.h as f64;
    let source = width as f64 / height as f64;
    if source > target {
        let h = (rect.w as f64 / source) as i64;
        Rect {
            x: rect.x,
            y: rect.y + (rect.h - h) / 2,
            w: rect.w,
            h,
        }
    } else {
        let w = (rect.h as f64 * source) as i64;
        Rect {
            x: rect.x + (rect.w - w) / 2,
            y: rect.y,
            w,
            h: rect.h,
        }
    }
}

fn bullet_paras(items: &[String], size: f64) -> Vec<Para> {
    items
        .iter()
        .map(|item| Para {
            text: item.trim_start().to_string(),
            bullet: true,
            level: (item.len().saturating_sub(item.trim_start().len()) / 2).min(8) as u8,
            size,
            bold: false,
            align: None,
            clr: Clr::Scheme("tx1"),
        })
        .collect()
}

fn text_paras(text: &str, size: f64) -> Vec<Para> {
    text.lines()
        .map(|line| Para {
            text: line.to_string(),
            bullet: false,
            level: 0,
            size,
            bold: false,
            align: None,
            clr: Clr::Scheme("tx1"),
        })
        .collect()
}

fn write_block(
    x: &mut Xml,
    ctx: &mut Ctx<'_>,
    rels: &mut Rels,
    id: &mut u32,
    block: &Block,
    rect: Rect,
    ph: Option<(&'static str, i64)>,
) -> Result<(), Error> {
    let slide_part = format!("ppt/slides/slide{}.xml", ctx.slide_no);
    match block {
        Block::Bullets(_) | Block::Text(_) | Block::Code(_) => {
            let mut paras = match block {
                Block::Bullets(items) => bullet_paras(items, 24.0),
                Block::Text(text) => text_paras(text, 24.0),
                Block::Code(text) => text
                    .split('\n')
                    .map(|line| Para {
                        text: line.into(),
                        bullet: false,
                        level: 0,
                        size: 24.0,
                        bold: false,
                        align: None,
                        clr: Clr::Scheme("tx1"),
                    })
                    .collect(),
                _ => Vec::new(),
            };
            let (size, fits) = fit_size(rect, &paras, 24.0, 14.0);
            for para in &mut paras {
                para.size = size;
            }
            if !fits {
                ctx.warnings.push(Warning::new(
                    MessageCode::SlideTextOverflow,
                    params(&[("slide", &ctx.slide_no.to_string())]),
                    ctx.locale,
                ));
            }
            let tb = TextBox {
                name: if matches!(block, Block::Code(_)) {
                    "Code"
                } else {
                    "Content Placeholder"
                },
                ph,
                anchor: "t",
                txbox: ph.is_none(),
            };
            text_shape(x, *id, &tb, rect, &paras);
            *id += 1;
        }
        Block::Table(table) => {
            if !table_shape(x, *id, table, rect) {
                ctx.warnings.push(Warning::new(
                    MessageCode::SlideTextOverflow,
                    params(&[("slide", &ctx.slide_no.to_string())]),
                    ctx.locale,
                ));
            }
            *id += 1;
        }
        Block::Image(path) => {
            if let Some(part) = add_media(ctx, path) {
                let target = rel_path(&slide_part, &part);
                let rid = rels.add("image", &target, false);
                let dims = ctx
                    .media
                    .parts
                    .iter()
                    .find(|(p, _, _)| p == &part)
                    .and_then(|(_, d, _)| image_dims(d));
                let rect = match dims {
                    Some((w, h)) => fit_image(rect, w, h),
                    None => rect,
                };
                picture_shape(x, *id, &rid, rect);
                *id += 1;
            }
        }
        Block::Chart(chart) => {
            ctx.media.chart_next += 1;
            let n = ctx.media.chart_next;
            let part = format!("ppt/charts/chart{n}.xml");
            let workbook = format!("ppt/embeddings/chart{n}.xlsx");
            ctx.media
                .parts
                .push((part.clone(), crate::charts::xml(chart, ctx.theme), CT_CHART));
            ctx.media
                .parts
                .push((workbook.clone(), crate::charts::workbook(chart)?, CT_XLSX));
            let mut cr = Rels::default();
            cr.add("package", &rel_path(&part, &workbook), false);
            ctx.media
                .parts
                .push((rels_path(&part), cr.to_xml(), CT_RELS));
            let rid = rels.add("chart", &rel_path(&slide_part, &part), false);
            x.open("p:graphicFrame", &[]);
            x.open("p:nvGraphicFramePr", &[]);
            x.empty("p:cNvPr", &[("id", &id.to_string()), ("name", "Chart")]);
            x.empty("p:cNvGraphicFramePr", &[]);
            x.open("p:nvPr", &[]);
            block_metadata(x, "chart");
            x.close("p:nvPr");
            x.close("p:nvGraphicFramePr");
            x.open("p:xfrm", &[]);
            x.empty(
                "a:off",
                &[("x", &rect.x.to_string()), ("y", &rect.y.to_string())],
            );
            x.empty(
                "a:ext",
                &[("cx", &rect.w.to_string()), ("cy", &rect.h.to_string())],
            );
            x.close("p:xfrm");
            x.open("a:graphic", &[]);
            x.open("a:graphicData", &[("uri", NS_C)]);
            x.empty("c:chart", &[("xmlns:c", NS_C), ("r:id", &rid)]);
            x.close("a:graphicData");
            x.close("a:graphic");
            x.close("p:graphicFrame");
            *id += 1;
        }
    }
    Ok(())
}

// A short, single-list slide gets a bounded tinted panel instead of a sparse floating list.
// Dense or mixed-content slides retain the normal grid. The text starts at the same height.
fn short_bullet_panel(
    x: &mut Xml,
    id: &mut u32,
    slide: &Slide,
    block: &Block,
    rect: Rect,
    theme: Theme,
) -> Rect {
    let Block::Bullets(items) = block else {
        return rect;
    };
    if !slide.short_slide_accent
        || slide.blocks.len() != 1
        || !matches!(slide.layout, Layout::TitleContent)
        || !(1..=3).contains(&items.len())
    {
        return rect;
    }
    let inner = Rect {
        x: rect.x + 365760,
        w: rect.w - 548640,
        ..rect
    };
    let paras = bullet_paras(items, 24.0);
    let height = paras
        .iter()
        .map(|p| {
            wrap_text(&p.text, inner, 24.0, p.level).lines().count() as f64 * 24.0 * 1.7 + 10.0
        })
        .sum::<f64>();
    let panel_h = ((height + 28.0) * EMU_PER_PT as f64) as i64;
    if panel_h > rect.h {
        return rect;
    }
    let panel = Rect {
        y: rect.y - 127000,
        h: panel_h,
        ..rect
    };
    solid_shape(x, *id, panel, theme.lt2);
    solid_shape(x, *id + 1, Rect { w: 91440, ..panel }, theme.accent[0]);
    *id += 2;
    inner
}

fn build_slide(slide: &Slide, ctx: &mut Ctx<'_>) -> Result<(Vec<u8>, Rels), Error> {
    let source_top = crate::templates::source_top(&slide.sources, SLIDE_W, SLIDE_H);
    if source_top <= BODY_RECT.y + 914400 {
        return Err(pptx_error(
            MessageCode::SlideCannotFit,
            params(&[("slide", &ctx.slide_no.to_string())]),
        ));
    }
    let reserve = |rect: Rect| Rect {
        h: rect.h.min(source_top - 91440 - rect.y).max(0),
        ..rect
    };
    let body_rect = reserve(BODY_RECT);
    let body1_rect = reserve(BODY1_RECT);
    let body2_rect = reserve(BODY2_RECT);
    let quote_rect = reserve(QUOTE_RECT);
    let attr_rect = reserve(ATTR_RECT);
    let mut x = Xml::new();
    x.decl();
    x.open(
        "p:sld",
        &[("xmlns:a", NS_A), ("xmlns:r", NS_R), ("xmlns:p", NS_P)],
    );
    x.open("p:cSld", &[]);
    let hero = matches!(slide.layout, Layout::Title | Layout::Section) && slide.blocks.is_empty();
    let background = if hero && ctx.theme.name != "Dark" {
        ctx.theme.accent[0]
    } else {
        ctx.theme.lt1
    };
    x.open("p:bg", &[]);
    x.open("p:bgPr", &[]);
    x.open("a:solidFill", &[]);
    x.empty("a:srgbClr", &[("val", background)]);
    x.close("a:solidFill");
    x.empty("a:effectLst", &[]);
    x.close("p:bgPr");
    x.close("p:bg");
    tree_open(&mut x);
    let mut rels = Rels::default();
    let mut id = 2u32;
    let bar = if hero {
        Rect {
            x: 1097280,
            y: 1676400,
            w: 9997440,
            h: BAR_H,
        }
    } else {
        Rect {
            h: BAR_H,
            ..FULL_RECT
        }
    };
    solid_shape(
        &mut x,
        id,
        bar,
        if hero {
            ctx.theme.on_accent_muted
        } else {
            ctx.theme.accent[0]
        },
    );
    id += 1;
    let mut phs = layout_phs(slide.layout);
    if matches!(slide.layout, Layout::Title | Layout::Section) && !slide.blocks.is_empty() {
        for ph in &mut phs {
            if ph.kind == "ctrTitle" {
                ph.rect = reserve(TITLE_RECT);
            }
        }
    }
    for ph in &mut phs {
        ph.rect = reserve(ph.rect);
    }
    if !slide.title.is_empty()
        && let Some(ph) = phs
            .iter()
            .find(|p| p.kind == "title" || p.kind == "ctrTitle")
    {
        let align = if ph.kind == "ctrTitle" {
            Some("ctr")
        } else {
            None
        };
        let para = Para {
            text: slide.title.clone(),
            bullet: false,
            level: 0,
            size: if hero { 44.0 } else { 34.0 },
            bold: true,
            align,
            clr: if hero {
                Clr::Rgb(ctx.theme.on_accent)
            } else {
                Clr::Scheme("tx1")
            },
        };
        // Explicit title breaks are paragraphs; Office handles visual soft wrapping.
        let mut paras: Vec<_> = slide
            .title
            .split('\n')
            .map(|line| Para {
                text: line.into(),
                ..para.clone()
            })
            .collect();
        let (size, fits) = fit_size(ph.rect, &paras, para.size, if hero { 40.0 } else { 14.0 });
        for p in &mut paras {
            p.size = size;
        }
        if !fits {
            ctx.warnings.push(Warning::new(
                MessageCode::SlideTitleOverflow,
                params(&[("slide", &ctx.slide_no.to_string())]),
                ctx.locale,
            ));
        }
        let anchor = if ph.kind == "ctrTitle" { "ctr" } else { "t" };
        let tb = TextBox::ph("Title", ph, anchor);
        text_shape(&mut x, id, &tb, ph.rect, &paras);
        id += 1;
    }
    if !slide.subtitle.is_empty() {
        let mut para = Para {
            text: slide.subtitle.clone(),
            bullet: false,
            level: 0,
            size: 24.0,
            bold: false,
            align: Some("ctr"),
            clr: if hero {
                Clr::Rgb(ctx.theme.on_accent_muted)
            } else {
                Clr::Scheme("tx1")
            },
        };
        let tb = TextBox {
            name: "Subtitle",
            ph: Some(("subTitle", 1)),
            anchor: "ctr",
            txbox: false,
        };
        let (size, fits) = fit_size(reserve(SUB_RECT), std::slice::from_ref(&para), 24.0, 14.0);
        para.size = size;
        if !fits {
            ctx.warnings.push(Warning::new(
                MessageCode::SlideTextOverflow,
                params(&[("slide", &ctx.slide_no.to_string())]),
                ctx.locale,
            ));
        }
        text_shape(&mut x, id, &tb, reserve(SUB_RECT), &[para]);
        id += 1;
    }
    let mut blocks = slide.blocks.clone();
    if slide.layout == Layout::TwoContent
        && let [Block::Bullets(items)] = blocks.as_slice()
    {
        let split = items.len().div_ceil(2);
        blocks = vec![
            Block::Bullets(items[..split].to_vec()),
            Block::Bullets(items[split..].to_vec()),
        ];
    }
    if slide.layout == Layout::Image {
        blocks.sort_by_key(|b| !matches!(b, Block::Image(_)));
    }
    if slide.layout == Layout::Quote {
        let quote = blocks
            .iter()
            .map(crate::block_text)
            .collect::<Vec<_>>()
            .join("\n");
        let text = if quote.is_empty() {
            slide.title.clone()
        } else {
            quote
        };
        let mut paras = text_paras(&text, 36.0);
        let (size, fits) = fit_size(quote_rect, &paras, 36.0, 14.0);
        for p in &mut paras {
            p.size = size;
            p.align = Some("ctr");
            p.clr = Clr::Scheme("accent1");
        }
        if !fits {
            ctx.warnings.push(Warning::new(
                MessageCode::SlideTextOverflow,
                params(&[("slide", &ctx.slide_no.to_string())]),
                ctx.locale,
            ));
        }
        text_shape(&mut x, id, &TextBox::plain("Quote"), quote_rect, &paras);
        id += 1;
        if !blocks.is_empty() {
            let mut paras = text_paras(&slide.title, 20.0);
            let (size, fits) = fit_size(attr_rect, &paras, 20.0, 14.0);
            if !fits {
                ctx.warnings.push(Warning::new(
                    MessageCode::SlideTitleOverflow,
                    params(&[("slide", &ctx.slide_no.to_string())]),
                    ctx.locale,
                ));
            }
            for p in &mut paras {
                p.align = Some("ctr");
                p.size = size;
            }
            let attribution = TextBox {
                name: "Attribution",
                ph: Some(("title", 0)),
                anchor: "ctr",
                txbox: false,
            };
            text_shape(&mut x, id, &attribution, attr_rect, &paras);
            id += 1;
        }
    } else {
        for (i, block) in blocks.iter().enumerate() {
            let (area, slot, count) = if slide.layout == Layout::Image
                && blocks.iter().any(|b| matches!(b, Block::Image(_)))
            {
                let images = blocks
                    .iter()
                    .filter(|b| matches!(b, Block::Image(_)))
                    .count();
                let image_h = if images == blocks.len() {
                    body_rect.h
                } else {
                    body_rect.h * 4 / 5
                };
                if i < images {
                    (
                        Rect {
                            h: image_h,
                            ..body_rect
                        },
                        i,
                        images,
                    )
                } else {
                    (
                        Rect {
                            y: body_rect.y + image_h,
                            h: body_rect.h - image_h,
                            ..body_rect
                        },
                        i - images,
                        blocks.len() - images,
                    )
                }
            } else if slide.layout == Layout::TwoContent {
                let col = i % 2;
                (
                    if col == 0 { body1_rect } else { body2_rect },
                    i / 2,
                    (blocks.len() + 1 - col) / 2,
                )
            } else {
                (body_rect, i, blocks.len())
            };
            let slice = area.h / count.max(1) as i64;
            let rect = Rect {
                y: area.y + slot as i64 * slice,
                h: slice,
                ..area
            };
            let ph = if i == 0 { Some(("body", 1)) } else { None };
            let rect = short_bullet_panel(&mut x, &mut id, slide, block, rect, ctx.theme);
            write_block(&mut x, ctx, &mut rels, &mut id, block, rect, ph)?;
        }
    }
    let footer = Para {
        text: ctx.position.to_string(),
        bullet: false,
        level: 0,
        size: 11.0,
        bold: false,
        align: Some("r"),
        clr: if hero {
            Clr::Rgb(ctx.theme.on_accent_muted)
        } else {
            Clr::Scheme("tx2")
        },
    };
    let footer_box = TextBox {
        name: "Slide number",
        ph: Some(("sldNum", 10)),
        anchor: "ctr",
        txbox: false,
    };
    text_shape(&mut x, id, &footer_box, FOOTER_RECT, &[footer]);
    x.close("p:spTree");
    x.close("p:cSld");
    x.open("p:clrMapOvr", &[]);
    x.empty("a:masterClrMapping", &[]);
    x.close("p:clrMapOvr");
    x.close("p:sld");
    if slide
        .sources
        .iter()
        .map(|s| s.chars().count().max(1).div_ceil(150))
        .sum::<usize>()
        > 3
    {
        ctx.warnings.push(Warning::new(
            MessageCode::SourceOverflow,
            params(&[("slide", &ctx.slide_no.to_string())]),
            ctx.locale,
        ));
    }
    Ok((
        crate::templates::footnote(x.finish(), &slide.sources, SLIDE_W, SLIDE_H)?,
        rels,
    ))
}

/// Paginate before writing either format, using the final renderer for trial layout.
pub(crate) fn paginate(
    deck: Deck,
    theme: Theme,
    base_dir: &Path,
    locale: &str,
) -> Result<(Deck, Vec<Warning>), Error> {
    paginate_using(deck, theme, base_dir, locale, |_, _| Ok(Vec::new()))
}

/// The company writer supplies its final placeholder fitting for the same split loop.
pub(crate) fn paginate_using(
    deck: Deck,
    theme: Theme,
    base_dir: &Path,
    locale: &str,
    mut final_layout: impl FnMut(&Slide, &[u8]) -> Result<Vec<Warning>, Error>,
) -> Result<(Deck, Vec<Warning>), Error> {
    let mut out = Deck {
        slides: Vec::new(),
        assets: deck.assets.clone(),
    };
    let mut warnings = Vec::new();
    for (source, original) in deck.slides.iter().enumerate() {
        let start = out.slides.len();
        let mut original = original.clone();
        for (block, content) in original.blocks.iter_mut().enumerate() {
            if let Block::Table(table) = content
                && table.logical_id.is_none()
            {
                table.logical_id = Some(format!("slide{source}-table{block}"));
                table.row_start = 0;
            }
        }
        let mut pending = vec![original];
        let mut initial = true;
        while let Some(mut slide) = pending.pop() {
            if (!matches!(slide.layout, Layout::Title | Layout::Section)
                || !slide.blocks.is_empty())
                && !slide.subtitle.is_empty()
            {
                slide
                    .blocks
                    .insert(0, Block::Text(std::mem::take(&mut slide.subtitle)));
            }
            let mut media = Media::default();
            let mut trial_warnings = Vec::new();
            let mut ctx = Ctx {
                deck: &deck,
                base_dir,
                media: &mut media,
                warnings: &mut trial_warnings,
                slide_no: source + 1,
                position: source + 1,
                locale,
                theme,
            };
            let (xml, _) = build_slide(&slide, &mut ctx)?;
            for warning in final_layout(&slide, &xml)? {
                trial_warnings.push(Warning::new(
                    if warning.code == MessageCode::SlideTitleOverflow.as_str() {
                        MessageCode::SlideTitleOverflow
                    } else {
                        MessageCode::SlideTextOverflow
                    },
                    params(&[("slide", &(source + 1).to_string())]),
                    locale,
                ));
            }
            let overflow: Vec<_> = trial_warnings
                .into_iter()
                .filter(|w| {
                    w.code == MessageCode::SlideTextOverflow.as_str()
                        || w.code == MessageCode::SlideTitleOverflow.as_str()
                })
                .collect();
            if initial {
                warnings.extend(overflow.clone());
                initial = false;
            }
            if overflow.is_empty() {
                out.slides.push(slide);
                continue;
            }
            let title_overflow = overflow
                .iter()
                .any(|w| w.code == MessageCode::SlideTitleOverflow.as_str())
                || (slide.layout == Layout::Quote && slide.blocks.is_empty());
            let Some((first, second)) = split_slide(slide, title_overflow) else {
                return Err(pptx_error(
                    MessageCode::SlideCannotFit,
                    params(&[("slide", &(source + 1).to_string())]),
                ));
            };
            // Each split consumes text or reduces the block/row count. Unsplittable
            // content returns an error rather than relying on an arbitrary page limit.
            pending.push(second);
            pending.push(first);
        }
        let count = out.slides.len() - start;
        if count > 1 {
            warnings.push(Warning::new(
                MessageCode::SlideSplit,
                params(&[
                    ("slide", &(source + 1).to_string()),
                    ("count", &count.to_string()),
                ]),
                locale,
            ));
        }
    }
    Ok((out, warnings))
}

fn bisect(text: &str) -> Option<(String, String)> {
    let n = text.chars().count();
    if n < 2 {
        return None;
    }
    let middle = text.char_indices().nth(n / 2)?.0;
    let boundary = text[..middle]
        .rfind('\n')
        .map(|i| i + 1)
        .filter(|i| *i > middle / 2)
        .or_else(|| {
            text[..middle]
                .rfind(' ')
                .map(|i| i + 1)
                .filter(|i| *i > middle / 2)
        })
        .unwrap_or(middle);
    Some((text[..boundary].to_string(), text[boundary..].to_string()))
}

fn fitting_prefix(text: &str, rect: Rect, bullet: bool) -> Option<(String, String)> {
    let offsets: Vec<_> = text
        .char_indices()
        .map(|(i, _)| i)
        .chain(std::iter::once(text.len()))
        .collect();
    let (mut lo, mut hi) = (0usize, offsets.len().saturating_sub(1));
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        let paras = if bullet {
            bullet_paras(&[text[..offsets[mid]].to_string()], 14.0)
        } else {
            text_paras(&text[..offsets[mid]], 14.0)
        };
        if fit_size(rect, &paras, 14.0, 14.0).1 {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    let end = *offsets.get(lo)?;
    if end == 0 || end == text.len() {
        return bisect(text);
    }
    let end = text[..end]
        .rfind('\n')
        .or_else(|| text[..end].rfind(char::is_whitespace))
        .map(|i| i + text[i..].chars().next().map(char::len_utf8).unwrap_or(0))
        .filter(|i| *i > end / 2)
        .unwrap_or(end);
    Some((text[..end].to_string(), text[end..].to_string()))
}

fn split_slide(mut first: Slide, title: bool) -> Option<(Slide, Slide)> {
    let mut second = first.clone();
    second.title = format!(
        "{} (cont.)",
        first.title.strip_suffix(" (cont.)").unwrap_or(&first.title)
    );
    second.notes.clear();
    if title {
        let (a, b) = bisect(first.title.strip_suffix(" (cont.)").unwrap_or(&first.title))?;
        first.title = a;
        second.title = format!("{b} (cont.)");
        first.blocks.clear();
        first.subtitle.clear();
    } else if !first.subtitle.is_empty() {
        let (a, b) = bisect(&first.subtitle)?;
        first.subtitle = a;
        second.subtitle = b;
        first.blocks.clear();
    } else if first.blocks.len() > 1 {
        let middle = first.blocks.len().div_ceil(2);
        second.blocks = first.blocks.split_off(middle);
    } else {
        let block = first.blocks.first()?;
        let rect = match first.layout {
            Layout::TwoContent => BODY1_RECT,
            Layout::Quote => QUOTE_RECT,
            _ => BODY_RECT,
        };
        let (a, b) = match block {
            Block::Text(s) | Block::Code(s) => {
                let (a, b) = fitting_prefix(s, rect, false)?;
                if matches!(block, Block::Code(_)) {
                    (Block::Code(a), Block::Code(b))
                } else {
                    (Block::Text(a), Block::Text(b))
                }
            }
            Block::Bullets(items) if items.len() > 1 => {
                let middle = items.len().div_ceil(2);
                (
                    Block::Bullets(items[..middle].to_vec()),
                    Block::Bullets(items[middle..].to_vec()),
                )
            }
            Block::Bullets(items) => {
                let (a, b) = fitting_prefix(items.first()?, rect, true)?;
                (Block::Bullets(vec![a]), Block::Bullets(vec![b]))
            }
            Block::Table(table) if table.rows.len() > 1 => {
                let middle = table.rows.len().div_ceil(2);
                (
                    Block::Table(Table {
                        headers: table.headers.clone(),
                        rows: table.rows[..middle].to_vec(),
                        logical_id: table.logical_id.clone(),
                        row_start: table.row_start,
                    }),
                    Block::Table(Table {
                        headers: table.headers.clone(),
                        rows: table.rows[middle..].to_vec(),
                        logical_id: table.logical_id.clone(),
                        row_start: table.row_start.checked_add(middle)?,
                    }),
                )
            }
            Block::Table(table) => {
                let mut a = table.clone();
                let mut b = table.clone();
                let row = table.rows.first()?;
                let mut changed = false;
                for (i, cell) in row.iter().enumerate() {
                    if let Some((left, right)) = bisect(cell) {
                        a.rows[0][i] = left;
                        b.rows[0][i] = right;
                        changed = true;
                    } else {
                        b.rows[0][i].clear();
                    }
                }
                if !changed {
                    return None;
                }
                (Block::Table(a), Block::Table(b))
            }
            _ => return None,
        };
        first.blocks = vec![a];
        second.blocks = vec![b];
    }
    Some((first, second))
}

/// Write a deck to a PPTX package.
pub fn write(
    deck: &Deck,
    theme: Theme,
    base_dir: &Path,
    locale: &str,
) -> Result<(Vec<u8>, Vec<Warning>), Error> {
    let mut warnings = Vec::new();
    let mut media = Media::default();
    let mut pkg = Pkg::new();

    pkg.xml_part("ppt/theme/theme1.xml", CT_THEME, theme_xml(theme));

    let mut layout_rids: Vec<(String, String)> = Vec::new();
    let mut master_rels = Rels::default();
    for (i, layout) in LAYOUTS.iter().enumerate() {
        let name = format!("ppt/slideLayouts/slideLayout{}.xml", i + 1);
        pkg.xml_part(&name, CT_LAYOUT, layout_xml(*layout));
        let mut rels = Rels::default();
        rels.add("slideMaster", "../slideMasters/slideMaster1.xml", false);
        pkg.rels_part(&name, &rels);
        let rid = master_rels.add(
            "slideLayout",
            &format!("../slideLayouts/slideLayout{}.xml", i + 1),
            false,
        );
        layout_rids.push((rid, layout.as_str().to_string()));
    }
    master_rels.add("theme", "../theme/theme1.xml", false);
    pkg.xml_part(
        "ppt/slideMasters/slideMaster1.xml",
        CT_MASTER,
        master_xml(theme, &layout_rids),
    );
    pkg.rels_part("ppt/slideMasters/slideMaster1.xml", &master_rels);

    let mut slide_parts: Vec<String> = Vec::new();
    let mut notes_parts: Vec<(String, String, String)> = Vec::new();
    for (i, slide) in deck.slides.iter().enumerate() {
        let slide_no = i + 1;
        for (element, block) in slide.blocks.iter().enumerate() {
            if let Block::Chart(chart) = block
                && (chart.labels.len().saturating_mul(chart.series.len()) > 20
                    || chart
                        .series
                        .iter()
                        .flat_map(|s| &s.values)
                        .any(|v| v.to_string().len() > 16))
            {
                warnings.push(Warning::new(
                    MessageCode::ChartLabelReview,
                    params(&[
                        ("slide", &slide_no.to_string()),
                        ("element", &format!("blocks/{element}/chart")),
                    ]),
                    locale,
                ));
            }
        }
        let name = format!("ppt/slides/slide{}.xml", slide_no);
        let layout_part = format!(
            "ppt/slideLayouts/slideLayout{}.xml",
            layout_index(slide.layout)
        );
        let mut ctx = Ctx {
            deck,
            base_dir,
            media: &mut media,
            warnings: &mut warnings,
            slide_no,
            position: slide_no,
            locale,
            theme,
        };
        let (xml, mut rels) = build_slide(slide, &mut ctx)?;
        rels.add("slideLayout", &rel_path(&name, &layout_part), false);
        if !slide.notes.is_empty() {
            let notes_name = format!("ppt/notesSlides/notesSlide{}.xml", slide_no);
            rels.add("notesSlide", &rel_path(&name, &notes_name), false);
            notes_parts.push((name.clone(), notes_name, slide.notes.clone()));
        }
        pkg.xml_part(&name, CT_SLIDE, xml);
        pkg.rels_part(&name, &rels);
        slide_parts.push(name);
    }

    let mut notes_master_rid = None;
    let mut pres_rels = Rels::default();
    let master_rid = pres_rels.add("slideMaster", "slideMasters/slideMaster1.xml", false);
    pres_rels.add("theme", "theme/theme1.xml", false);
    pres_rels.add("presProps", "presProps.xml", false);
    pres_rels.add("viewProps", "viewProps.xml", false);
    pres_rels.add("tableStyles", "tableStyles.xml", false);
    let mut slide_rids = Vec::new();
    for name in &slide_parts {
        slide_rids.push(pres_rels.add("slide", &rel_path("ppt/presentation.xml", name), false));
    }
    if !notes_parts.is_empty() {
        pkg.xml_part(
            "ppt/notesMasters/notesMaster1.xml",
            CT_NOTES_MASTER,
            notes_master_xml(),
        );
        let mut rels = Rels::default();
        rels.add("theme", "../theme/theme1.xml", false);
        pkg.rels_part("ppt/notesMasters/notesMaster1.xml", &rels);
        for (slide_name, notes_name, text) in &notes_parts {
            pkg.xml_part(notes_name, CT_NOTES, notes_slide_xml(text));
            let mut rels = Rels::default();
            rels.add(
                "notesMaster",
                &rel_path(notes_name, "ppt/notesMasters/notesMaster1.xml"),
                false,
            );
            rels.add("slide", &rel_path(notes_name, slide_name), false);
            pkg.rels_part(notes_name, &rels);
        }
        notes_master_rid =
            Some(pres_rels.add("notesMaster", "notesMasters/notesMaster1.xml", false));
    }

    pkg.xml_part(
        "ppt/presentation.xml",
        CT_PRES,
        presentation_xml(&master_rid, &slide_rids, notes_master_rid.as_deref()),
    );
    pkg.rels_part("ppt/presentation.xml", &pres_rels);
    pkg.xml_part("ppt/presProps.xml", CT_PRES_PROPS, pres_props_xml());
    pkg.xml_part("ppt/viewProps.xml", CT_VIEW_PROPS, view_props_xml());
    pkg.xml_part("ppt/tableStyles.xml", CT_TABLE_STYLES, table_styles_xml());
    pkg.xml_part("docProps/core.xml", CT_CORE, core_xml("slides"));
    pkg.xml_part("docProps/app.xml", CT_APP, app_xml(deck.slides.len()));

    let mut root_rels = Rels::default();
    root_rels.add(RT_DOC, "ppt/presentation.xml", false);
    root_rels.add(RT_CORE, "docProps/core.xml", false);
    root_rels.add("extended-properties", "docProps/app.xml", false);
    pkg.rels_part("", &root_rels);

    for (name, data, ct) in &media.parts {
        if *ct == CT_CHART {
            pkg.xml_part(name, ct, data.clone());
        } else {
            pkg.binary(name, ct, data.clone());
        }
    }

    Ok((pkg.finish()?, warnings))
}

// -------------------------------------------------------------------------------------------
// Reading

fn open_zip(bytes: &[u8], limit: u64) -> Result<HashMap<String, Vec<u8>>, Error> {
    let package = crate::ooxml::Package::open(bytes, limit)?;
    let mut parts = HashMap::new();
    for (name, data) in package.0 {
        if parts.insert(name.to_ascii_lowercase(), data).is_some() {
            return Err(crate::ooxml::invalid(&name));
        }
    }
    Ok(parts)
}

fn get_part<'a>(parts: &'a HashMap<String, Vec<u8>>, name: &str) -> Option<&'a [u8]> {
    let key = name.trim_start_matches('/').to_ascii_lowercase();
    parts.get(&key).map(Vec::as_slice)
}

struct ReadRel {
    kind: String,
    target: String,
}

fn read_rels(
    parts: &HashMap<String, Vec<u8>>,
    part: &str,
) -> Result<HashMap<String, ReadRel>, Error> {
    let mut map = HashMap::new();
    let Some(bytes) = get_part(parts, &rels_path(part)) else {
        return Ok(map);
    };
    let doc = xmldom::parse(bytes)?;
    let base = dir_of(part);
    for rel in doc.children_named("Relationship") {
        let id = rel.attr("Id").unwrap_or("").to_string();
        let ty = rel.attr("Type").unwrap_or("").to_string();
        let raw = rel.attr("Target").unwrap_or("").to_string();
        let external = rel
            .attr("TargetMode")
            .map(|m| m.eq_ignore_ascii_case("external"))
            .unwrap_or(false);
        let kind = ty.rsplit('/').next().unwrap_or("").to_string();
        let target = if external {
            raw.clone()
        } else {
            resolve_part(&base, &raw)
        };
        map.insert(id, ReadRel { kind, target });
    }
    Ok(map)
}

fn find_path<'a>(root: &'a El, path: &[&str]) -> Option<&'a El> {
    let mut current = root;
    for name in path {
        current = current.child(name)?;
    }
    Some(current)
}

fn paragraphs_of(shape: &El) -> Vec<String> {
    all_paragraphs_of(shape)
        .into_iter()
        .filter(|s| !s.trim().is_empty())
        .collect()
}

// Code uses every paragraph, including blank first/last lines. Placeholder filtering
// belongs to the ordinary text reader, not to this semantic extraction step.
fn all_paragraphs_of(shape: &El) -> Vec<String> {
    let Some(tb) = shape.child("p:txBody").or_else(|| shape.child("a:txBody")) else {
        return Vec::new();
    };
    tb.children_named("a:p")
        .map(|p| {
            let text = crate::paragraph::Paragraph::read(p).text;
            let level = p
                .child("a:pPr")
                .and_then(|pr| pr.attr("lvl"))
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(0)
                .min(8);
            format!("{}{}", "  ".repeat(level), text)
        })
        .collect()
}

pub(crate) fn table_metadata(tbl: &El) -> Result<Option<(String, usize)>, Error> {
    let Some(metadata) = tbl.child("a:extLst").and_then(|extensions| {
        extensions
            .children_named("a:ext")
            .find_map(|ext| ext.child("sl:table"))
    }) else {
        return Ok(None);
    };
    let id = metadata
        .attr("id")
        .filter(|s| !s.is_empty())
        .ok_or_else(|| crate::ooxml::invalid("logical table id"))?;
    let start = metadata
        .attr("start")
        .ok_or_else(|| crate::ooxml::invalid("logical table row offset"))?
        .parse::<usize>()
        .map_err(|source| Error::XmlSource {
            source: Box::new(source),
        })?;
    Ok(Some((id.into(), start)))
}

fn parse_table(tbl: &El) -> Result<Table, Error> {
    let mut rows = Vec::new();
    for tr in tbl.children_named("a:tr") {
        let mut cells = Vec::new();
        for tc in tr.children_named("a:tc") {
            let text = all_paragraphs_of(tc).join("\n");
            cells.push(text);
        }
        rows.push(cells);
    }
    let mut table = Table::default();
    if let Some((id, start)) = table_metadata(tbl)? {
        table.logical_id = Some(id);
        table.row_start = start;
    }
    let mut iter = rows.into_iter();
    if let Some(headers) = iter.next() {
        table.headers = headers;
        table.rows = iter.collect();
    }
    Ok(table)
}

// Visit each selected object once, including compatibility branches at any group depth.
// Return false if a compatibility container cannot be interpreted.
pub(crate) fn shape_objects<'a>(tree: &'a El, out: &mut Vec<&'a El>) -> bool {
    let mut supported = true;
    let mut pending: Vec<_> = tree.elements().collect();
    pending.reverse();
    while let Some(child) = pending.pop() {
        if child.name == "p:grpSp" {
            pending.extend(child.elements().collect::<Vec<_>>().into_iter().rev());
        } else if child.name == "mc:AlternateContent" {
            if let Some(branch) = child.compatibility_branch() {
                pending.extend(branch.elements().collect::<Vec<_>>().into_iter().rev());
            } else {
                supported = false;
            }
        } else if matches!(child.name.as_str(), "p:sp" | "p:graphicFrame" | "p:pic") {
            out.push(child);
        } else if child.name != "p:nvGrpSpPr" && child.name != "p:grpSpPr" {
            supported = false;
        }
    }
    supported
}

fn extract_slide(
    dom: &El,
    rels: &HashMap<String, ReadRel>,
    parts: &HashMap<String, Vec<u8>>,
    assets: &mut BTreeMap<String, Vec<u8>>,
    warnings: &mut Vec<Warning>,
    locale: &str,
    index: usize,
) -> Result<Slide, Error> {
    let mut slide = Slide::default();
    let tree = crate::ooxml::slide_tree(dom)?;
    let mut objects = Vec::new();
    if !shape_objects(tree, &mut objects) {
        warnings.push(Warning::new(
            MessageCode::ReadCompatibilityUnsupported,
            params(&[("slide", &index.to_string())]),
            locale,
        ));
    }
    for sp in objects {
        if sp.name == "p:graphicFrame" {
            if let Some(rid) = find_path(sp, &["a:graphic", "a:graphicData", "c:chart"])
                .and_then(|c| c.attr("r:id"))
                && let Some(rel) = rels.get(rid)
                && let Some(data) = get_part(parts, &rel.target)
            {
                match crate::charts::read_all(data) {
                    Ok(charts) => {
                        if charts.len() > 1 {
                            warnings.push(Warning::new(
                                MessageCode::ReadChartCombination,
                                params(&[
                                    ("slide", &index.to_string()),
                                    ("groups", &charts.len().to_string()),
                                ]),
                                locale,
                            ));
                        }
                        slide.blocks.extend(charts.into_iter().map(Block::Chart));
                    }
                    Err(Error::Unsupported(mut msg))
                        if msg.code == MessageCode::ReadChartUnsupported =>
                    {
                        msg.params.push(("slide".into(), index.to_string()));
                        warnings.push(Warning::new(msg.code, msg.params, locale));
                    }
                    Err(error) => return Err(error),
                }
            }
            if let Some(tbl) = find_path(sp, &["a:graphic", "a:graphicData", "a:tbl"]) {
                let table = parse_table(tbl)?;
                if !table.headers.is_empty() || !table.rows.is_empty() {
                    slide.blocks.push(Block::Table(table));
                }
            }
            continue;
        }
        if sp.name == "p:pic" {
            if let Some(embed) =
                find_path(sp, &["p:blipFill", "a:blip"]).and_then(|b| b.attr("r:embed"))
                && let Some(rel) = rels.get(embed)
                && let Some(data) = get_part(parts, &rel.target)
            {
                assets.insert(rel.target.clone(), data.to_vec());
                slide.blocks.push(Block::Image(rel.target.clone()));
            }
            continue;
        }
        let ph_type = sp
            .child("p:nvSpPr")
            .and_then(|nv| nv.child("p:nvPr"))
            .and_then(|pr| pr.child("p:ph"))
            .and_then(|ph| ph.attr("type"))
            .unwrap_or("");
        if block_kind(sp) == Some("code")
            || sp
                .child("p:nvSpPr")
                .and_then(|nv| nv.child("p:cNvPr"))
                .and_then(|pr| pr.attr("name"))
                == Some("Code")
        {
            slide
                .blocks
                .push(Block::Code(all_paragraphs_of(sp).join("\n")));
            continue;
        }
        let paras = paragraphs_of(sp);
        if sp
            .child("p:nvSpPr")
            .and_then(|nv| nv.child("p:cNvPr"))
            .and_then(|pr| pr.attr("name"))
            == Some("Source footnotes")
        {
            slide.sources.extend(paras);
            continue;
        }
        if paras.is_empty() {
            continue;
        }
        match ph_type {
            "title" | "ctrTitle" => slide.title = paras.join("\n"),
            "sldImg" | "sldNum" | "ftr" | "dt" => {}
            "subTitle" => slide.subtitle = paras.join("\n"),
            _ => slide.blocks.push(Block::Bullets(paras)),
        }
    }
    Ok(slide)
}

/// Read a deck from a PPTX package.
pub(crate) fn read_with_warnings(
    bytes: &[u8],
    limit: u64,
    locale: &str,
) -> Result<(Deck, Vec<Warning>), Error> {
    let mut warnings = Vec::new();
    let parts = open_zip(bytes, limit)?;
    let pres_bytes = get_part(&parts, "ppt/presentation.xml")
        .ok_or_else(|| pptx_error(MessageCode::PptxMissingPresentation, Vec::new()))?;
    let pres = xmldom::parse(pres_bytes)?;
    let rels = read_rels(&parts, "ppt/presentation.xml")?;
    let mut slide_parts = Vec::new();
    {
        let list = crate::ooxml::slide_list(&pres)?;
        for id in list.children_named("p:sldId") {
            let rel = id
                .attr("r:id")
                .and_then(|rid| rels.get(rid))
                .filter(|rel| rel.kind == "slide")
                .ok_or_else(|| crate::ooxml::invalid("p:sldId"))?;
            slide_parts.push(rel.target.clone());
        }
    }
    let mut deck = Deck::default();
    for part in slide_parts {
        let data = get_part(&parts, &part).ok_or_else(|| crate::ooxml::invalid(&part))?;
        let dom = xmldom::parse(data)?;
        let slide_rels = read_rels(&parts, &part)?;
        let mut slide = extract_slide(
            &dom,
            &slide_rels,
            &parts,
            &mut deck.assets,
            &mut warnings,
            locale,
            deck.slides.len() + 1,
        )?;
        if let Some(order) = slide_rels.values().find(|r| r.kind == "notesSlide")
            && let Some(notes_bytes) = get_part(&parts, &order.target)
            && let Ok(notes_dom) = xmldom::parse(notes_bytes)
        {
            slide.notes = extract_notes(&notes_dom, &mut warnings, locale, deck.slides.len() + 1);
        }
        slide.layout = Deck::infer_layout(&slide);
        if let Some(layout) = slide_rels.values().find(|r| r.kind == "slideLayout")
            && let Some(data) = get_part(&parts, &layout.target)
        {
            let layout = xmldom::parse(data)?;
            slide.layout = layout
                .child("p:cSld")
                .and_then(|c| c.attr("name"))
                .and_then(Layout::parse)
                .unwrap_or_else(|| match layout.attr("type") {
                    Some("title") => Layout::Title,
                    Some("twoObj") => Layout::TwoContent,
                    Some("secHead") => Layout::Section,
                    _ => slide.layout,
                });
        }
        deck.slides.push(slide);
    }
    Ok((deck, warnings))
}

fn extract_notes(dom: &El, warnings: &mut Vec<Warning>, locale: &str, index: usize) -> String {
    let Some(tree) = find_path(dom, &["p:cSld", "p:spTree"]) else {
        return String::new();
    };
    let mut objects = Vec::new();
    if !shape_objects(tree, &mut objects) {
        warnings.push(Warning::new(
            MessageCode::ReadCompatibilityUnsupported,
            params(&[("slide", &index.to_string())]),
            locale,
        ));
    }
    for sp in objects {
        let ph_type = sp
            .child("p:nvSpPr")
            .and_then(|nv| nv.child("p:nvPr"))
            .and_then(|pr| pr.child("p:ph"))
            .and_then(|ph| ph.attr("type"))
            .unwrap_or("");
        if ph_type == "body" {
            let paras = paragraphs_of(sp);
            if !paras.is_empty() {
                return paras.join("\n");
            }
        }
    }
    String::new()
}

// -------------------------------------------------------------------------------------------
// Editing

struct Store {
    items: Vec<(String, Vec<u8>)>,
    index: HashMap<String, usize>,
}

impl Store {
    fn open(bytes: &[u8], limit: u64) -> Result<Self, Error> {
        let items = crate::ooxml::Package::open(bytes, limit)?
            .0
            .into_iter()
            .collect();
        let mut store = Store {
            items,
            index: HashMap::new(),
        };
        store.reindex();
        Ok(store)
    }

    fn reindex(&mut self) {
        self.index.clear();
        for (i, (name, _)) in self.items.iter().enumerate() {
            self.index.insert(name.to_ascii_lowercase(), i);
        }
    }

    fn get(&self, name: &str) -> Option<&[u8]> {
        self.index
            .get(&name.to_ascii_lowercase())
            .and_then(|i| self.items.get(*i))
            .map(|(_, d)| d.as_slice())
    }

    fn set(&mut self, name: &str, data: Vec<u8>) {
        match self.index.get(&name.to_ascii_lowercase()) {
            Some(i) => {
                if let Some(slot) = self.items.get_mut(*i) {
                    slot.1 = data;
                }
            }
            None => {
                self.items.push((name.to_string(), data));
                self.reindex();
            }
        }
    }

    fn remove(&mut self, name: &str) {
        if let Some(i) = self.index.get(&name.to_ascii_lowercase()).copied() {
            self.items.remove(i);
            self.reindex();
        }
    }

    fn finish(&self) -> Result<Vec<u8>, Error> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default()
            .last_modified_time(zip::DateTime::default())
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, data) in &self.items {
            writer
                .start_file(name.as_str(), options)
                .map_err(Error::zip)?;
            writer.write_all(data).map_err(Error::zip)?;
        }
        let cursor = writer.finish().map_err(Error::zip)?;
        Ok(cursor.into_inner())
    }
}

/// Select the requested PDF font in a temporary package, without changing the user's PPTX.
pub(crate) fn with_font(bytes: &[u8], family: &str) -> Result<Vec<u8>, Error> {
    let mut store = Store::open(bytes, crate::DEFAULT_MAX_INPUT_BYTES)?;
    for (name, data) in &mut store.items {
        if name.starts_with("ppt/") && name.ends_with(".xml") {
            let mut dom = xmldom::parse(data)?;
            dom.visit_mut(|el| {
                if matches!(el.name.as_str(), "a:latin" | "a:ea" | "a:cs")
                    && el.attr("typeface") != Some("DejaVu Sans Mono")
                {
                    el.set_attr("typeface", family);
                }
                true
            });
            *data = xmldom::write(&dom);
        }
    }
    store.finish()
}

fn replace_in_text(el: &mut El, find: &str, replace: &str) -> usize {
    let mut matches = 0;
    el.visit_mut(|el| {
        if el.name == "a:p" {
            matches += crate::paragraph::replace(el, find, replace);
            false
        } else {
            true
        }
    });
    matches
}

fn shape_ph_type(shape: &El) -> String {
    shape
        .child("p:nvSpPr")
        .and_then(|nv| nv.child("p:nvPr"))
        .and_then(|pr| pr.child("p:ph"))
        // OOXML defaults an omitted placeholder type to obj (LibreOffice emits <p:ph/>).
        .map(|ph| ph.attr("type").unwrap_or("obj"))
        .unwrap_or("")
        .to_string()
}

fn find_shape_mut<'a>(root: &'a mut El, kinds: &[&str]) -> Option<&'a mut El> {
    let mut pending = vec![root];
    while let Some(root) = pending.pop() {
        let direct = root.children.iter().position(|child| matches!(child, Node::El(e) if e.name == "p:sp" && kinds.contains(&shape_ph_type(e).as_str())));
        if let Some(i) = direct {
            if let Some(Node::El(e)) = root.children.get_mut(i) {
                return Some(e);
            }
            return None;
        }
        let children: Vec<_> = root.elements_mut().filter(|e| e.name != "p:sp").collect();
        pending.extend(children.into_iter().rev());
    }
    None
}

fn find_tree_mut(dom: &mut El) -> Option<&mut El> {
    let c_sld = dom.child_mut("p:cSld")?;
    c_sld.child_mut("p:spTree")
}

fn set_placeholder_text(dom: &mut El, kinds: &[&str], texts: &[String]) -> Result<bool, Error> {
    let Some(tree) = find_tree_mut(dom) else {
        return Err(pptx_error(MessageCode::PptxNoShapeTree, Vec::new()));
    };
    let Some(shape) = find_shape_mut(tree, kinds) else {
        return Ok(false);
    };
    let Some(tb) = shape.child_mut("p:txBody") else {
        return Ok(false);
    };
    let originals: Vec<_> = tb.children_named("a:p").cloned().collect();
    if originals
        .iter()
        .map(|p| crate::paragraph::Paragraph::read(p).text)
        .collect::<Vec<_>>()
        == texts
    {
        return Ok(true);
    }
    let mut paras = Vec::new();
    for (index, text) in texts.iter().enumerate() {
        let mut para = originals
            .get(index)
            .or_else(|| originals.last())
            .cloned()
            .unwrap_or_else(|| crate::ooxml::el("a:p", &[], vec![]));
        crate::paragraph::set_text(&mut para, text);
        paras.push(Node::El(para));
    }
    let at = tb
        .children
        .iter()
        .position(|n| matches!(n, Node::El(e) if e.name == "a:p"))
        .unwrap_or(tb.children.len());
    tb.children
        .retain(|n| !matches!(n, Node::El(e) if e.name == "a:p"));
    tb.children.splice(at..at, paras);
    Ok(true)
}

pub(crate) fn next_rid(rels: &El) -> String {
    let mut max = 0u32;
    let mut used = std::collections::BTreeSet::new();
    for rel in rels.children_named("Relationship") {
        if let Some(id) = rel.attr("Id")
            && let Some(number) = id.strip_prefix("rId").and_then(|n| n.parse::<u32>().ok())
        {
            max = max.max(number);
            used.insert(u64::from(number));
        }
    }
    let number = max.checked_add(1).map(u64::from).unwrap_or_else(|| {
        // Find the first hole without overflowing even at u32::MAX.
        let mut candidate = 1u64;
        for n in used {
            if n == candidate {
                candidate += 1;
            }
        }
        candidate
    });
    format!("rId{number}")
}

fn ensure_rels_dom(store: &mut Store, part: &str) -> Result<El, Error> {
    let name = rels_path(part);
    match store.get(&name) {
        Some(bytes) => {
            let mut dom = xmldom::parse(bytes)?;
            if dom.name != "Relationships" {
                return Err(pptx_error(
                    MessageCode::PptxBadRelationships,
                    params(&[("part", &name)]),
                ));
            }
            // The namespace is required when a rels part is created from scratch; a parsed one
            // already carries it.
            if dom.attr("xmlns").is_none() {
                dom.set_attr(
                    "xmlns",
                    "http://schemas.openxmlformats.org/package/2006/relationships",
                );
            }
            Ok(dom)
        }
        None => {
            let mut dom = El {
                spelling: Default::default(),
                name: "Relationships".to_string(),
                attrs: vec![(
                    "xmlns".to_string(),
                    "http://schemas.openxmlformats.org/package/2006/relationships".to_string(),
                )],
                children: Vec::new(),
            };
            let _ = &mut dom;
            Ok(dom)
        }
    }
}

fn add_relationship(dom: &mut El, kind: &str, target: &str) -> String {
    let id = next_rid(dom);
    let ty = if kind.starts_with("http") {
        kind.to_string()
    } else {
        format!("{RT_BASE}{kind}")
    };
    dom.children.push(Node::El(El {
        spelling: Default::default(),
        name: "Relationship".to_string(),
        attrs: vec![
            ("Id".to_string(), id.clone()),
            ("Type".to_string(), ty),
            ("Target".to_string(), target.to_string()),
        ],
        children: Vec::new(),
    }));
    id
}

fn remove_relationship(dom: &mut El, id: &str) {
    dom.children.retain(|node| match node {
        Node::El(e) => !(e.name == "Relationship" && e.attr("Id") == Some(id)),
        _ => true,
    });
}

fn content_types_add_override(dom: &mut El, part: &str, ct: &str) {
    let part_name = format!("/{part}");
    dom.children.retain(|node| {
        !matches!(node, Node::El(e) if e.name == "Override" && e.attr("PartName") == Some(part_name.as_str()))
    });
    dom.children.push(Node::El(El {
        spelling: Default::default(),
        name: "Override".to_string(),
        attrs: vec![
            ("PartName".to_string(), part_name),
            ("ContentType".to_string(), ct.to_string()),
        ],
        children: Vec::new(),
    }));
}

fn content_types_remove_override(dom: &mut El, part: &str) {
    let part_name = format!("/{part}");
    dom.children.retain(|node| {
        !matches!(node, Node::El(e) if e.name == "Override" && e.attr("PartName") == Some(part_name.as_str()))
    });
}

fn slide_part_for_index(
    pres: &El,
    pres_rels: &HashMap<String, ReadRel>,
    index: usize,
) -> Option<String> {
    let list = pres.child("p:sldIdLst")?;
    let id = list.children_named("p:sldId").nth(index)?;
    let rid = id.attr("r:id")?;
    pres_rels.get(rid).map(|r| r.target.clone())
}

fn next_part_number(store: &Store, prefix: &str, suffix: &str) -> u32 {
    let mut max = 0u32;
    for (name, _) in &store.items {
        let lower = name.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix(prefix)
            && let Some(num) = rest.strip_suffix(suffix)
            && let Ok(n) = num.parse::<u32>()
        {
            max = max.max(n);
        }
    }
    max + 1
}

fn ensure_notes_master(
    store: &mut Store,
    pres: &mut El,
    pres_rels: &mut El,
) -> Result<String, Error> {
    if let Some(rel) = pres_rels
        .children_named("Relationship")
        .find(|r| r.attr("Type").is_some_and(|t| t.ends_with("/notesMaster")))
    {
        let part = crate::ooxml::target(
            "ppt/presentation.xml",
            rel.attr("Target")
                .ok_or_else(|| crate::ooxml::invalid("notes master"))?,
        )?;
        if store.get(&part).is_none() {
            return Err(crate::ooxml::invalid(&part));
        }
        return Ok(part);
    }
    let part = "ppt/notesMasters/notesMaster1.xml";
    if store.get(part).is_none() {
        store.set(part, notes_master_xml());
        let mut rels = El {
            spelling: Default::default(),
            name: "Relationships".to_string(),
            attrs: vec![(
                "xmlns".to_string(),
                "http://schemas.openxmlformats.org/package/2006/relationships".to_string(),
            )],
            children: Vec::new(),
        };
        let master = pres_rels
            .children_named("Relationship")
            .find(|r| r.attr("Type").is_some_and(|t| t.ends_with("/slideMaster")))
            .and_then(|r| r.attr("Target"))
            .ok_or_else(|| crate::ooxml::invalid("slide master"))?;
        let master = crate::ooxml::target("ppt/presentation.xml", master)?;
        let master_rels = ensure_rels_dom(store, &master)?;
        let theme = master_rels
            .children_named("Relationship")
            .find(|r| r.attr("Type").is_some_and(|t| t.ends_with("/theme")))
            .and_then(|r| r.attr("Target"))
            .ok_or_else(|| crate::ooxml::invalid("theme"))?;
        let theme = crate::ooxml::target(&master, theme)?;
        add_relationship(&mut rels, "theme", &format!("/{theme}"));
        store.set(&rels_path(part), xmldom::write(&rels));
        if let Some(ct) = store.get("[Content_Types].xml") {
            let mut dom = xmldom::parse(ct)?;
            content_types_add_override(&mut dom, part, CT_NOTES_MASTER);
            store.set("[Content_Types].xml", xmldom::write(&dom));
        }
    }
    let id = add_relationship(pres_rels, "notesMaster", "notesMasters/notesMaster1.xml");
    if pres.child("p:notesMasterIdLst").is_none() {
        let at = pres
            .children
            .iter()
            .position(
                |n| matches!(n, Node::El(e) if matches!(e.name.as_str(), "p:sldIdLst" | "p:sldSz")),
            )
            .unwrap_or(pres.children.len());
        pres.children
            .insert(at, crate::ooxml::node("p:notesMasterIdLst", &[], vec![]));
    }
    if let Some(list) = pres.child_mut("p:notesMasterIdLst") {
        list.children.push(crate::ooxml::node(
            "p:notesMasterId",
            &[("r:id", &id)],
            vec![],
        ));
    }
    Ok(part.to_string())
}

fn set_notes(
    store: &mut Store,
    slide_part: &str,
    slide_rels: &mut El,
    pres: &mut El,
    pres_rels: &mut El,
    text: &str,
) -> Result<(), Error> {
    let existing = slide_rels.children_named("Relationship").find(|r| {
        r.attr("Type")
            .map(|t| t.ends_with("/notesSlide"))
            .unwrap_or(false)
    });
    if let Some(rel) = existing {
        if let Some(target) = rel.attr("Target") {
            let notes_part = resolve_part(&dir_of(slide_part), target);
            if let Some(bytes) = store.get(&notes_part) {
                let mut dom = xmldom::parse(bytes)?;
                let texts: Vec<String> = text.lines().map(String::from).collect();
                let texts = if texts.is_empty() {
                    vec![String::new()]
                } else {
                    texts
                };
                set_placeholder_text(&mut dom, &["body"], &texts)?;
                store.set(&notes_part, xmldom::write(&dom));
            }
        }
        return Ok(());
    }
    let master = ensure_notes_master(store, pres, pres_rels)?;
    let number = next_part_number(store, "ppt/notesslides/notesslide", ".xml");
    let notes_part = format!("ppt/notesSlides/notesSlide{number}.xml");
    store.set(&notes_part, notes_slide_xml(text));
    let mut rels = El {
        spelling: Default::default(),
        name: "Relationships".to_string(),
        attrs: vec![(
            "xmlns".to_string(),
            "http://schemas.openxmlformats.org/package/2006/relationships".to_string(),
        )],
        children: Vec::new(),
    };
    add_relationship(&mut rels, "notesMaster", &rel_path(&notes_part, &master));
    add_relationship(&mut rels, "slide", &rel_path(&notes_part, slide_part));
    store.set(&rels_path(&notes_part), xmldom::write(&rels));
    add_relationship(slide_rels, "notesSlide", &rel_path(slide_part, &notes_part));
    if let Some(ct) = store.get("[Content_Types].xml") {
        let mut dom = xmldom::parse(ct)?;
        content_types_add_override(&mut dom, &notes_part, CT_NOTES);
        store.set("[Content_Types].xml", xmldom::write(&dom));
    }
    Ok(())
}

/// Apply an edit request to a PPTX package.
pub fn edit(
    bytes: &[u8],
    req: &EditInput,
    locale: &str,
    limit: u64,
) -> Result<(Vec<u8>, Vec<Warning>), Error> {
    let mut store = Store::open(bytes, limit)?;
    let pres_bytes = store
        .get("ppt/presentation.xml")
        .ok_or_else(|| pptx_error(MessageCode::PptxMissingPresentation, Vec::new()))?
        .to_vec();
    let mut pres = xmldom::parse(&pres_bytes)?;
    let mut pres_rels = ensure_rels_dom(&mut store, "ppt/presentation.xml")?;
    let original_pres = xmldom::write(&pres);
    let original_rels = xmldom::write(&pres_rels);
    let mut warnings: Vec<Warning> = Vec::new();

    // Recompute slide order from the presentation.
    let parts = open_zip(bytes, limit)?;
    let rels_map: HashMap<String, ReadRel> = read_rels(&parts, "ppt/presentation.xml")?;
    let mut slide_parts: Vec<String> = Vec::new();
    if let Some(list) = pres.child("p:sldIdLst") {
        for id in list.children_named("p:sldId") {
            if let Some(rid) = id.attr("r:id")
                && let Some(rel) = rels_map.get(rid)
            {
                slide_parts.push(rel.target.clone());
            }
        }
    }

    // 1. Text replacement in slides and notes.
    if let Some(replace) = &req.replace {
        let mut matches = 0;
        for part in &slide_parts {
            if let Some(data) = store.get(part) {
                let mut dom = xmldom::parse(data)?;
                let count = replace_in_text(&mut dom, &replace.find, &replace.replace);
                matches += count;
                if count > 0 && replace.find != replace.replace {
                    store.set(part, xmldom::write(&dom));
                }
            }
            if let Some(map_data) = store.get(&rels_path(part)) {
                let map = xmldom::parse(map_data)?;
                for rel in map.children_named("Relationship") {
                    if rel
                        .attr("Type")
                        .map(|t| t.ends_with("/notesSlide"))
                        .unwrap_or(false)
                        && let Some(target) = rel.attr("Target")
                    {
                        let notes_part = resolve_part(&dir_of(part), target);
                        if let Some(data) = store.get(&notes_part) {
                            let mut dom = xmldom::parse(data)?;
                            let count = replace_in_text(&mut dom, &replace.find, &replace.replace);
                            matches += count;
                            if count > 0 && replace.find != replace.replace {
                                store.set(&notes_part, xmldom::write(&dom));
                            }
                        }
                    }
                }
            }
        }
        if matches == 0 {
            warnings.push(Warning::new(MessageCode::EditNoMatch, Vec::new(), locale));
        }
    }

    // 2. Per-slide edits.
    if let Some(edits) = &req.slides {
        for edit in edits {
            let Some(part) = slide_part_for_index(&pres, &rels_map, edit.index)
                .or_else(|| slide_parts.get(edit.index).cloned())
            else {
                warnings.push(Warning::new(
                    MessageCode::EditSlideMissing,
                    params(&[("index", &edit.index.to_string())]),
                    locale,
                ));
                continue;
            };
            if let Some(data) = store.get(&part) {
                let mut dom = xmldom::parse(data)?;
                let before = xmldom::write(&dom);
                apply_slide_edit(&mut dom, edit, &mut warnings, locale)?;
                if xmldom::write(&dom) != before {
                    store.set(&part, xmldom::write(&dom));
                }
            }
            if let Some(notes) = &edit.notes {
                let mut slide_rels = ensure_rels_dom(&mut store, &part)?;
                set_notes(
                    &mut store,
                    &part,
                    &mut slide_rels,
                    &mut pres,
                    &mut pres_rels,
                    notes,
                )?;
                store.set(&rels_path(&part), xmldom::write(&slide_rels));
            }
        }
    }

    // 3. Delete slides.
    if let Some(delete) = &req.delete {
        let mut indices: Vec<usize> = delete.clone();
        indices.sort_unstable();
        indices.dedup();
        for index in indices.into_iter().rev() {
            let Some(part) = slide_parts.get(index).cloned() else {
                warnings.push(Warning::new(
                    MessageCode::EditSlideMissing,
                    params(&[("index", &index.to_string())]),
                    locale,
                ));
                continue;
            };
            remove_slide(&mut store, &mut pres, &mut pres_rels, &part)?;
            slide_parts.remove(index);
        }
    }

    // Fit only changed text bodies, once all edits/deletions have determined the final text
    // and output slide numbers. Original parts remain available for the comparison.
    for (index, part) in slide_parts.iter().enumerate() {
        if let (Some(current), Some(original)) = (store.get(part), get_part(&parts, part)) {
            if current == original {
                continue;
            }
            let mut dom = xmldom::parse(current)?;
            let before = xmldom::parse(original)?;
            let mut inherited = Vec::new();
            let mut parent = part.clone();
            for kind in ["slideLayout", "slideMaster"] {
                let rels = read_rels(&parts, &parent)?;
                if let Some(rel) = rels.values().find(|rel| rel.kind == kind) {
                    parent = rel.target.clone();
                    if let Some(data) = get_part(&parts, &parent) {
                        inherited.push(xmldom::parse(data)?);
                    }
                }
            }
            crate::edited_text::refit(
                &mut dom,
                &before,
                &inherited,
                index + 1,
                &mut warnings,
                locale,
            );
            store.set(part, xmldom::write(&dom));
        }
    }

    // Additions are composed by the company-template path after these edits, using the
    // current presentation's dimensions, masters and placeholders instead of our default grid.

    let updated_pres = xmldom::write(&pres);
    let updated_rels = xmldom::write(&pres_rels);
    if updated_pres != original_pres {
        store.set("ppt/presentation.xml", updated_pres);
    }
    if updated_rels != original_rels {
        store.set(&rels_path("ppt/presentation.xml"), updated_rels);
    }
    Ok((store.finish()?, warnings))
}

fn apply_slide_edit(
    dom: &mut El,
    edit: &SlideEdit,
    warnings: &mut Vec<Warning>,
    locale: &str,
) -> Result<(), Error> {
    let mut missing = |field: &str| {
        warnings.push(Warning::new(
            MessageCode::EditFieldMissing,
            params(&[("index", &edit.index.to_string()), ("field", field)]),
            locale,
        ))
    };
    if let Some(title) = &edit.title
        && !set_placeholder_text(dom, &["title", "ctrTitle"], std::slice::from_ref(title))?
    {
        missing("title");
    }
    if let Some(bullets) = &edit.bullets
        && !set_placeholder_text(dom, &["body", "obj", "subTitle"], bullets)?
    {
        missing("bullets");
    }
    Ok(())
}

fn remove_slide(
    store: &mut Store,
    pres: &mut El,
    pres_rels: &mut El,
    part: &str,
) -> Result<(), Error> {
    let mut removed = vec![part.to_string()];
    if let Some(data) = store.get(&rels_path(part)) {
        let map = xmldom::parse(data)?;
        for rel in map.children_named("Relationship") {
            if rel.attr("Type").is_some_and(|t| t.ends_with("/notesSlide")) {
                removed.push(crate::ooxml::target(
                    part,
                    rel.attr("Target")
                        .ok_or_else(|| crate::ooxml::invalid(part))?,
                )?);
            }
        }
    }
    // Resolve addresses, never compare their relative/absolute spelling. Also remove incoming
    // slide hyperlinks and custom-show entries, not just the main presentation's slide list.
    let maps: Vec<_> = store
        .items
        .iter()
        .filter(|(p, _)| p.ends_with(".rels"))
        .map(|(p, _)| p.clone())
        .collect();
    for path in maps {
        let source =
            crate::ooxml::relationship_source(&path).ok_or_else(|| crate::ooxml::invalid(&path))?;
        let mut map = if source == "ppt/presentation.xml" {
            pres_rels.clone()
        } else {
            xmldom::parse(
                store
                    .get(&path)
                    .ok_or_else(|| crate::ooxml::invalid(&path))?,
            )?
        };
        let mut ids = Vec::new();
        for rel in map.children_named("Relationship") {
            if rel
                .attr("TargetMode")
                .is_some_and(|s| s.eq_ignore_ascii_case("external"))
            {
                continue;
            }
            let target = crate::ooxml::target(
                &source,
                rel.attr("Target")
                    .ok_or_else(|| crate::ooxml::invalid(&path))?,
            )?;
            if removed.iter().any(|p| p.eq_ignore_ascii_case(&target)) {
                ids.push(
                    rel.attr("Id")
                        .ok_or_else(|| crate::ooxml::invalid(&path))?
                        .to_string(),
                );
            }
        }
        if ids.is_empty() {
            continue;
        }
        for id in &ids {
            remove_relationship(&mut map, id);
        }
        let remove_refs = |dom: &mut El| {
            dom.visit_mut(|el| {
                el.children.retain(|n| !matches!(n, Node::El(e) if e.attr("r:id").is_some_and(|id| ids.iter().any(|i| i == id))));
                true
            });
        };
        if source == "ppt/presentation.xml" {
            *pres_rels = map;
            remove_refs(pres);
        } else {
            store.set(&path, xmldom::write(&map));
            if let Some(data) = store.get(&source) {
                let mut dom = xmldom::parse(data)?;
                remove_refs(&mut dom);
                store.set(&source, xmldom::write(&dom));
            }
        }
    }
    for part in removed {
        store.remove(&part);
        store.remove(&rels_path(&part));
        if let Some(bytes) = store.get("[Content_Types].xml") {
            let mut dom = xmldom::parse(bytes)?;
            content_types_remove_override(&mut dom, &part);
            store.set("[Content_Types].xml", xmldom::write(&dom));
        }
    }
    Ok(())
}

#[cfg(test)]
mod overflow_tests {
    use super::*;

    #[test]
    fn pagination_is_a_fitting_fixed_point_for_all_text_regions() -> Result<(), Error> {
        let dir = tempfile::tempdir().map_err(|source| Error::PdfIo { source })?;
        let long = "Wide W文字 Zażółć ".repeat(180);
        let mut slides = Vec::new();
        for layout in [
            Layout::Title,
            Layout::Section,
            Layout::TitleContent,
            Layout::TwoContent,
            Layout::Quote,
        ] {
            slides.push(Slide {
                layout,
                title: long.clone(),
                subtitle: if layout == Layout::Title {
                    long.clone()
                } else {
                    String::new()
                },
                blocks: vec![Block::Text(long.clone())],
                ..Slide::default()
            });
        }
        slides.push(Slide {
            title: "Cells".into(),
            blocks: vec![Block::Table(Table {
                headers: vec!["Name".into(), "Description".into()],
                rows: vec![vec!["A".into(), long.clone()]],
                ..Table::default()
            })],
            ..Slide::default()
        });
        let (prepared, warnings) = paginate(
            Deck {
                slides,
                ..Deck::default()
            },
            crate::deck::theme("light"),
            dir.path(),
            "en",
        )?;
        assert!(warnings.iter().any(|w| w.code == "slide.split"));
        let (_, warnings) = write(&prepared, crate::deck::theme("light"), dir.path(), "en")?;
        assert!(warnings.is_empty(), "{warnings:?}");
        let (again, warnings) = paginate(
            prepared.clone(),
            crate::deck::theme("light"),
            dir.path(),
            "en",
        )?;
        assert!(warnings.is_empty());
        assert_eq!(prepared.slides.len(), again.slides.len());
        Ok(())
    }
}
