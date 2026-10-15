//! The presentation model shared by Markdown, JSON, PPTX and PDF.

/// Supported slide layouts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    Title,
    TitleContent,
    TwoContent,
    Section,
    Image,
    Table,
    Quote,
    Chart,
}

impl Layout {
    /// Parse a layout name from the JSON API (canonical `snake_case` names).
    pub fn parse(name: &str) -> Option<Layout> {
        match name {
            "title" => Some(Layout::Title),
            "title_content" => Some(Layout::TitleContent),
            "two_content" => Some(Layout::TwoContent),
            "section" => Some(Layout::Section),
            "image" => Some(Layout::Image),
            "table" => Some(Layout::Table),
            "quote" => Some(Layout::Quote),
            "chart" => Some(Layout::Chart),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Layout::Title => "title",
            Layout::TitleContent => "title_content",
            Layout::TwoContent => "two_content",
            Layout::Section => "section",
            Layout::Image => "image",
            Layout::Table => "table",
            Layout::Quote => "quote",
            Layout::Chart => "chart",
        }
    }

    /// The `type` attribute of the matching `p:sldLayout`.
    pub fn ooxml_type(self) -> &'static str {
        match self {
            Layout::Title => "title",
            Layout::TitleContent => "obj",
            Layout::TwoContent => "twoObj",
            Layout::Section => "secHead",
            Layout::Image => "obj",
            Layout::Table => "obj",
            Layout::Quote => "obj",
            Layout::Chart => "obj",
        }
    }
}

/// A simple table: an optional header row plus body rows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Table {
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
    /// Logical identity shared by all paginated fragments, scoped to one package.
    pub logical_id: Option<String>,
    /// First body row in the logical table (headers are repeated, not counted).
    pub row_start: usize,
}

/// The kind of a chart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChartKind {
    Bar,
    Column,
    Line,
    Pie,
    Area,
    Scatter,
}

impl ChartKind {
    pub fn parse(name: &str) -> Option<ChartKind> {
        match name {
            "bar" => Some(ChartKind::Bar),
            "column" => Some(ChartKind::Column),
            "area" => Some(ChartKind::Area),
            "scatter" => Some(ChartKind::Scatter),
            "line" => Some(ChartKind::Line),
            "pie" => Some(ChartKind::Pie),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ChartKind::Bar => "bar",
            ChartKind::Column => "column",
            ChartKind::Area => "area",
            ChartKind::Scatter => "scatter",
            ChartKind::Line => "line",
            ChartKind::Pie => "pie",
        }
    }
}

/// One named data series of a chart.
#[derive(Clone, Debug, PartialEq)]
pub struct Series {
    pub name: String,
    pub values: Vec<f64>,
}

/// A chart: category labels plus one or more series of the same length.
#[derive(Clone, Debug, PartialEq)]
pub struct Chart {
    pub kind: ChartKind,
    pub labels: Vec<String>,
    pub series: Vec<Series>,
}

/// One content block of a slide.
#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Bullets(Vec<String>),
    Text(String),
    Code(String),
    Image(String),
    Table(Table),
    Chart(Chart),
}

#[derive(Clone, Debug)]
pub struct Slide {
    pub layout: Layout,
    pub title: String,
    /// Subtitle of a title slide; attribution line of a quote slide lives in `title`
    /// when the quote itself is a text block.
    pub subtitle: String,
    pub blocks: Vec<Block>,
    pub notes: String,
    /// Give short bullet-only slides a themed panel and accent rail.
    pub short_slide_accent: bool,
    pub sources: Vec<String>,
    /// Optional company layout name, used only with a template.
    pub layout_name: Option<String>,
}

impl Default for Slide {
    fn default() -> Self {
        Slide {
            layout: Layout::TitleContent,
            title: String::new(),
            subtitle: String::new(),
            blocks: Vec::new(),
            notes: String::new(),
            short_slide_accent: true,
            sources: Vec::new(),
            layout_name: None,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Deck {
    pub slides: Vec<Slide>,
    /// Image bytes carried by a deck read from a PPTX, keyed by the image path stored in a
    /// [`Block::Image`]. Decks created from Markdown or JSON leave this empty and the path is a
    /// file relative to the base directory.
    pub assets: std::collections::BTreeMap<String, Vec<u8>>,
}

impl Deck {
    /// Infer the layout for a slide from its content when none was given.
    pub fn infer_layout(slide: &Slide) -> Layout {
        if !slide.title.is_empty() && slide.blocks.is_empty() && slide.notes.is_empty() {
            return Layout::Title;
        }
        match slide.blocks.as_slice() {
            [] => Layout::Section,
            [Block::Image(_)] => Layout::Image,
            [Block::Table(_)] => Layout::Table,
            [Block::Chart(_)] => Layout::Chart,
            _ => Layout::TitleContent,
        }
    }
}

/// A colour and font palette written into `ppt/theme`.
#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub name: &'static str,
    pub dk1: &'static str,
    pub lt1: &'static str,
    pub dk2: &'static str,
    pub lt2: &'static str,
    pub accent: [&'static str; 6],
    pub hlink: &'static str,
    pub fol_hlink: &'static str,
    pub major_font: &'static str,
    pub minor_font: &'static str,
    /// Readable text colour on top of `accent[0]` (title and section slides).
    pub on_accent: &'static str,
    /// Muted text colour on top of `accent[0]` (the subtitle of a title slide).
    pub on_accent_muted: &'static str,
}

const LIGHT: Theme = Theme {
    name: "Light",
    dk1: "1F1F1F",
    lt1: "FFFFFF",
    dk2: "44546A",
    lt2: "E7E6E6",
    accent: ["2F5597", "ED7D31", "A5A5A5", "FFC000", "5B9BD5", "70AD47"],
    hlink: "0563C1",
    fol_hlink: "954F72",
    major_font: "Calibri Light",
    minor_font: "Calibri",
    on_accent: "FFFFFF",
    on_accent_muted: "D6E0F0",
};

const DARK: Theme = Theme {
    name: "Dark",
    dk1: "F2F2F2",
    lt1: "141414",
    dk2: "C9C9C9",
    lt2: "2B2B2B",
    accent: ["4F9BFF", "FF9F45", "C0C0C0", "FFD34D", "79C1FF", "8FD98F"],
    hlink: "6FB7FF",
    fol_hlink: "E39BD0",
    major_font: "Calibri Light",
    minor_font: "Calibri",
    on_accent: "FFFFFF",
    on_accent_muted: "D9E9FF",
};

const CORPORATE: Theme = Theme {
    name: "Corporate",
    dk1: "1F3864",
    lt1: "FFFFFF",
    dk2: "2E5A88",
    lt2: "D9E2F3",
    accent: ["1F3864", "2E74B5", "548235", "BF8F00", "7030A0", "C00000"],
    hlink: "0563C1",
    fol_hlink: "7030A0",
    major_font: "Arial",
    minor_font: "Arial",
    on_accent: "FFFFFF",
    on_accent_muted: "B9CCE8",
};

const CREATIVE: Theme = Theme {
    name: "Creative",
    dk1: "302443",
    lt1: "FFF9F2",
    dk2: "695778",
    lt2: "F0E5F4",
    accent: ["7040A0", "D35458", "207F80", "BA7A13", "456AB0", "A33F79"],
    hlink: "456AB0",
    fol_hlink: "A33F79",
    major_font: "DejaVu Sans",
    minor_font: "DejaVu Sans",
    on_accent: "FFFFFF",
    on_accent_muted: "ECE1F6",
};

/// Look up a theme by name; unknown names fall back to `light`.
pub fn theme(name: &str) -> Theme {
    match name {
        "dark" => DARK,
        "corporate" => CORPORATE,
        "creative" => CREATIVE,
        _ => LIGHT,
    }
}
