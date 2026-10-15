//! Conservative fitting of changed foreign text bodies without resizing foreign shapes.

use crate::messages::{MessageCode, Warning, params};
use crate::pptx::{Rect, wrap_text};
use crate::xmldom::{El, Node};

fn number(el: &El, attr: &str) -> Option<i64> {
    el.attr(attr)?.parse().ok()
}

fn element(name: &str) -> El {
    El {
        spelling: Default::default(),
        name: name.into(),
        attrs: Vec::new(),
        children: Vec::new(),
    }
}

fn placeholder(shape: &El) -> Option<&El> {
    shape.child("p:nvSpPr")?.child("p:nvPr")?.child("p:ph")
}

fn inherited_shape<'a>(root: &'a El, shape: &El) -> Option<&'a El> {
    let ph = placeholder(shape)?;
    let mut shapes = Vec::new();
    root.descendants("p:sp", &mut shapes);
    shapes.into_iter().find(|s| {
        placeholder(s).is_some_and(|other| {
            let idx = ph.attr("idx").unwrap_or("0");
            other.attr("idx").unwrap_or("0") == idx
                && (idx != "0"
                    || other.attr("type").unwrap_or("obj") == ph.attr("type").unwrap_or("obj"))
        })
    })
}

fn extent(shape: &El) -> Option<Rect> {
    let xfrm = shape.child("p:spPr")?.child("a:xfrm")?;
    let ext = xfrm.child("a:ext")?;
    Some(Rect {
        x: 0,
        y: 0,
        w: number(ext, "cx")?,
        h: number(ext, "cy")?,
    })
}

fn max_size(root: &El) -> Option<f64> {
    let mut sizes = Vec::new();
    for name in ["a:rPr", "a:defRPr", "a:endParaRPr"] {
        let mut props = Vec::new();
        root.descendants(name, &mut props);
        sizes.extend(
            props
                .into_iter()
                .filter_map(|p| number(p, "sz"))
                .filter(|s| *s > 0),
        );
    }
    sizes.into_iter().max().map(|s| s as f64 / 100.0)
}

fn paragraph_text(el: &El, out: &mut String) {
    out.push_str(&crate::paragraph::Paragraph::read(el).text);
}

fn lines(tb: &El, rect: Rect, size: f64) -> Vec<(String, f64)> {
    let mut result = Vec::new();
    for para in tb.children_named("a:p") {
        let mut text = String::new();
        paragraph_text(para, &mut text);
        let props = para.child("a:pPr");
        let indent = props.and_then(|p| number(p, "marL")).unwrap_or(0).max(0);
        let available = Rect {
            w: rect.w.saturating_sub(indent),
            ..rect
        };
        let spacing = |name: &str| props.and_then(|p| p.child(name));
        let points = |name: &str| {
            spacing(name)
                .and_then(|s| s.child("a:spcPts"))
                .and_then(|s| number(s, "val"))
                .unwrap_or(0)
                .max(0) as f64
                / 100.0
        };
        let percent = spacing("a:lnSpc")
            .and_then(|s| s.child("a:spcPct"))
            .and_then(|s| number(s, "val"))
            .unwrap_or(120000)
            .max(120000) as f64
            / 120000.0;
        let leading = (size * 1.7 * percent).max(points("a:lnSpc"));
        let wrapped = wrap_text(&text, available, size, 0);
        let count = wrapped.split('\n').count();
        for (i, line) in wrapped.split('\n').enumerate() {
            let before = if i == 0 { points("a:spcBef") } else { 0.0 };
            let after = if i + 1 == count {
                points("a:spcAft").max(10.0)
            } else {
                0.0
            };
            result.push((line.to_string(), leading + before + after));
        }
    }
    result
}

fn set_size(el: &mut El, size: f64) {
    el.visit_mut(|el| {
        if matches!(el.name.as_str(), "a:rPr" | "a:defRPr" | "a:endParaRPr") {
            // Do not enlarge intentionally small text; preserve other run properties.
            let original = number(el, "sz")
                .filter(|n| *n > 0)
                .unwrap_or((size * 100.0) as i64);
            el.set_attr("sz", &original.min((size * 100.0) as i64).to_string());
        }
        if matches!(el.name.as_str(), "a:r" | "a:fld") && el.child("a:rPr").is_none() {
            let mut props = element("a:rPr");
            props.set_attr("sz", &((size * 100.0) as i64).to_string());
            el.children.insert(0, Node::El(props));
        }
        true
    });
}

fn fit_body(
    shape: &mut El,
    parents: &[&El],
    rect: Option<Rect>,
    slide: usize,
    warnings: &mut Vec<Warning>,
    locale: &str,
) {
    let name = if shape.child("p:txBody").is_some() {
        "p:txBody"
    } else {
        "a:txBody"
    };
    let Some(body) = shape.child(name) else {
        return;
    };
    let start = max_size(body)
        .or_else(|| parents.iter().find_map(|s| max_size(s)))
        .unwrap_or(24.0);
    let minimum = start.min(14.0);
    let mut size = start;
    let mut rect = rect;
    if let Some(r) = &mut rect {
        let inset = |key: &str, default: i64| {
            body.child("a:bodyPr")
                .and_then(|p| number(p, key))
                .or_else(|| {
                    parents.iter().find_map(|s| {
                        s.child("p:txBody")?
                            .child("a:bodyPr")
                            .and_then(|p| number(p, key))
                    })
                })
                .unwrap_or(default)
                .max(0)
        };
        r.w =
            r.w.saturating_sub(inset("lIns", 91440))
                .saturating_sub(inset("rIns", 91440));
        r.h =
            r.h.saturating_sub(inset("tIns", 45720))
                .saturating_sub(inset("bIns", 45720));
    }
    let mut hidden_words = body
        .children_named("a:p")
        .map(|p| {
            let mut text = String::new();
            paragraph_text(p, &mut text);
            text.split_whitespace().count()
        })
        .sum::<usize>();
    let total_words = hidden_words;
    // Unusual vertical/rotated bodies are not safely measurable by this horizontal fitter.
    if body.child("a:bodyPr").is_some_and(|p| {
        p.attr("vert").is_some_and(|v| v != "horz") || p.attr("rot").is_some_and(|v| v != "0")
    }) {
        rect = None;
    }
    let mut fits = hidden_words == 0;
    if let Some(r) = rect {
        loop {
            let wrapped = lines(body, r, size);
            // Reserve extra paragraph spacing and substitution-font leading. This is a
            // conservative capacity estimate, not a claim to shape arbitrary Office fonts.
            let mut used = 0.0;
            let capacity = wrapped
                .iter()
                .take_while(|(_, height)| {
                    used += height;
                    used <= r.h.max(0) as f64 / 12700.0
                })
                .count();
            fits = wrapped.len() <= capacity && r.w as f64 / 12700.0 >= size * 2.0;
            if fits {
                hidden_words = 0;
                break;
            }
            if size <= minimum {
                hidden_words = if r.w as f64 / 12700.0 < size * 2.0 {
                    hidden_words
                } else {
                    wrapped
                        .iter()
                        .skip(capacity)
                        .map(|(s, _)| s.split_whitespace().count())
                        .sum::<usize>()
                        .max(1)
                        .min(total_words)
                };
                break;
            }
            size = (size - 1.0).max(minimum);
        }
    } else {
        size = minimum;
    }
    // A fitting edit changes text only, including inherited typography and autofit policy.
    if fits && size == start {
        return;
    }
    let Some(body) = shape.child_mut(name) else {
        return;
    };
    set_size(body, size);
    if body.child("a:bodyPr").is_none() {
        body.children.insert(0, Node::El(element("a:bodyPr")));
    }
    if let Some(props) = body.child_mut("a:bodyPr") {
        // Request fixed bounds and clipping rather than shape growth. Keep all source text
        // and warn when it cannot fit; table renderers may still expand their own row heights.
        props.set_attr("wrap", "square");
        props.set_attr("vertOverflow", "clip");
        props.set_attr("horzOverflow", "clip");
        props.children.retain(|n| !matches!(n, Node::El(e) if matches!(e.name.as_str(), "a:spAutoFit" | "a:noAutofit" | "a:normAutofit")));
        // LibreOffice recomputes normAutofit even with fontScale=100000, shrinking below
        // the minimum. Freeze the minimum in the overflow case and clip instead.
        let mut autofit = element(if fits { "a:normAutofit" } else { "a:noAutofit" });
        if fits {
            autofit.set_attr("fontScale", "100000");
        }
        props.children.push(Node::El(autofit));
    }
    if fits {
        warnings.push(Warning::new(
            MessageCode::EditTextFitted,
            params(&[("slide", &slide.to_string())]),
            locale,
        ));
    } else {
        warnings.push(Warning::new(
            MessageCode::EditTextOverflow,
            params(&[
                ("slide", &slide.to_string()),
                ("hidden_words", &hidden_words.to_string()),
            ]),
            locale,
        ));
    }
}

fn changed_body(current: &El, before: &El) -> bool {
    ["p:txBody", "a:txBody"]
        .iter()
        .any(|name| match (current.child(name), before.child(name)) {
            (Some(a), Some(b)) => crate::xmldom::write(a) != crate::xmldom::write(b),
            (Some(_), None) => true,
            _ => false,
        })
}

/// Compare text bodies rather than entire slides, so unrelated shapes retain their styling.
pub(crate) fn refit(
    current: &mut El,
    before: &El,
    inherited: &[El],
    slide: usize,
    warnings: &mut Vec<Warning>,
    locale: &str,
) {
    let mut pending = vec![(current, before)];
    while let Some((current, before)) = pending.pop() {
        if current.name == "p:sp" {
            if changed_body(current, before) {
                let parents = inherited
                    .iter()
                    .filter_map(|root| inherited_shape(root, current))
                    .collect::<Vec<_>>();
                let rect = extent(current).or_else(|| parents.iter().find_map(|s| extent(s)));
                fit_body(current, &parents, rect, slide, warnings, locale);
            }
            continue;
        }
        if current.name == "a:tbl" {
            let widths = current
                .child("a:tblGrid")
                .map(|g| {
                    g.children_named("a:gridCol")
                        .map(|c| number(c, "w").unwrap_or(0))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            for (row, old_row) in current
                .elements_mut()
                .filter(|e| e.name == "a:tr")
                .zip(before.children_named("a:tr"))
            {
                let height = number(row, "h").unwrap_or(0);
                let mut col = 0;
                for (cell, old_cell) in row
                    .elements_mut()
                    .filter(|e| e.name == "a:tc")
                    .zip(old_row.children_named("a:tc"))
                {
                    let span = number(cell, "gridSpan").unwrap_or(1).max(1) as usize;
                    let width = widths
                        .iter()
                        .skip(col)
                        .take(span)
                        .fold(0i64, |a, b| a.saturating_add(*b));
                    col = col.saturating_add(span);
                    if changed_body(cell, old_cell) {
                        fit_body(
                            cell,
                            &[],
                            Some(Rect {
                                x: 0,
                                y: 0,
                                w: width,
                                h: height,
                            }),
                            slide,
                            warnings,
                            locale,
                        );
                    }
                }
            }
            continue;
        }
        let children: Vec<_> = current
            .elements_mut()
            .zip(before.elements())
            .filter(|(child, old_child)| child.name == old_child.name)
            .collect();
        pending.extend(children.into_iter().rev());
    }
}
