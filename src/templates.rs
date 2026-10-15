//! Company-template composition. Template masters, layouts and their assets stay unchanged.
use crate::deck::{Deck, Layout, Slide, Theme};
use crate::messages::{MessageCode, Warning, params};
use crate::ooxml::{Package, descendants, el, invalid, node, rels_path, target, text};
use crate::xmldom::{self, El, Node};
use crate::{Error, Options, SlideInput};

pub(crate) fn load(path: Option<&str>, opts: &Options) -> Result<Option<Vec<u8>>, Error> {
    path.map(|path| {
        crate::resolve_path(&opts.base_dir, path)?;
        if !matches!(crate::extension(path).as_str(), "pptx" | "potx") {
            return Err(crate::input_error(MessageCode::TemplateFormat, vec![]));
        }
        crate::read_request(&opts.base_dir, path, opts)
    })
    .transpose()
}

pub(crate) fn deck_from_slides(inputs: &[SlideInput]) -> Result<Deck, Error> {
    let slides = inputs
        .iter()
        .map(|input| {
            let mut raw = input.clone();
            if raw
                .layout
                .as_deref()
                .is_some_and(|n| Layout::parse(n).is_none())
            {
                raw.layout = None;
            }
            let mut slide = crate::slide_from_input(&raw)?;
            slide.layout_name = input.layout.clone();
            Ok(slide)
        })
        .collect::<Result<Vec<_>, Error>>()?;
    Ok(Deck {
        slides,
        ..Deck::default()
    })
}

struct CompanyLayout {
    part: String,
    name: String,
    kind: String,
    dom: El,
}
fn layouts(pkg: &Package) -> Result<Vec<CompanyLayout>, Error> {
    let mut out = Vec::new();
    for (_, master) in pkg.related("ppt/presentation.xml", "slideMaster")? {
        let master_dom = pkg.xml(&master)?;
        for (_, part) in pkg.related(&master, "slideLayout")? {
            let mut dom = pkg.xml(&part)?;
            inherit_placeholder_settings(&mut dom, &master_dom);
            let name = dom
                .child("p:cSld")
                .and_then(|c| c.attr("name"))
                .unwrap_or("")
                .to_string();
            let kind = dom.attr("type").unwrap_or("cust").to_string();
            out.push(CompanyLayout {
                part,
                name,
                kind,
                dom,
            });
        }
    }
    if out.is_empty() {
        return Err(crate::input_error(MessageCode::PptxNoLayout, vec![]));
    }
    Ok(out)
}
// Resolve inherited geometry, anchors and base text style only in our working copy.
fn inherit_placeholder_settings(root: &mut El, master: &El) {
    root.visit_mut(|root| {
        if root.name == "p:sp" && placeholder(root).is_some() {
            let kind = ph_kind(root);
            let parent = descendants(master, "p:sp")
                .into_iter()
                .filter(|s| {
                    placeholder(s).is_some()
                        && (ph_kind(s) == kind
                            || (is_title(kind) && is_title(ph_kind(s)))
                            || (body(kind) && body(ph_kind(s))))
                })
                .min_by_key(|s| {
                    (
                        usize::from(ph_kind(s) != kind),
                        usize::from(
                            placeholder(s).and_then(|p| p.attr("idx"))
                                != placeholder(root).and_then(|p| p.attr("idx")),
                        ),
                    )
                });
            let title = is_title(kind);
            if let Some(anchor) = parent
                .and_then(|s| s.child("p:txBody"))
                .and_then(|b| b.child("a:bodyPr"))
                .and_then(|p| p.attr("anchor"))
            {
                if root.child("p:txBody").is_none() {
                    root.children.push(node("p:txBody", &[], vec![]));
                }
                if let Some(tb) = root.child_mut("p:txBody") {
                    if tb.child("a:bodyPr").is_none() {
                        tb.children.insert(0, node("a:bodyPr", &[], vec![]));
                    }
                    if let Some(props) = tb.child_mut("a:bodyPr")
                        && props.attr("anchor").is_none()
                    {
                        props.set_attr("anchor", anchor);
                    }
                }
            }
            // The fitter must see the master font size instead of substituting its generic
            // 24 pt default for an unstyled company title (which enlarges the visible gap).
            let style = master
                .child("p:txStyles")
                .and_then(|s| s.child(if title { "p:titleStyle" } else { "p:bodyStyle" }))
                .and_then(|s| s.child("a:lvl1pPr"))
                .cloned();
            if descendants(root, "a:defRPr").is_empty()
                && descendants(root, "a:rPr").is_empty()
                && descendants(root, "a:endParaRPr").is_empty()
                && let Some(style) = style
                && let Some(tb) = root.child_mut("p:txBody")
            {
                if tb.child("a:lstStyle").is_none() {
                    tb.children.push(node("a:lstStyle", &[], vec![]));
                }
                if let Some(list) = tb.child_mut("a:lstStyle") {
                    if let Some(level) = list.child_mut("a:lvl1pPr") {
                        if let Some(run) = style.child("a:defRPr") {
                            level.children.push(Node::El(run.clone()));
                        }
                    } else {
                        list.children.insert(0, Node::El(style));
                    }
                }
            }
            let transform = parent
                .and_then(|s| s.child("p:spPr"))
                .and_then(|p| p.child("a:xfrm"))
                .cloned();
            if let Some(transform) = transform {
                if root.child("p:spPr").is_none() {
                    root.children.push(node("p:spPr", &[], vec![]));
                }
                if let Some(props) = root.child_mut("p:spPr")
                    && props.child("a:xfrm").is_none()
                {
                    props.children.insert(0, Node::El(transform));
                }
            }
            false
        } else {
            true
        }
    });
}

pub(crate) fn list(bytes: &[u8], limit: u64) -> Result<serde_json::Value, Error> {
    let pkg = Package::open(bytes, limit)?;
    Ok(serde_json::Value::Array(layouts(&pkg)?.iter().map(|l| {
        let placeholders: Vec<_> = descendants(&l.dom, "p:sp").iter().filter_map(|s| {
            let ph = placeholder(s)?;
            Some(serde_json::json!({"type": ph.attr("type").unwrap_or("obj"), "idx": ph.attr("idx").unwrap_or("0"), "name": descendants(s,"p:cNvPr").first().and_then(|n| n.attr("name")).unwrap_or("")}))
        }).collect();
        serde_json::json!({"name":l.name,"type":l.kind,"part":l.part,"placeholders":placeholders})
    }).collect()))
}
fn placeholder(shape: &El) -> Option<&El> {
    descendants(shape, "p:ph").first().copied()
}
fn ph_kind(shape: &El) -> &str {
    placeholder(shape)
        .and_then(|p| p.attr("type"))
        .unwrap_or("obj")
}
fn is_title(kind: &str) -> bool {
    matches!(kind, "title" | "ctrTitle")
}
fn body(kind: &str) -> bool {
    matches!(
        kind,
        "body" | "obj" | "subTitle" | "pic" | "tbl" | "chart" | "media" | "clipArt"
    )
}
fn select<'a>(
    layouts: &'a [CompanyLayout],
    slide: &Slide,
) -> Result<(&'a CompanyLayout, bool), Error> {
    let requested = slide
        .layout_name
        .as_deref()
        .unwrap_or(slide.layout.as_str());
    let ty = match requested {
        "blank" => "blank",
        _ => slide.layout.ooxml_type(),
    };
    let exact = |kind: &str| {
        kind == ty
            || (ty == "obj" && matches!(kind, "tx" | "titleAndContent"))
            || (ty == "twoObj" && kind == "twoColTx")
    };
    if let Some(found) = layouts
        .iter()
        .filter(|l| l.name.eq_ignore_ascii_case(requested))
        .min_by_key(|l| usize::from(!exact(&l.kind)))
    {
        return Ok((found, false));
    }
    if let Some(found) = layouts.iter().find(|l| exact(&l.kind)) {
        return Ok((
            found,
            slide
                .layout_name
                .as_deref()
                .is_some_and(|n| Layout::parse(n).is_none() && n != "blank"),
        ));
    }
    let wanted_body = usize::from(slide.layout != Layout::Title && slide.layout != Layout::Section)
        + usize::from(slide.layout == Layout::TwoContent);
    let found = layouts
        .iter()
        .min_by_key(|l| {
            let shapes = descendants(&l.dom, "p:sp");
            let bodies = shapes
                .iter()
                .filter(|s| placeholder(s).is_some() && body(ph_kind(s)))
                .count();
            let titles = shapes.iter().any(|s| is_title(ph_kind(s)));
            bodies.abs_diff(wanted_body) + usize::from(!titles && !slide.title.is_empty()) * 3
        })
        .ok_or_else(|| invalid("slide layouts"))?;
    Ok((found, true))
}

/// Reserve the same source area in trial layout, final layout and company placeholders.
pub(crate) fn source_top(sources: &[String], width: i64, height: i64) -> i64 {
    if sources.is_empty() {
        return height;
    }
    let columns = ((width as f64 * 0.85 / 12700.0 / 5.4) as usize).max(1);
    let lines = sources
        .iter()
        .flat_map(|s| s.split('\n'))
        .map(|s| s.chars().count().max(1).div_ceil(columns))
        .sum::<usize>();
    let required = (lines as i64)
        .saturating_mul(16 * 12700)
        .saturating_add(91440);
    (height * 9 / 10).min(height.saturating_sub(required).saturating_sub(height / 60))
}

/// Add a small, theme-linked source box. It is not a footer placeholder, so readers do not hide it.
pub(crate) fn footnote(
    bytes: Vec<u8>,
    sources: &[String],
    width: i64,
    height: i64,
) -> Result<Vec<u8>, Error> {
    if sources.is_empty() {
        return Ok(bytes);
    }
    let mut dom = xmldom::parse(&bytes)?;
    let id = descendants(&dom, "p:cNvPr")
        .iter()
        .filter_map(|e| e.attr("id").and_then(|v| v.parse::<u64>().ok()))
        .max()
        .unwrap_or(1)
        .saturating_add(1)
        .to_string();
    let tree = dom
        .child_mut("p:cSld")
        .and_then(|c| c.child_mut("p:spTree"))
        .ok_or_else(|| invalid("p:spTree"))?;
    let paragraphs: Vec<_> = sources
        .iter()
        .map(|s| {
            node(
                "a:p",
                &[],
                vec![
                    node(
                        "a:pPr",
                        &[],
                        vec![
                            node(
                                "a:lnSpc",
                                &[],
                                vec![node("a:spcPct", &[("val", "120000")], vec![])],
                            ),
                            node(
                                "a:spcAft",
                                &[],
                                vec![node("a:spcPts", &[("val", "200")], vec![])],
                            ),
                        ],
                    ),
                    node(
                        "a:r",
                        &[],
                        vec![
                            node(
                                "a:rPr",
                                &[("sz", "900")],
                                vec![
                                    node(
                                        "a:solidFill",
                                        &[],
                                        vec![node("a:schemeClr", &[("val", "tx2")], vec![])],
                                    ),
                                    node("a:latin", &[("typeface", "+mn-lt")], vec![]),
                                ],
                            ),
                            text("a:t", s),
                        ],
                    ),
                ],
            )
        })
        .collect();
    tree.children.push(node(
        "p:sp",
        &[],
        vec![
            node(
                "p:nvSpPr",
                &[],
                vec![
                    node(
                        "p:cNvPr",
                        &[("id", &id), ("name", "Source footnotes")],
                        vec![],
                    ),
                    node("p:cNvSpPr", &[("txBox", "1")], vec![]),
                    node("p:nvPr", &[], vec![]),
                ],
            ),
            node(
                "p:spPr",
                &[],
                vec![node(
                    "a:xfrm",
                    &[],
                    vec![
                        node(
                            "a:off",
                            &[
                                ("x", &(width / 20).to_string()),
                                ("y", &source_top(sources, width, height).to_string()),
                            ],
                            vec![],
                        ),
                        node(
                            "a:ext",
                            &[
                                ("cx", &(width * 17 / 20).to_string()),
                                (
                                    "cy",
                                    &(height - source_top(sources, width, height) - height / 60)
                                        .to_string(),
                                ),
                            ],
                            vec![],
                        ),
                    ],
                )],
            ),
            node(
                "p:txBody",
                &[],
                vec![
                    node(
                        "a:bodyPr",
                        &[
                            ("wrap", "square"),
                            ("lIns", "0"),
                            ("rIns", "0"),
                            ("tIns", "0"),
                            ("bIns", "0"),
                            ("anchor", "t"),
                        ],
                        vec![],
                    ),
                    node("a:lstStyle", &[], vec![]),
                ]
                .into_iter()
                .chain(paragraphs)
                .collect(),
            ),
        ],
    ));
    Ok(xmldom::write(&dom))
}
fn strip_run_style(root: &mut El) {
    root.visit_mut(|root| {
    if matches!(root.name.as_str(), "a:rPr" | "a:defRPr" | "a:endParaRPr") {
        root.attrs
            .retain(|(n, _)| !matches!(n.as_str(), "sz" | "b" | "i"));
        root.children.retain(|n| !matches!(n, Node::El(e) if matches!(e.name.as_str(),"a:solidFill" | "a:latin" | "a:ea" | "a:cs")));
    }
        true
    });
}
fn scale(root: &mut El, width: i64, height: i64) {
    root.visit_mut(|root| {
        for (name, value) in &mut root.attrs {
            let factor = match name.as_str() {
                "x" | "cx" => Some((width, 12192000)),
                "y" | "cy" => Some((height, 6858000)),
                _ => None,
            };
            if let Some((n, d)) = factor
                && let Ok(v) = value.parse::<i64>()
            {
                *value = ((v as i128 * n as i128) / d as i128).to_string();
            }
        }
        true
    });
}
fn shape_name(shape: &El) -> &str {
    descendants(shape, "p:cNvPr")
        .first()
        .and_then(|e| e.attr("name"))
        .unwrap_or("")
}
fn apply_layout(
    dom: &mut El,
    layout: &El,
    width: i64,
    height: i64,
    image_layout: bool,
) -> Result<bool, Error> {
    for (name, value) in &layout.attrs {
        if name.starts_with("xmlns:") && dom.attr(name).is_none() {
            dom.set_attr(name, value);
        }
    }
    scale(dom, width, height);
    let c = dom.child_mut("p:cSld").ok_or_else(|| invalid("p:cSld"))?;
    c.children
        .retain(|n| !matches!(n,Node::El(e) if e.name == "p:bg"));
    let tree = c.child_mut("p:spTree").ok_or_else(|| invalid("p:spTree"))?;
    tree.children.retain(
        |n| !matches!(n,Node::El(e) if matches!(shape_name(e),"Theme accent" | "Slide number")),
    );
    let shapes: Vec<_> = descendants(layout, "p:sp")
        .into_iter()
        .filter(|s| placeholder(s).is_some())
        .collect();
    let mut used = Vec::new();
    let mut fallback = false;
    let caption_ids: Vec<String> = if image_layout {
        tree.elements()
            .filter(|s| shape_name(s) == "Content Placeholder")
            .filter_map(|s| {
                descendants(s, "p:cNvPr")
                    .first()?
                    .attr("id")
                    .map(str::to_string)
            })
            .collect()
    } else {
        Vec::new()
    };
    let caption_count = caption_ids.len();
    let mut caption_area = None;
    for shape in tree.elements_mut() {
        if !matches!(shape.name.as_str(), "p:sp" | "p:pic" | "p:graphicFrame") {
            continue;
        }
        let kind = ph_kind(shape).to_string();
        let code = crate::pptx::block_kind(shape) == Some("code");
        let aspect = if shape.name == "p:pic" {
            shape
                .child("p:spPr")
                .and_then(|p| p.child("a:xfrm"))
                .and_then(|x| x.child("a:ext"))
                .and_then(|e| {
                    Some(
                        e.attr("cx")?.parse::<f64>().ok()? * height as f64 * 12192000.0
                            / (e.attr("cy")?.parse::<f64>().ok()? * width as f64 * 6858000.0),
                    )
                })
        } else {
            None
        };
        if shape_name(shape) != "Source footnotes" && shape.name == "p:sp" {
            strip_run_style(shape);
        }
        if shape_name(shape) == "Source footnotes" {
            let sources: Vec<_> = descendants(shape, "a:t").iter().map(|s| s.text()).collect();
            let top = source_top(&sources, width, height);
            if let Some(xfrm) = shape
                .child_mut("p:spPr")
                .and_then(|p| p.child_mut("a:xfrm"))
            {
                if let Some(off) = xfrm.child_mut("a:off") {
                    off.set_attr("x", &(width / 20).to_string());
                    off.set_attr("y", &top.to_string());
                }
                if let Some(ext) = xfrm.child_mut("a:ext") {
                    ext.set_attr("cx", &(width * 17 / 20).to_string());
                    ext.set_attr("cy", &(height - top - height / 60).to_string());
                }
            }
            if let Some(footer) = shapes.iter().find(|s| ph_kind(s) == "ftr") {
                let style = descendants(footer, "a:defRPr")
                    .first()
                    .copied()
                    .or_else(|| descendants(footer, "a:rPr").first().copied())
                    .cloned();
                if let Some(mut style) = style {
                    style.name = "a:rPr".into();
                    style.set_attr("sz", "900");
                    shape.visit_mut(|root| {
                        if root.name == "a:rPr" {
                            *root = style.clone();
                            false
                        } else {
                            true
                        }
                    });
                }
            }
            continue;
        }
        let matched = shapes.iter().enumerate().find(|(i, s)| {
            !used.contains(i)
                && (if is_title(&kind) {
                    is_title(ph_kind(s))
                } else {
                    body(&kind) && body(ph_kind(s))
                })
        });
        if let Some((i, template)) = matched {
            used.push(i);
            if shape.name == "p:sp" {
                let template_style = descendants(template, "a:defRPr")
                    .first()
                    .copied()
                    .or_else(|| descendants(template, "a:endParaRPr").first().copied())
                    .cloned();
                let paragraphs = shape
                    .child("p:txBody")
                    .map(|tb| {
                        tb.children_named("a:p")
                            .cloned()
                            .map(|mut p| {
                                strip_run_style(&mut p);
                                if let Some(style) = &template_style {
                                    p.visit_mut(|root| {
                                        if root.name == "a:rPr" {
                                            *root = style.clone();
                                            root.name = "a:rPr".into();
                                            false
                                        } else {
                                            true
                                        }
                                    });
                                }
                                Node::El(p)
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let id = descendants(shape, "p:cNvPr")
                    .first()
                    .and_then(|e| e.attr("id"))
                    .unwrap_or("2")
                    .to_string();
                let anchor = shape
                    .child("p:txBody")
                    .and_then(|tb| tb.child("a:bodyPr"))
                    .and_then(|p| p.attr("anchor"))
                    .unwrap_or("t")
                    .to_string();
                let mut replacement = (*template).clone();
                if let Some(nv) = replacement
                    .child_mut("p:nvSpPr")
                    .and_then(|e| e.child_mut("p:cNvPr"))
                {
                    nv.set_attr("id", &id);
                }
                if replacement.child("p:txBody").is_none() {
                    replacement.children.push(node("p:txBody", &[], vec![]));
                }
                if let Some(tb) = replacement.child_mut("p:txBody") {
                    if tb.child("a:bodyPr").is_none() {
                        tb.children.insert(0, node("a:bodyPr", &[], vec![]));
                    }
                    // Company anchors (including master inheritance) win. Otherwise retain
                    // the generated top alignment or intentional title/section/quote setting.
                    if let Some(props) = tb.child_mut("a:bodyPr")
                        && props.attr("anchor").is_none()
                    {
                        props.set_attr("anchor", &anchor);
                    }
                    tb.children
                        .retain(|n| !matches!(n,Node::El(e) if e.name == "a:p"));
                    tb.children.extend(paragraphs);
                }
                crate::pptx::copy_block_metadata(shape, &mut replacement);
                *shape = replacement;
            } else if let Some(xfrm) = template.child("p:spPr").and_then(|p| p.child("a:xfrm")) {
                let name = if shape.name == "p:graphicFrame" {
                    "p:xfrm"
                } else {
                    "a:xfrm"
                };
                if let Some(destination) = if name == "p:xfrm" {
                    shape.child_mut(name)
                } else {
                    shape.child_mut("p:spPr").and_then(|p| p.child_mut(name))
                } {
                    *destination = xfrm.clone();
                    destination.name = name.into();
                }
            } else {
                fallback = true;
            }
        } else {
            fallback = true;
        }
        if code {
            // Company colors and sizing still apply; the code's font is semantic, not branding.
            shape.visit_mut(|el| {
                if matches!(el.name.as_str(), "a:rPr" | "a:defRPr" | "a:endParaRPr") {
                    el.children.retain(|n| {
                        !matches!(n, Node::El(e) if matches!(e.name.as_str(), "a:latin" | "a:ea" | "a:cs"))
                    });
                    for tag in ["a:latin", "a:ea", "a:cs"] {
                        el.children.push(node(
                            tag,
                            &[("typeface", "DejaVu Sans Mono")],
                            vec![],
                        ));
                    }
                    false
                } else {
                    true
                }
            });
        }
        if let Some(aspect) = aspect.filter(|a| a.is_finite() && *a > 0.0)
            && let Some(xfrm) = shape
                .child_mut("p:spPr")
                .and_then(|p| p.child_mut("a:xfrm"))
        {
            let bounds = |x: &El, child: &str, attr: &str| -> Result<i64, Error> {
                x.child(child)
                    .and_then(|e| e.attr(attr))
                    .and_then(|v| v.parse().ok())
                    .ok_or_else(|| invalid("image geometry"))
            };
            let x = bounds(xfrm, "a:off", "x")?;
            let y = bounds(xfrm, "a:off", "y")?;
            let w = bounds(xfrm, "a:ext", "cx")?;
            let mut h = bounds(xfrm, "a:ext", "cy")?;
            if x < 0
                || y < 0
                || x > width
                || y > height
                || w <= 0
                || h <= 0
                || w > width - x
                || h > height - y
            {
                return Err(invalid("image placeholder geometry"));
            }
            if caption_count > 0 {
                // Allow two minimum-size lines per caption, including inherited margins/spacing.
                let caption_h = (h / 5)
                    .max((caption_count as i64).saturating_mul(60 * 12700))
                    .min(h * 3 / 4);
                let image_h = h - caption_h;
                caption_area = Some((x, y + image_h, w, caption_h));
                h = image_h;
            }
            let (fw, fh) = if w as f64 / h.max(1) as f64 > aspect {
                ((h as f64 * aspect) as i64, h)
            } else {
                (w, (w as f64 / aspect) as i64)
            };
            if let Some(off) = xfrm.child_mut("a:off") {
                off.set_attr("x", &(x + (w - fw) / 2).to_string());
                off.set_attr("y", &(y + (h - fh) / 2).to_string());
            }
            if let Some(ext) = xfrm.child_mut("a:ext") {
                ext.set_attr("cx", &fw.to_string());
                ext.set_attr("cy", &fh.to_string());
            }
        }
    }
    if let Some((x, y, w, h)) = caption_area {
        for (i, caption) in tree
            .elements_mut()
            .filter(|s| {
                descendants(s, "p:cNvPr")
                    .first()
                    .and_then(|e| e.attr("id"))
                    .is_some_and(|id| caption_ids.iter().any(|saved| saved == id))
            })
            .enumerate()
        {
            if let Some(xfrm) = caption
                .child_mut("p:spPr")
                .and_then(|p| p.child_mut("a:xfrm"))
            {
                if let Some(off) = xfrm.child_mut("a:off") {
                    off.set_attr("x", &x.to_string());
                    off.set_attr("y", &(y + h * i as i64 / caption_count as i64).to_string());
                }
                if let Some(ext) = xfrm.child_mut("a:ext") {
                    ext.set_attr("cx", &w.to_string());
                    ext.set_attr("cy", &(h / caption_count as i64).to_string());
                }
            }
        }
    }
    Ok(fallback)
}

fn fit_content(
    dom: &mut El,
    sources: &[String],
    width: i64,
    height: i64,
    locale: &str,
) -> Vec<Warning> {
    fn clear_text(root: &mut El) {
        root.visit_mut(|root| {
            if matches!(root.name.as_str(), "p:txBody" | "a:txBody") {
                root.children
                    .retain(|n| !matches!(n, Node::El(e) if e.name == "a:p"));
                false
            } else {
                true
            }
        });
    }
    fn table_size(root: &mut El, width: i64, height: i64) {
        root.visit_mut(|root| {
            if root.name == "a:tbl" {
                let old_height: i64 = root
                    .children_named("a:tr")
                    .filter_map(|r| r.attr("h")?.parse::<i64>().ok())
                    .sum();
                for row in root.elements_mut().filter(|e| e.name == "a:tr") {
                    if let Some(h) = row.attr("h").and_then(|v| v.parse::<i64>().ok()) {
                        row.set_attr(
                            "h",
                            &((h as i128 * height as i128 / old_height.max(1) as i128) as i64)
                                .to_string(),
                        );
                    }
                }
                if let Some(grid) = root.child_mut("a:tblGrid") {
                    let old_width: i64 = grid
                        .elements()
                        .filter_map(|c| c.attr("w")?.parse::<i64>().ok())
                        .sum();
                    for col in grid.elements_mut() {
                        if let Some(w) = col.attr("w").and_then(|v| v.parse::<i64>().ok()) {
                            col.set_attr(
                                "w",
                                &((w as i128 * width as i128 / old_width.max(1) as i128) as i64)
                                    .to_string(),
                            );
                        }
                    }
                    // A smaller company body also narrows each cell. Scale its type before
                    // refitting, rather than keeping a default-sized word that breaks mid-word.
                    let ratio = (width as f64 / old_width.max(1) as f64).min(1.0);
                    root.visit_mut(|props| {
                        if matches!(props.name.as_str(), "a:rPr" | "a:defRPr" | "a:endParaRPr")
                            && let Some(size) = props.attr("sz").and_then(|s| s.parse::<i64>().ok())
                        {
                            let fitted = ((size as f64 * ratio) as i64).max(1400).min(size);
                            props.set_attr("sz", &fitted.to_string());
                        }
                        true
                    });
                }
                false
            } else {
                true
            }
        });
    }
    let mut warnings = Vec::new();
    let top = source_top(sources, width, height) - height / 75;
    let Some(tree) = dom
        .child_mut("p:cSld")
        .and_then(|c| c.child_mut("p:spTree"))
    else {
        return warnings;
    };
    for shape in tree.elements_mut() {
        if shape_name(shape) == "Source footnotes" {
            continue;
        }
        let title = is_title(ph_kind(shape));
        let xfrm = if shape.name == "p:graphicFrame" {
            shape.child_mut("p:xfrm")
        } else {
            shape
                .child_mut("p:spPr")
                .and_then(|p| p.child_mut("a:xfrm"))
        };
        let mut dimensions = None;
        if let Some(xfrm) = xfrm {
            let y = xfrm
                .child("a:off")
                .and_then(|e| e.attr("y"))
                .and_then(|v| v.parse::<i64>().ok());
            if let Some(ext) = xfrm.child_mut("a:ext") {
                let w = ext.attr("cx").and_then(|v| v.parse::<i64>().ok());
                let h = ext.attr("cy").and_then(|v| v.parse::<i64>().ok());
                if let (Some(w), Some(h)) = (w, h) {
                    let h = if sources.is_empty() {
                        h
                    } else {
                        h.min(top.saturating_sub(y.unwrap_or(top)))
                    }
                    .max(0);
                    ext.set_attr("cy", &h.to_string());
                    dimensions = Some((w, h));
                }
            }
        }
        if let Some((w, h)) = dimensions {
            table_size(shape, w, h);
        }
        let mut before = shape.clone();
        clear_text(&mut before);
        let mut overflow = Vec::new();
        crate::edited_text::refit(shape, &before, &[], 1, &mut overflow, locale);
        if overflow
            .iter()
            .any(|w| w.code == MessageCode::EditTextOverflow.as_str())
        {
            warnings.push(Warning::new(
                if title {
                    MessageCode::SlideTitleOverflow
                } else {
                    MessageCode::SlideTextOverflow
                },
                params(&[("slide", "1")]),
                locale,
            ));
        }
    }
    warnings
}

pub(crate) fn paginate(
    deck: Deck,
    theme: Theme,
    template: Option<&[u8]>,
    opts: &Options,
) -> Result<(Deck, Vec<Warning>), Error> {
    let Some(bytes) = template else {
        return crate::pptx::paginate(deck, theme, &opts.base_dir, &opts.locale);
    };
    let pkg = Package::open(bytes, opts.max_input_bytes)?;
    let layouts = layouts(&pkg)?;
    let pres = pkg.xml("ppt/presentation.xml")?;
    let (width, height) = slide_size(&pres)?;
    crate::pptx::paginate_using(deck, theme, &opts.base_dir, &opts.locale, |slide, xml| {
        let (layout, _) = select(&layouts, slide)?;
        let mut dom = xmldom::parse(xml)?;
        apply_layout(
            &mut dom,
            &layout.dom,
            width,
            height,
            slide.layout == Layout::Image,
        )?;
        Ok(fit_content(
            &mut dom,
            &slide.sources,
            width,
            height,
            &opts.locale,
        ))
    })
}

/// Compose additions exactly like a company-template export, then import only its new parts.
/// Existing parts (including slides and masters) stay byte-for-byte intact; the three package
/// indexes are merged instead of accepting the template writer's replacement slide list.
pub(crate) fn append(
    bytes: &[u8],
    additions: &[SlideInput],
    opts: &Options,
) -> Result<(Vec<u8>, Vec<Warning>), Error> {
    if additions.is_empty() {
        return Ok((bytes.to_vec(), Vec::new()));
    }
    let theme = crate::deck::theme("light");
    let (deck, mut warnings) = paginate(deck_from_slides(additions)?, theme, Some(bytes), opts)?;
    let (composed, export_warnings) = write(&deck, theme, Some(bytes), opts)?;
    warnings.extend(export_warnings);
    let mut pkg = Package::open(bytes, opts.max_input_bytes)?;
    let mut added = Package::open(
        &composed,
        opts.max_input_bytes.max(composed.len() as u64 * 50),
    )?;
    // Keep the existing editor's numbered asset names, without overwriting earlier assets.
    let mut renamed = std::collections::BTreeMap::new();
    for stem in [
        "ppt/media/image",
        "ppt/charts/chart",
        "ppt/embeddings/chart",
    ] {
        let mut next = pkg
            .0
            .keys()
            .filter_map(|p| p.strip_prefix(stem)?.split_once('.')?.0.parse::<u32>().ok())
            .max()
            .unwrap_or(0);
        for part in added.0.keys().filter(|p| !pkg.0.contains_key(*p)) {
            let Some((_, tail)) = part.split_once("/ppt/") else {
                continue;
            };
            let canonical = format!("ppt/{tail}");
            let Some(suffix) = canonical.strip_prefix(stem) else {
                continue;
            };
            let Some((number, extension)) = suffix.split_once('.') else {
                continue;
            };
            if number.parse::<u32>().is_err() || extension.ends_with("rels") {
                continue;
            }
            next = next
                .checked_add(1)
                .ok_or_else(|| invalid("asset number range"))?;
            renamed.insert(part.clone(), format!("{stem}{next}.{extension}"));
        }
    }
    let new_rels: Vec<_> = added
        .0
        .keys()
        .filter(|p| !pkg.0.contains_key(*p) && p.ends_with(".rels"))
        .cloned()
        .collect();
    for part in new_rels {
        let source = part
            .replace("/_rels/", "/")
            .trim_end_matches(".rels")
            .to_string();
        let mut rels = added.xml(&part)?;
        for rel in rels.elements_mut() {
            if rel.attr("TargetMode") != Some("External") {
                let resolved = target(&source, rel.attr("Target").ok_or_else(|| invalid(&part))?)?;
                rel.set_attr(
                    "Target",
                    &format!("/{}", renamed.get(&resolved).unwrap_or(&resolved)),
                );
            }
        }
        added.set_xml(&part, &rels);
    }
    let mut added_ct = added.xml("[Content_Types].xml")?;
    for entry in added_ct.elements_mut() {
        if let Some(path) = entry
            .attr("PartName")
            .and_then(|p| renamed.get(p.trim_start_matches('/')))
        {
            entry.set_attr("PartName", &format!("/{path}"));
        }
    }
    added.set_xml("[Content_Types].xml", &added_ct);
    for (old, new) in renamed {
        if let Some(data) = added.0.remove(&old) {
            added.0.insert(new.clone(), data);
        }
        if let Some(data) = added.0.remove(&rels_path(&old)) {
            added.0.insert(rels_path(&new), data);
        }
    }
    let mut pres = pkg.xml("ppt/presentation.xml")?;
    let mut rels = pkg.rels("ppt/presentation.xml")?;
    let added_pres = added.xml("ppt/presentation.xml")?;
    let mut remapped = std::collections::BTreeMap::new();
    for rel in added.rels("ppt/presentation.xml")?.elements().filter(|r| {
        r.attr("Type")
            .is_some_and(|t| t.ends_with("/slide") || t.ends_with("/notesMaster"))
    }) {
        let mut rel = rel.clone();
        let old = rel
            .attr("Id")
            .ok_or_else(|| invalid("relationship Id"))?
            .to_string();
        let id = crate::pptx::next_rid(&rels);
        rel.set_attr("Id", &id);
        remapped.insert(old, id);
        rels.children.push(Node::El(rel));
    }
    for (list_name, item_name) in [
        ("p:notesMasterIdLst", "p:notesMasterId"),
        ("p:sldIdLst", "p:sldId"),
    ] {
        for list in added_pres.children_named(list_name) {
            if pres.child(list_name).is_none() {
                let insert = pres.children.iter().position(|n| matches!(n, Node::El(e) if matches!(e.name.as_str(), "p:sldIdLst" | "p:sldSz"))).unwrap_or(pres.children.len());
                pres.children.insert(insert, node(list_name, &[], vec![]));
            }
            let destination = pres
                .child_mut(list_name)
                .ok_or_else(|| invalid(list_name))?;
            for item in list.children_named(item_name) {
                let mut item = item.clone();
                let id = item
                    .attr("r:id")
                    .and_then(|id| remapped.get(id))
                    .ok_or_else(|| invalid("r:id"))?;
                item.set_attr("r:id", id);
                if item_name == "p:sldId" {
                    let used: std::collections::BTreeSet<_> = destination
                        .elements()
                        .filter_map(|i| i.attr("id")?.parse::<u32>().ok())
                        .collect();
                    let next = used
                        .last()
                        .copied()
                        .unwrap_or(255)
                        .checked_add(1)
                        .filter(|i| *i <= 2147483647)
                        .or_else(|| (256..=2147483647).find(|i| !used.contains(i)))
                        .ok_or_else(|| invalid("slide id range"))?;
                    item.set_attr("id", &next.to_string());
                }
                destination.children.push(Node::El(item));
            }
        }
    }
    let mut ct = pkg.xml("[Content_Types].xml")?;
    for entry in added.xml("[Content_Types].xml")?.elements() {
        let key = if entry.name == "Override" {
            "PartName"
        } else {
            "Extension"
        };
        if !ct
            .elements()
            .any(|old| old.name == entry.name && old.attr(key) == entry.attr(key))
        {
            ct.children.push(Node::El(entry.clone()));
        }
    }
    for (part, data) in added.0 {
        // The template writer reserves a fresh generatedN/ prefix, including media, charts,
        // workbooks and notes. Never overwrite an original part while importing additions.
        pkg.0.entry(part).or_insert(data);
    }
    pkg.set_xml("ppt/presentation.xml", &pres);
    pkg.set_xml(&rels_path("ppt/presentation.xml"), &rels);
    pkg.set_xml("[Content_Types].xml", &ct);
    Ok((pkg.finish()?, warnings))
}

fn slide_size(pres: &El) -> Result<(i64, i64), Error> {
    pres.child("p:sldSz")
        .and_then(|s| {
            Some((
                s.attr("cx")?.parse::<i64>().ok()?,
                s.attr("cy")?.parse::<i64>().ok()?,
            ))
        })
        // Keep subsequent EMU scaling arithmetic representable even for hostile packages.
        .filter(|(w, h)| *w > 0 && *h > 0 && *w <= i64::MAX / 100 && *h <= i64::MAX / 100)
        .ok_or_else(|| invalid("p:sldSz"))
}

pub(crate) fn write(
    deck: &Deck,
    theme: Theme,
    template: Option<&[u8]>,
    opts: &Options,
) -> Result<(Vec<u8>, Vec<Warning>), Error> {
    let (generated, warnings) = crate::pptx::write(deck, theme, &opts.base_dir, &opts.locale)?;
    let Some(template) = template else {
        return Ok((generated, warnings));
    };
    let mut warnings = warnings;
    warnings.push(Warning::new(
        MessageCode::TemplateContentReview,
        vec![],
        &opts.locale,
    ));
    let mut pkg = Package::open(template, opts.max_input_bytes)?;
    // Reserve names against the original package, before deleting its sample slides.
    let prefix = (1u64..)
        .map(|i| format!("generated{i}/"))
        .find(|s| !pkg.0.keys().any(|p| p.starts_with(s)))
        .ok_or_else(|| invalid("part prefix"))?;
    let generated = Package::open(
        &generated,
        opts.max_input_bytes.max(generated.len() as u64 * 50),
    )?;
    let layouts = layouts(&pkg)?;
    let mut pres = pkg.xml("ppt/presentation.xml")?;
    let (width, height) = slide_size(&pres)?;
    let mut ct = pkg.xml("[Content_Types].xml")?;
    let discard: Vec<String> = ct
        .children_named("Override")
        .filter(|o| {
            o.attr("ContentType")
                .is_some_and(|s| s.ends_with(".slide+xml") || s.ends_with(".notesSlide+xml"))
        })
        .filter_map(|o| {
            o.attr("PartName")
                .map(|p| p.trim_start_matches('/').to_string())
        })
        .collect();
    for part in &discard {
        pkg.0.remove(part);
        pkg.0.remove(&rels_path(part));
    }
    ct.children.retain(|n| !matches!(n,Node::El(e) if e.attr("PartName").is_some_and(|p| discard.iter().any(|d| p.trim_start_matches('/') == d))));
    for o in ct.elements_mut() {
        if o.attr("PartName") == Some("/ppt/presentation.xml") {
            o.set_attr("ContentType","application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml");
        }
    }
    pres.children.retain(|n| !matches!(n,Node::El(e) if matches!(e.name.as_str(),"p:sldIdLst" | "p:notesMasterIdLst" | "p:custShowLst" | "p:extLst")));
    let mut rels = pkg.rels("ppt/presentation.xml")?;
    rels.children.retain(|n| !matches!(n,Node::El(e) if e.attr("Type").is_some_and(|s| s.ends_with("/slide") || s.ends_with("/notesMaster"))));
    let import = |p: &str| {
        p.starts_with("ppt/")
            && !p.starts_with("ppt/slideMasters/")
            && !p.starts_with("ppt/slideLayouts/")
            && !p.starts_with("ppt/theme/")
            && p != "ppt/presentation.xml"
            && p != "ppt/_rels/presentation.xml.rels"
    };
    for (p, data) in &generated.0 {
        if import(p) {
            let data = if p.starts_with("ppt/charts/") && p.ends_with(".xml") {
                crate::charts::company_style(data, theme)?
            } else {
                data.clone()
            };
            pkg.0.insert(format!("{prefix}{p}"), data);
        }
    }
    for entry in generated.xml("[Content_Types].xml")?.elements() {
        if entry.name == "Override"
            && let Some(p) = entry.attr("PartName").map(|p| p.trim_start_matches('/'))
            && import(p)
        {
            let mut entry = entry.clone();
            entry.set_attr("PartName", &format!("/{prefix}{p}"));
            ct.children.push(Node::El(entry));
        } else if entry.name == "Default"
            && !ct
                .children_named("Default")
                .any(|e| e.attr("Extension") == entry.attr("Extension"))
        {
            ct.children.push(Node::El(entry.clone()));
        }
    }
    let mut ids = el("p:sldIdLst", &[], vec![]);
    for (index, part) in generated.slides()?.iter().enumerate() {
        let slide = deck.slides.get(index).ok_or_else(|| invalid(part))?;
        let (layout, fallback) = select(&layouts, slide)?;
        if fallback {
            warnings.push(Warning::new(
                MessageCode::TemplateFallback,
                params(&[
                    ("slide", &(index + 1).to_string()),
                    (
                        "requested",
                        slide
                            .layout_name
                            .as_deref()
                            .unwrap_or(slide.layout.as_str()),
                    ),
                    ("selected", &layout.name),
                ]),
                &opts.locale,
            ));
        }
        let mut dom = generated.xml(part)?;
        // Imported additions must not share a logical-table identity with the existing deck.
        dom.visit_mut(|el| {
            if el.name == "sl:table"
                && let Some(id) = el.attr("id").map(str::to_string)
            {
                el.set_attr("id", &format!("{prefix}{id}"));
            }
            true
        });
        let mut sr = generated.rels(part)?;
        let mut layout_dom = layout.dom.clone();
        for (i, r) in pkg.rels(&layout.part)?.elements().enumerate() {
            if r.attr("Type").is_some_and(|t| t.ends_with("/slideMaster")) {
                continue;
            }
            let mut r = r.clone();
            let old = r
                .attr("Id")
                .ok_or_else(|| invalid(&layout.part))?
                .to_string();
            let id = format!("companyLayout{i}");
            r.set_attr("Id", &id);
            if r.attr("TargetMode") != Some("External") {
                let resolved = target(
                    &layout.part,
                    r.attr("Target").ok_or_else(|| invalid(&layout.part))?,
                )?;
                r.set_attr("Target", &format!("/{resolved}"));
            }
            layout_dom.visit_mut(|root| {
                for (name, value) in &mut root.attrs {
                    if matches!(name.as_str(), "r:embed" | "r:link" | "r:id") && value == &old {
                        *value = id.clone();
                    }
                }
                true
            });
            sr.children.push(Node::El(r));
        }
        if apply_layout(
            &mut dom,
            &layout_dom,
            width,
            height,
            slide.layout == Layout::Image,
        )? {
            warnings.push(Warning::new(
                MessageCode::TemplatePlaceholderFallback,
                params(&[("slide", &(index + 1).to_string())]),
                &opts.locale,
            ));
        }
        warnings.extend(fit_content(
            &mut dom,
            &slide.sources,
            width,
            height,
            &opts.locale,
        ));
        pkg.set_xml(&format!("{prefix}{part}"), &dom);
        for r in sr.elements_mut() {
            if r.attr("Type").is_some_and(|t| t.ends_with("/slideLayout")) {
                r.set_attr("Target", &format!("/{}", layout.part));
            }
        }
        pkg.set_xml(&rels_path(&format!("{prefix}{part}")), &sr);
        let rid = format!("slidesGenerated{}", index + 1);
        rels.children.push(node(
            "Relationship",
            &[
                ("Id", &rid),
                (
                    "Type",
                    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide",
                ),
                ("Target", &format!("/{prefix}{part}")),
            ],
            vec![],
        ));
        ids.children.push(node(
            "p:sldId",
            &[("id", &(256 + index).to_string()), ("r:id", &rid)],
            vec![],
        ));
    }
    // Keep imported notes linked to an existing company theme, not a missing generated theme.
    let theme_part = pkg
        .related(&layouts[0].part, "slideMaster")?
        .first()
        .map(|(_, p)| p.clone())
        .ok_or_else(|| invalid("slide master"))?;
    let company_theme = pkg
        .related(&theme_part, "theme")?
        .first()
        .map(|(_, p)| p.clone())
        .ok_or_else(|| invalid("theme"))?;
    for (p, _) in generated
        .0
        .iter()
        .filter(|(p, _)| import(p) && p.ends_with(".rels"))
    {
        let mut rr = pkg.xml(&format!("{prefix}{p}"))?;
        let mut changed = false;
        for r in rr.elements_mut() {
            if r.attr("Type").is_some_and(|t| t.ends_with("/theme")) {
                r.set_attr("Target", &format!("/{company_theme}"));
                changed = true;
            }
        }
        if changed {
            pkg.set_xml(&format!("{prefix}{p}"), &rr);
        }
    }
    for (i, (_, p)) in generated
        .related("ppt/presentation.xml", "notesMaster")?
        .iter()
        .enumerate()
    {
        let rid = format!("slidesNotes{i}");
        rels.children.push(node("Relationship", &[("Id",&rid),("Type","http://schemas.openxmlformats.org/officeDocument/2006/relationships/notesMaster"),("Target",&format!("/{prefix}{p}"))],vec![]));
        pres.children.push(node(
            "p:notesMasterIdLst",
            &[],
            vec![node("p:notesMasterId", &[("r:id", &rid)], vec![])],
        ));
    }
    // Presentation child order follows the OOXML schema: masters, notes masters, slides, size.
    let insert = pres
        .children
        .iter()
        .position(|n| matches!(n,Node::El(e) if e.name == "p:sldSz"))
        .unwrap_or(pres.children.len());
    pres.children.insert(insert, Node::El(ids));
    if let Some(i) = pres
        .children
        .iter()
        .position(|n| matches!(n,Node::El(e) if e.name == "p:notesMasterIdLst"))
    {
        let notes = pres.children.remove(i);
        let insert = pres
            .children
            .iter()
            .position(|n| matches!(n,Node::El(e) if e.name == "p:sldIdLst"))
            .unwrap_or(pres.children.len());
        pres.children.insert(insert, notes);
    }
    pkg.set_xml("ppt/presentation.xml", &pres);
    pkg.set_xml(&rels_path("ppt/presentation.xml"), &rels);
    pkg.set_xml("[Content_Types].xml", &ct);
    // Sample-slide assets and generated presentation-only properties are not part of the new
    // deck unless reachable through its actual relationships.
    pkg.prune_unreachable()?;
    pkg.validate_relationships()?;
    Ok((pkg.finish()?, warnings))
}
