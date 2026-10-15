//! Markdown <-> presentation model.

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use crate::deck::{Block, Chart, Deck, Layout, Slide, Table};

struct State {
    finished: Vec<Slide>,
    cur: Slide,
    started: bool,
    text: String,
    in_quote: usize,
    in_item: usize,
    in_code: bool,
    in_heading: bool,
    in_table: bool,
    table: Table,
    row: Vec<String>,
    image: Option<String>,
    current_layout: Layout,
    /// Layouts chosen with `<!-- layout: ... -->`, parallel to `finished` plus the current one.
    explicit: Vec<bool>,
    explicit_cur: bool,
    pending_layout: Option<Layout>,
}

impl State {
    fn new() -> Self {
        State {
            finished: Vec::new(),
            cur: Slide {
                layout: Layout::Section,
                ..Slide::default()
            },
            started: false,
            text: String::new(),
            in_quote: 0,
            in_item: 0,
            in_code: false,
            in_heading: false,
            in_table: false,
            table: Table::default(),
            row: Vec::new(),
            image: None,
            current_layout: Layout::Section,
            explicit: Vec::new(),
            explicit_cur: false,
            pending_layout: None,
        }
    }

    fn slide(&mut self) -> &mut Slide {
        if !self.started
            && let Some(layout) = self.pending_layout.take()
        {
            self.cur.layout = layout;
            self.current_layout = layout;
            self.explicit_cur = true;
        }
        self.started = true;
        &mut self.cur
    }

    fn pristine(&self) -> bool {
        self.cur.title.is_empty() && self.cur.blocks.is_empty() && self.cur.notes.is_empty()
    }

    fn begin_slide(&mut self, layout: Layout) {
        // A heading right after `---` reuses the still-empty slide the rule started.
        if self.started && self.pristine() {
            if let Some(pending) = self.pending_layout.take() {
                self.cur.layout = pending;
                self.current_layout = pending;
                self.explicit_cur = true;
            } else if !self.explicit_cur {
                self.cur.layout = layout;
                self.current_layout = layout;
            }
            return;
        }
        let was_pending = self.pending_layout.is_some();
        let layout = self.pending_layout.take().unwrap_or(layout);
        if self.started {
            let explicit = self.explicit_cur;
            let done = std::mem::replace(
                &mut self.cur,
                Slide {
                    layout,
                    ..Slide::default()
                },
            );
            self.finished.push(done);
            self.explicit.push(explicit);
        } else {
            self.cur = Slide {
                layout,
                ..Slide::default()
            };
        }
        self.explicit_cur = was_pending;
        self.current_layout = layout;
        self.started = true;
    }

    /// A `<!-- layout: name -->` comment. Before any content of the current slide it sets its
    /// layout; after content (or before the first slide) it applies to the next slide.
    fn set_layout_comment(&mut self, layout: Layout) {
        // A comment right after a heading (title set, no body yet) belongs to this slide.
        let empty = self.cur.blocks.is_empty() && self.cur.notes.is_empty();
        if self.started && empty {
            self.cur.layout = layout;
            self.current_layout = layout;
            self.explicit_cur = true;
            self.pending_layout = None;
        } else {
            self.pending_layout = Some(layout);
        }
    }

    /// A `---` rule always starts a new slide, even without a following heading.
    fn rule(&mut self) {
        if self.started && !self.pristine() {
            self.begin_slide(Layout::TitleContent);
        }
    }

    fn take_text(&mut self) -> String {
        let t = self.text.trim().to_string();
        self.text.clear();
        t
    }

    fn push_bullet(&mut self, mut text: String) {
        if text.is_empty() {
            return;
        }
        text = format!("{}{}", "  ".repeat(self.in_item.saturating_sub(1)), text);
        let s = self.slide();
        match s.blocks.last_mut() {
            Some(Block::Bullets(list)) => list.push(text),
            _ => s.blocks.push(Block::Bullets(vec![text])),
        }
    }

    fn flush(&mut self) {
        let text = self.take_text();
        if text.is_empty() {
            return;
        }
        if self.in_quote > 0 && self.cur.layout != Layout::Quote {
            let s = self.slide();
            if !s.notes.is_empty() {
                s.notes.push('\n');
            }
            s.notes.push_str(&text);
        } else if self.in_item > 0 {
            self.push_bullet(text);
        } else {
            self.slide().blocks.push(Block::Text(text));
        }
    }

    fn into_deck(mut self) -> Deck {
        if self.started {
            let cur = std::mem::take(&mut self.cur);
            self.finished.push(cur);
            self.explicit.push(self.explicit_cur);
        }
        let explicit = self.explicit.clone();
        let deck = Deck {
            slides: self.finished,
            ..Deck::default()
        };
        // Drop slides left empty by a trailing `---` or layout comment.
        let mut kept: Vec<(Slide, bool)> = Vec::new();
        for (i, slide) in deck.slides.into_iter().enumerate() {
            let is_explicit = explicit.get(i).copied().unwrap_or(false);
            let empty = slide.title.is_empty() && slide.blocks.is_empty() && slide.notes.is_empty();
            if !empty || is_explicit {
                kept.push((slide, is_explicit));
            }
        }
        let slides: Vec<Slide> = kept
            .into_iter()
            .map(|(mut slide, is_explicit)| {
                if !is_explicit {
                    if !slide.blocks.is_empty() && slide.layout != Layout::Title {
                        slide.layout = match slide.blocks.as_slice() {
                            [Block::Image(_)] => Layout::Image,
                            [Block::Table(_)] => Layout::Table,
                            [Block::Chart(_)] => Layout::Chart,
                            _ => Layout::TitleContent,
                        };
                    } else if slide.blocks.is_empty() && slide.layout != Layout::Title {
                        slide.layout = Layout::Section;
                    }
                }
                // The first paragraph of a title slide is its subtitle.
                if slide.layout == Layout::Title
                    && slide.subtitle.is_empty()
                    && matches!(slide.blocks.first(), Some(Block::Text(_)))
                {
                    if let Some(Block::Text(text)) = slide.blocks.first() {
                        slide.subtitle = text.clone();
                    }
                    slide.blocks.remove(0);
                }
                slide
            })
            .collect();
        Deck {
            slides,
            ..Deck::default()
        }
    }
}

fn parse_layout_comment(html: &str) -> Option<Layout> {
    let inner = html
        .trim()
        .strip_prefix("<!--")?
        .strip_suffix("-->")?
        .trim();
    let name = inner.strip_prefix("layout:")?.trim();
    Layout::parse(name)
}

pub fn parse(markdown: &str) -> Result<Deck, crate::Error> {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    let parser = Parser::new_ext(markdown, options);
    let mut st = State::new();
    let mut chart_block = false;

    for event in parser {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                let layout = if level == HeadingLevel::H1 {
                    Layout::Title
                } else {
                    Layout::Section
                };
                st.begin_slide(layout);
                st.in_heading = true;
                st.text.clear();
            }
            Event::End(TagEnd::Heading(_)) => {
                let title = st.take_text();
                st.cur.title = title;
                st.in_heading = false;
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                chart_block = matches!(kind, pulldown_cmark::CodeBlockKind::Fenced(info) if info.trim() == "chart");
                st.in_code = true;
                st.text.clear();
            }
            Event::End(TagEnd::CodeBlock) => {
                let code = std::mem::take(&mut st.text);
                let block = if chart_block {
                    let chart: crate::ChartInput =
                        serde_json::from_str(&code).map_err(|source| {
                            crate::Error::RequestJson {
                                source,
                                field: Some("/markdown".into()),
                                allowed: Vec::new(),
                            }
                        })?;
                    Block::Chart(crate::chart_from(&chart)?)
                } else {
                    Block::Code(code)
                };
                st.slide().blocks.push(block);
                st.in_code = false;
            }
            Event::Start(Tag::Item) => {
                if !st.text.trim().is_empty() {
                    let text = st.take_text();
                    st.push_bullet(text);
                }
                st.in_item += 1;
                st.text.clear();
            }
            Event::End(TagEnd::Item) => {
                let text = st.take_text();
                st.push_bullet(text);
                st.in_item = st.in_item.saturating_sub(1);
            }
            Event::Start(Tag::BlockQuote(_)) => {
                st.in_quote += 1;
                st.text.clear();
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                st.in_quote = st.in_quote.saturating_sub(1);
                st.flush();
            }
            Event::Start(Tag::Paragraph) => st.text.clear(),
            Event::End(TagEnd::Paragraph) => {
                if !st.in_table {
                    st.flush();
                }
            }
            Event::Start(Tag::Image { dest_url, .. }) => {
                st.image = Some(decode_image_path(&dest_url));
                st.text.clear();
            }
            Event::End(TagEnd::Image) => {
                if let Some(path) = st.image.take() {
                    st.slide().blocks.push(Block::Image(path));
                }
            }
            Event::Start(Tag::Table(_)) => {
                st.in_table = true;
                st.table = Table::default();
            }
            Event::Start(Tag::TableHead) | Event::Start(Tag::TableRow) => st.row.clear(),
            Event::Start(Tag::TableCell) => st.text.clear(),
            Event::End(TagEnd::TableCell) => {
                let cell = st.take_text();
                st.row.push(cell);
            }
            Event::End(TagEnd::TableHead) => {
                st.table.headers = std::mem::take(&mut st.row);
            }
            Event::End(TagEnd::TableRow) => {
                if !st.row.is_empty() {
                    st.table.rows.push(std::mem::take(&mut st.row));
                }
            }
            Event::End(TagEnd::Table) => {
                st.in_table = false;
                let table = std::mem::take(&mut st.table);
                st.slide().blocks.push(Block::Table(table));
            }
            Event::Rule => st.rule(),
            Event::Html(html) | Event::InlineHtml(html) => {
                if matches!(html.trim(), "<br>" | "<br/>" | "<br />") {
                    st.text.push('\n');
                } else if let Some(layout) = parse_layout_comment(&html) {
                    st.set_layout_comment(layout);
                } else if let Some(inner) = html
                    .trim()
                    .strip_prefix("<!--")
                    .and_then(|s| s.strip_suffix("-->"))
                {
                    if let Some(source) = inner.trim().strip_prefix("source:") {
                        st.slide().sources.push(source.trim().to_string());
                    } else if let Some(name) = inner.trim().strip_prefix("layout:") {
                        st.slide().layout_name = Some(name.trim().to_string());
                    }
                }
            }
            // CommonMark events already contain every inline space, even across styles.
            Event::Text(text) | Event::Code(text) => st.text.push_str(&text),
            Event::SoftBreak => st.text.push(if st.in_heading { '\n' } else { ' ' }),
            Event::HardBreak => st.text.push('\n'),
            _ => {}
        }
    }

    Ok(st.into_deck())
}

/// Render a deck back to Markdown (`convert` PPTX -> MD).
pub fn render(deck: &Deck) -> String {
    let mut out = String::new();
    for slide in &deck.slides {
        let prefix = if slide.layout == Layout::Title {
            "#"
        } else {
            "##"
        };
        if slide.title.contains('\n') && !slide.title.contains("\n\n") {
            // Setext headings retain ordinary title line breaks in a single heading.
            out.push_str(&escape_text(&slide.title));
            out.push_str(if prefix == "#" { "\n===\n" } else { "\n---\n" });
        } else {
            out.push_str(prefix);
            out.push(' ');
            out.push_str(&one_line(&slide.title));
            out.push('\n');
        }
        if matches!(
            slide.layout,
            Layout::Quote | Layout::TwoContent | Layout::Chart
        ) {
            out.push_str("<!-- layout: ");
            out.push_str(slide.layout.as_str());
            out.push_str(" -->\n");
        }
        if !slide.subtitle.is_empty() {
            out.push_str(&hard_lines(&slide.subtitle, ""));
            out.push('\n');
        }
        for block in &slide.blocks {
            match block {
                Block::Bullets(items) => {
                    for item in items {
                        let indent = item.len().saturating_sub(item.trim_start().len());
                        out.push_str(&" ".repeat(indent));
                        out.push_str("- ");
                        out.push_str(&hard_lines(item.trim_start(), &" ".repeat(indent + 2)));
                        out.push('\n');
                    }
                }
                Block::Text(text) => {
                    out.push_str(&hard_lines(text, ""));
                    out.push('\n');
                }
                Block::Code(code) => {
                    let fence = "`".repeat(
                        code.split(|c| c != '`')
                            .map(str::len)
                            .max()
                            .unwrap_or(0)
                            .max(3)
                            + 1,
                    );
                    out.push_str(&fence);
                    out.push('\n');
                    out.push_str(code);
                    if !code.ends_with('\n') {
                        out.push('\n');
                    }
                    out.push_str(&fence);
                    out.push('\n');
                }
                Block::Image(path) => {
                    out.push_str("![](<");
                    out.push_str(&encode_image_path(path));
                    out.push_str(">)\n");
                }
                Block::Table(table) => out.push_str(&render_table(table)),
                Block::Chart(chart) => out.push_str(&render_chart(chart)),
            }
            out.push('\n');
        }
        for source in &slide.sources {
            out.push_str("<!-- source: ");
            out.push_str(&source.replace("-->", ""));
            out.push_str(" -->\n");
        }
        if !slide.notes.is_empty() {
            for line in slide.notes.lines() {
                out.push_str("> ");
                out.push_str(&escape_text(line));
                out.push('\n');
            }
        }
        out.push('\n');
    }
    out
}

// Angle-bracket destinations support spaces and Unicode directly. Escape delimiters,
// percent signs and line breaks without dropping bytes; the importer decodes these URLs.
fn encode_image_path(path: &str) -> String {
    let mut out = String::new();
    for c in path.chars() {
        if matches!(c, '<' | '>' | '%' | '\\' | '\n' | '\r') {
            for byte in c.to_string().bytes() {
                out.push_str(&format!("%{byte:02X}"));
            }
        } else {
            out.push(c);
        }
    }
    out
}
fn decode_image_path(path: &str) -> String {
    let mut out = Vec::new();
    let bytes = path.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let digit = |b: u8| char::from(b).to_digit(16).map(|d| d as u8);
            if let (Some(a), Some(b)) = (digit(bytes[i + 1]), digit(bytes[i + 2])) {
                out.push(a * 16 + b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| path.into())
}

/// A chart has no Markdown form; it renders as its data table.
fn render_chart(chart: &Chart) -> String {
    let mut headers = vec![chart.kind.as_str().to_string()];
    headers.extend(chart.labels.iter().cloned());
    let rows = chart
        .series
        .iter()
        .map(|series| {
            let mut row = vec![series.name.clone()];
            for value in &series.values {
                if *value == value.trunc() && value.abs() < 1e15 {
                    row.push(format!("{}", *value as i64));
                } else {
                    row.push(format!("{value}"));
                }
            }
            row
        })
        .collect();
    render_table(&Table {
        headers,
        rows,
        ..Table::default()
    })
}

// Escape active inline syntax and block openers; leave inert punctuation readable.
// Intraword underscores and a single non-leading asterisk cannot open emphasis.
fn escape_text(text: &str) -> String {
    let chars: Vec<_> = text.chars().collect();
    let stars = chars.iter().filter(|c| **c == '*').count();
    let mut out = String::new();
    let mut line_start = 0;
    let mut first = true;
    let mut digits_only = true;
    for (i, &c) in chars.iter().enumerate() {
        let prev = i.checked_sub(1).and_then(|i| chars.get(i));
        let next = chars.get(i + 1);
        let ordered = matches!(c, '.' | ')')
            && i > line_start
            && digits_only
            && next.is_some_and(|c| c.is_whitespace());
        let escape = match c {
            '\\' | '`' | '[' | ']' | '<' | '>' | '#' => true,
            '*' => stars > 1 || first,
            '_' => {
                !(prev.is_some_and(|c| c.is_alphanumeric())
                    && next.is_some_and(|c| c.is_alphanumeric()))
            }
            '&' => chars[i + 1..]
                .iter()
                .take(33) // CommonMark character references are bounded, unlike arbitrary text.
                .take_while(|c| !c.is_whitespace())
                .any(|c| *c == ';'),
            '-' | '+' | '=' | '~' => first,
            _ => ordered,
        };
        if escape {
            out.push('\\');
        }
        out.push(c);
        if c == '\n' {
            line_start = i + 1;
            first = true;
            digits_only = true;
        } else {
            first &= c.is_whitespace();
            digits_only &= c.is_ascii_digit();
        }
    }
    out
}

fn hard_lines(text: &str, indent: &str) -> String {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    if text.contains("\n\n") {
        return one_line(&text);
    }
    escape_text(&text).replace('\n', &format!("  \n{indent}"))
}

fn one_line(text: &str) -> String {
    escape_text(&text.replace("\r\n", "\n").replace('\r', "\n"))
        .replace('|', "\\|")
        .replace('\n', "<br>")
}

fn render_table(table: &Table) -> String {
    let mut out = String::new();
    let headers: Vec<String> = if table.headers.is_empty() {
        match table.rows.first() {
            Some(first) => first.clone(),
            None => return out,
        }
    } else {
        table.headers.clone()
    };
    out.push('|');
    for h in &headers {
        out.push(' ');
        out.push_str(&one_line(h));
        out.push_str(" |");
    }
    out.push('\n');
    out.push('|');
    for _ in &headers {
        out.push_str(" --- |");
    }
    out.push('\n');
    let rows: &[Vec<String>] = if table.headers.is_empty() {
        table.rows.get(1..).unwrap_or(&[])
    } else {
        &table.rows
    };
    for row in rows {
        out.push('|');
        for i in 0..headers.len() {
            let cell = row.get(i).map(String::as_str).unwrap_or("");
            out.push(' ');
            out.push_str(&one_line(cell));
            out.push_str(" |");
        }
        out.push('\n');
    }
    out
}
