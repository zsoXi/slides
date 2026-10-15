//! Ordered DrawingML paragraph text shared by reading, replacement and fitting.
//! The original inline elements carry formatting and field metadata; offsets are UTF-8 bytes.

use crate::xmldom::{El, Node};

struct Inline<'a> {
    node: &'a El,
    start: usize,
    end: usize,
}

pub(crate) struct Paragraph<'a> {
    pub text: String,
    inlines: Vec<Inline<'a>>,
}

fn inline_text(node: &El) -> Option<String> {
    match node.name.as_str() {
        "a:r" | "a:fld" => Some(node.child("a:t").map(El::text).unwrap_or_default()),
        "a:br" => Some("\n".into()),
        "a:tab" => Some("\t".into()),
        _ => None,
    }
}

impl<'a> Paragraph<'a> {
    pub fn read(para: &'a El) -> Self {
        let mut text = String::new();
        let mut inlines = Vec::new();
        for node in para.elements() {
            if let Some(value) = inline_text(node) {
                let start = text.len();
                text.push_str(&value);
                inlines.push(Inline {
                    node,
                    start,
                    end: text.len(),
                });
            }
        }
        Self { text, inlines }
    }
}

fn with_text(template: &El, value: &str) -> El {
    let mut run = template.clone();
    if !matches!(run.name.as_str(), "a:r" | "a:fld") {
        run.name = "a:r".into();
        run.attrs.clear();
        run.children
            .retain(|n| matches!(n, Node::El(e) if e.name == "a:rPr"));
    }
    if let Some(text) = run.child_mut("a:t") {
        text.children = vec![Node::Text(value.into())];
    } else {
        run.children.push(crate::ooxml::text("a:t", value));
    }
    run
}

fn replacement(template: &El, value: &str) -> Vec<Node> {
    // A field has one identity and one cached value, even when its display text has line breaks.
    if template.name == "a:fld" {
        return vec![Node::El(with_text(template, value))];
    }
    let mut result = Vec::new();
    for (i, line) in value.split('\n').enumerate() {
        if i > 0 {
            result.push(crate::ooxml::node(
                "a:br",
                &[],
                template
                    .child("a:rPr")
                    .cloned()
                    .map(Node::El)
                    .into_iter()
                    .collect(),
            ));
        }
        if !line.is_empty() {
            result.push(Node::El(with_text(template, line)));
        }
    }
    result
}

/// Replace across inline boundaries while keeping untouched runs and the first touched style.
/// Walk original byte ranges once, so repeated matches do not repeatedly clone the paragraph.
pub(crate) fn replace(para: &mut El, find: &str, value: &str) -> usize {
    if find.is_empty() {
        return 0;
    }
    let model = Paragraph::read(para);
    let count = model.text.matches(find).count();
    if find == value || count == 0 {
        return count;
    }
    let mut matches = model.text.match_indices(find).peekable();
    let mut spans = model.inlines.iter();
    let mut children = Vec::new();
    for node in &para.children {
        let Node::El(el) = node else {
            children.push(node.clone());
            continue;
        };
        if inline_text(el).is_none() {
            children.push(node.clone());
            continue;
        }
        let Some(span) = spans.next() else {
            continue;
        };
        let mut cursor = span.start;
        if cursor == span.end {
            children.push(node.clone());
        }
        while cursor < span.end {
            let next_match = matches.peek().map(|(start, _)| *start);
            let stop = next_match.unwrap_or(span.end).min(span.end);
            if cursor < stop {
                if cursor == span.start && stop == span.end {
                    children.push(node.clone());
                } else {
                    children.push(Node::El(with_text(span.node, &model.text[cursor..stop])));
                }
                cursor = stop;
            }
            if let Some(start) = next_match.filter(|s| *s < span.end) {
                if start >= span.start {
                    children.extend(replacement(span.node, value));
                }
                let end = start + find.len();
                cursor = end.min(span.end);
                if end <= span.end {
                    matches.next();
                }
            }
        }
    }
    // Prefix/replacement/suffix slices of the same field must not duplicate its UUID.
    let mut merged: Vec<Node> = Vec::new();
    for node in children {
        if let Node::El(field) = &node
            && field.name == "a:fld"
            && let Some(Node::El(previous)) = merged.last_mut()
            && previous.name == "a:fld"
            && previous.attrs == field.attrs
            && let (Some(target), Some(source)) = (previous.child_mut("a:t"), field.child("a:t"))
        {
            target.children.extend(source.children.clone());
            continue;
        }
        merged.push(node);
    }
    para.children = merged;
    count
}

/// Change only paragraph text, retaining paragraph/end properties and the first inline style.
pub(crate) fn set_text(para: &mut El, value: &str) {
    let original = Paragraph::read(para).text;
    if original == value {
        return;
    }
    if !original.is_empty() {
        replace(para, &original, value);
        return;
    }
    let template = para
        .elements()
        .find(|e| inline_text(e).is_some())
        .cloned()
        .unwrap_or_else(|| crate::ooxml::el("a:r", &[], vec![]));
    let at = para
        .children
        .iter()
        .position(|n| matches!(n, Node::El(e) if e.name == "a:endParaRPr"))
        .unwrap_or(para.children.len());
    para.children.splice(at..at, replacement(&template, value));
}
