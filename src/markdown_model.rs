use std::collections::HashMap;
use std::ops::Range;

use gpui_kit::HighlightStyle;
use pulldown_cmark::{Alignment, BlockQuoteKind, CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
    pub kbd: bool,
    pub sub: bool,
    pub sup: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub range: Range<usize>,
    pub style: Style,
    pub link: Option<usize>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rich {
    pub text: String,
    pub runs: Vec<Run>,
    pub links: Vec<String>,
}

impl Rich {
    pub fn split_lines(&self) -> Vec<Rich> {
        if !self.text.contains('\n') {
            return vec![self.clone()];
        }
        let mut out = Vec::new();
        let mut start = 0;
        for (ix, _) in self.text.match_indices('\n').chain(std::iter::once((self.text.len(), ""))) {
            out.push(self.slice(start..ix));
            start = ix + 1;
        }
        out
    }

    fn slice(&self, range: Range<usize>) -> Rich {
        let runs = self
            .runs
            .iter()
            .filter_map(|run| {
                let start = run.range.start.max(range.start);
                let end = run.range.end.min(range.end);
                (start < end).then(|| Run { range: start - range.start..end - range.start, style: run.style, link: run.link })
            })
            .collect();
        Rich { text: self.text[range].to_string(), runs, links: self.links.clone() }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ImageRef {
    pub src: String,
    pub alt: String,
    pub width: Option<f32>,
    pub height: Option<f32>,
    pub link: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Segment {
    Text(Rich),
    Image(ImageRef),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Inline {
    pub segments: Vec<Segment>,
}

impl Inline {
    pub fn plain(&self) -> String {
        let mut out = String::new();
        for segment in &self.segments {
            match segment {
                Segment::Text(rich) => out.push_str(&rich.text),
                Segment::Image(image) => out.push_str(&image.alt),
            }
        }
        out
    }

    pub fn is_blank(&self) -> bool {
        self.segments.iter().all(|segment| match segment {
            Segment::Text(rich) => rich.text.trim().is_empty(),
            Segment::Image(_) => false,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Default,
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlertKind {
    Note,
    Tip,
    Important,
    Warning,
    Caution,
}

impl AlertKind {
    pub fn title(self) -> &'static str {
        match self {
            AlertKind::Note => "Note",
            AlertKind::Tip => "Tip",
            AlertKind::Important => "Important",
            AlertKind::Warning => "Warning",
            AlertKind::Caution => "Caution",
        }
    }
}

#[derive(Clone, Debug)]
pub struct CodeBlock {
    pub lang: Option<String>,
    pub code: String,
    pub highlights: Vec<(Range<usize>, HighlightStyle)>,
}

#[derive(Clone, Debug)]
pub struct ListItem {
    pub task: Option<bool>,
    pub blocks: Vec<Block>,
}

#[derive(Clone, Debug)]
pub struct Table {
    pub aligns: Vec<Align>,
    pub head: Vec<Inline>,
    pub rows: Vec<Vec<Inline>>,
    pub widths: Vec<usize>,
}

#[derive(Clone, Debug)]
pub struct Footnote {
    pub number: usize,
    pub label: String,
    pub blocks: Vec<Block>,
}

#[derive(Clone, Debug)]
pub enum Block {
    Heading { level: u8, content: Inline, slug: String },
    Paragraph { content: Inline, tight: bool },
    Code(CodeBlock),
    Quote(Vec<Block>),
    Alert { kind: AlertKind, blocks: Vec<Block> },
    List { ordered: bool, start: u64, items: Vec<ListItem>, loose: bool },
    Table(Table),
    Rule,
    Container { align: Align, spaced: bool, blocks: Vec<Block> },
    Details { id: usize, open: bool, summary: Inline, blocks: Vec<Block> },
    Footnotes(Vec<Footnote>),
}

#[derive(Clone, Debug, Default)]
pub struct Document {
    pub blocks: Vec<Block>,
    pub lines: Vec<usize>,
    pub anchors: HashMap<String, usize>,
}

pub fn options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_HEADING_ATTRIBUTES
        | Options::ENABLE_GFM
        | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS
}

impl Document {
    pub fn parse(text: &str) -> Document {
        let mut builder = Builder::default();
        for (event, range) in Parser::new_ext(text, options()).into_offset_iter() {
            builder.event(event, range);
        }
        builder.finish(text)
    }

    pub fn anchor(&self, fragment: &str) -> Option<usize> {
        let fragment = fragment.trim_start_matches('#');
        let fragment = fragment.strip_prefix("user-content-").unwrap_or(fragment);
        let decoded = percent_decode(fragment);
        self.anchors.get(&decoded).or_else(|| self.anchors.get(&decoded.to_lowercase())).copied()
    }

    pub fn block_for_line(&self, line: usize) -> Option<usize> {
        if self.blocks.is_empty() {
            return None;
        }
        Some(self.lines.partition_point(|&start| start <= line).saturating_sub(1))
    }

    pub fn code_blocks_mut(&mut self) -> Vec<&mut CodeBlock> {
        let mut out = Vec::new();
        collect_code(&mut self.blocks, &mut out);
        out
    }
}

fn collect_code<'a>(blocks: &'a mut [Block], out: &mut Vec<&'a mut CodeBlock>) {
    for block in blocks {
        match block {
            Block::Code(code) => out.push(code),
            Block::Quote(children)
            | Block::Alert { blocks: children, .. }
            | Block::Container { blocks: children, .. }
            | Block::Details { blocks: children, .. } => collect_code(children, out),
            Block::List { items, .. } => {
                for item in items {
                    collect_code(&mut item.blocks, out);
                }
            }
            Block::Footnotes(notes) => {
                for note in notes {
                    collect_code(&mut note.blocks, out);
                }
            }
            _ => {}
        }
    }
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Some(value) = std::str::from_utf8(&bytes[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(value);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn slug_base(text: &str) -> String {
    text.chars()
        .flat_map(char::to_lowercase)
        .filter_map(|c| match c {
            ' ' => Some('-'),
            '-' | '_' => Some(c),
            c if c.is_alphanumeric() => Some(c),
            _ => None,
        })
        .collect()
}

#[derive(Default)]
pub struct Slugger {
    occurrences: HashMap<String, usize>,
}

impl Slugger {
    pub fn slug(&mut self, text: &str) -> String {
        let original = slug_base(text);
        let mut result = original.clone();
        while self.occurrences.contains_key(&result) {
            let count = self.occurrences.entry(original.clone()).or_insert(0);
            *count += 1;
            result = format!("{original}-{count}");
        }
        self.occurrences.insert(result.clone(), 0);
        result
    }
}

#[derive(Default)]
struct Counters {
    bold: u32,
    italic: u32,
    strike: u32,
    code: u32,
    kbd: u32,
    sub: u32,
    sup: u32,
}

impl Counters {
    fn style(&self) -> Style {
        Style {
            bold: self.bold > 0,
            italic: self.italic > 0,
            strike: self.strike > 0,
            code: self.code > 0,
            kbd: self.kbd > 0,
            sub: self.sub > 0,
            sup: self.sup > 0,
        }
    }

    fn slot(&mut self, name: &str) -> Option<&mut u32> {
        Some(match name {
            "b" | "strong" => &mut self.bold,
            "i" | "em" | "cite" | "var" => &mut self.italic,
            "s" | "del" | "strike" => &mut self.strike,
            "code" | "tt" | "samp" => &mut self.code,
            "kbd" => &mut self.kbd,
            "sub" => &mut self.sub,
            "sup" => &mut self.sup,
            _ => return None,
        })
    }
}

#[derive(Default)]
struct InlineBuilder {
    segments: Vec<Segment>,
    current: Rich,
    current_ids: Vec<u64>,
    counters: Counters,
    links: Vec<(String, u64)>,
    image: Option<ImageRef>,
}

impl InlineBuilder {
    fn ends_with_space(&self) -> bool {
        match self.current.text.chars().last() {
            Some(c) => c.is_whitespace(),
            None => self.segments.is_empty(),
        }
    }

    fn push_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        if let Some(image) = self.image.as_mut() {
            image.alt.push_str(text);
            return;
        }
        let style = self.counters.style();
        let link = match self.links.last() {
            Some((url, id)) => Some(match self.current_ids.iter().position(|known| known == id) {
                Some(ix) => ix,
                None => {
                    self.current.links.push(url.clone());
                    self.current_ids.push(*id);
                    self.current.links.len() - 1
                }
            }),
            None => None,
        };
        let start = self.current.text.len();
        self.current.text.push_str(text);
        let end = self.current.text.len();
        if let Some(last) = self.current.runs.last_mut()
            && last.range.end == start
            && last.style == style
            && last.link == link
        {
            last.range.end = end;
        } else {
            self.current.runs.push(Run { range: start..end, style, link });
        }
    }

    fn push_image(&mut self, mut image: ImageRef) {
        if image.link.is_none() {
            image.link = self.links.last().map(|(url, _)| url.clone());
        }
        self.flush_text();
        self.segments.push(Segment::Image(image));
    }

    fn flush_text(&mut self) {
        if !self.current.text.is_empty() {
            self.segments.push(Segment::Text(std::mem::take(&mut self.current)));
            self.current_ids.clear();
        }
    }

    fn finish(mut self) -> Inline {
        if let Some(image) = self.image.take() {
            self.push_image(image);
        }
        self.flush_text();
        Inline { segments: self.segments }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    Paragraph { tight: bool, html: bool },
    Heading { level: u8 },
    Cell,
    Summary,
}

enum Frame {
    Root,
    Quote(Option<BlockQuoteKind>),
    List { ordered: bool, start: u64, items: Vec<ListItem> },
    Item { task: Option<bool> },
    Footnote(String),
    Html { tag: String, align: Align },
    Details { id: usize, open: bool, summary: Inline },
}

struct Level {
    frame: Frame,
    blocks: Vec<Block>,
}

struct TableState {
    aligns: Vec<Align>,
    head: Vec<Inline>,
    rows: Vec<Vec<Inline>>,
    row: Vec<Inline>,
}

struct Builder {
    levels: Vec<Level>,
    inline: Option<(Target, Option<String>, InlineBuilder)>,
    table: Option<TableState>,
    code: Option<(Option<String>, String)>,
    html: Option<String>,
    metadata: Option<String>,
    slugger: Slugger,
    anchors: HashMap<String, usize>,
    starts: Vec<usize>,
    mark: Option<usize>,
    offset: usize,
    footnote_numbers: HashMap<String, usize>,
    footnote_order: Vec<String>,
    footnote_defs: HashMap<String, Vec<Block>>,
    details: usize,
    link_ids: u64,
    html_links: Vec<bool>,
}

impl Default for Builder {
    fn default() -> Self {
        Builder {
            levels: vec![Level { frame: Frame::Root, blocks: Vec::new() }],
            inline: None,
            table: None,
            code: None,
            html: None,
            metadata: None,
            slugger: Slugger::default(),
            anchors: HashMap::new(),
            starts: Vec::new(),
            mark: None,
            offset: 0,
            footnote_numbers: HashMap::new(),
            footnote_order: Vec::new(),
            footnote_defs: HashMap::new(),
            details: 0,
            link_ids: 0,
            html_links: Vec::new(),
        }
    }
}

fn alignment(align: Alignment) -> Align {
    match align {
        Alignment::None => Align::Default,
        Alignment::Left => Align::Left,
        Alignment::Center => Align::Center,
        Alignment::Right => Align::Right,
    }
}

fn parse_align(value: Option<&str>) -> Align {
    match value.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
        Some("center") | Some("middle") => Align::Center,
        Some("right") => Align::Right,
        Some("left") => Align::Left,
        _ => Align::Default,
    }
}

fn parse_length(value: Option<&str>) -> Option<f32> {
    let value = value?.trim();
    if value.ends_with('%') {
        return None;
    }
    let value = value.trim_end_matches("px");
    value.parse::<f32>().ok().filter(|v| v.is_finite() && *v > 0.)
}

impl Builder {
    fn event(&mut self, event: Event, range: Range<usize>) {
        self.offset = range.start;
        if self.levels.len() == 1
            && self.inline.is_none()
            && self.table.is_none()
            && self.code.is_none()
            && self.html.is_none()
            && self.metadata.is_none()
            && self.mark.is_none()
        {
            self.mark = Some(range.start);
        }
        if let Some(buffer) = self.metadata.as_mut() {
            match event {
                Event::Text(text) => buffer.push_str(&text),
                Event::End(TagEnd::MetadataBlock(_)) => {
                    let yaml = self.metadata.take().unwrap_or_default();
                    if let Some(table) = front_matter(&yaml) {
                        self.push_block(Block::Table(table));
                    }
                }
                _ => {}
            }
            return;
        }
        if let Some((_, buffer)) = self.code.as_mut() {
            match event {
                Event::Text(text) => buffer.push_str(&text),
                Event::End(TagEnd::CodeBlock) => {
                    let (lang, mut code) = self.code.take().unwrap_or_default();
                    if code.ends_with('\n') {
                        code.pop();
                    }
                    self.push_block(Block::Code(CodeBlock { lang, code, highlights: Vec::new() }));
                }
                _ => {}
            }
            return;
        }
        if let Some(buffer) = self.html.as_mut() {
            match event {
                Event::Html(text) | Event::Text(text) | Event::InlineHtml(text) => buffer.push_str(&text),
                Event::End(TagEnd::HtmlBlock) => {
                    let html = self.html.take().unwrap_or_default();
                    for token in tokenize(&html) {
                        self.html_token(token, true);
                    }
                    if matches!(self.inline, Some((Target::Paragraph { html: true, .. }, _, _))) {
                        self.flush_inline();
                    }
                }
                _ => {}
            }
            return;
        }
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) | Event::InlineMath(text) | Event::DisplayMath(text) => {
                self.inline_mut().push_text(&text);
            }
            Event::Code(text) => {
                let inline = self.inline_mut();
                inline.counters.code += 1;
                inline.push_text(&text);
                inline.counters.code -= 1;
            }
            Event::Html(text) => {
                for token in tokenize(&text) {
                    self.html_token(token, true);
                }
            }
            Event::InlineHtml(text) => {
                for token in tokenize(&text) {
                    self.html_token(token, false);
                }
            }
            Event::FootnoteReference(label) => {
                let label = label.to_string();
                let number = match self.footnote_numbers.get(&label) {
                    Some(number) => *number,
                    None => {
                        let number = self.footnote_order.len() + 1;
                        self.footnote_order.push(label.clone());
                        self.footnote_numbers.insert(label.clone(), number);
                        number
                    }
                };
                self.link_ids += 1;
                let id = self.link_ids;
                let inline = self.inline_mut();
                inline.links.push((format!("#footnote-{label}"), id));
                inline.counters.sup += 1;
                inline.push_text(&format!("[{number}]"));
                inline.counters.sup -= 1;
                inline.links.pop();
            }
            Event::SoftBreak => {
                if let Some((_, _, inline)) = self.inline.as_mut() {
                    inline.push_text(" ");
                }
            }
            Event::HardBreak => self.inline_mut().push_text("\n"),
            Event::Rule => {
                self.flush_inline();
                self.push_block(Block::Rule);
            }
            Event::TaskListMarker(checked) => {
                if let Some(Level { frame: Frame::Item { task }, .. }) = self.levels.last_mut() {
                    *task = Some(checked);
                }
            }
        }
    }

    fn inline_mut(&mut self) -> &mut InlineBuilder {
        if self.inline.is_none() {
            self.inline = Some((Target::Paragraph { tight: true, html: false }, None, InlineBuilder::default()));
        }
        &mut self.inline.as_mut().expect("inline was just created").2
    }

    fn html_inline_mut(&mut self) -> &mut InlineBuilder {
        if self.inline.is_none() {
            self.inline = Some((Target::Paragraph { tight: true, html: true }, None, InlineBuilder::default()));
        }
        &mut self.inline.as_mut().expect("inline was just created").2
    }

    fn start_inline(&mut self, target: Target, id: Option<String>) {
        self.flush_inline();
        self.inline = Some((target, id, InlineBuilder::default()));
    }

    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::Paragraph => self.start_inline(Target::Paragraph { tight: false, html: false }, None),
            Tag::Heading { level, id, .. } => {
                self.start_inline(Target::Heading { level: level as u8 }, id.map(|id| id.to_string()))
            }
            Tag::BlockQuote(kind) => self.push_level(Frame::Quote(kind)),
            Tag::CodeBlock(kind) => {
                self.flush_inline();
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => info
                        .split(|c: char| c.is_whitespace() || c == ',' || c == '{')
                        .next()
                        .filter(|lang| !lang.is_empty())
                        .map(str::to_string),
                    CodeBlockKind::Indented => None,
                };
                self.code = Some((lang, String::new()));
            }
            Tag::HtmlBlock => self.html = Some(String::new()),
            Tag::List(start) => {
                self.push_level(Frame::List { ordered: start.is_some(), start: start.unwrap_or(1), items: Vec::new() })
            }
            Tag::Item => self.push_level(Frame::Item { task: None }),
            Tag::FootnoteDefinition(label) => self.push_level(Frame::Footnote(label.to_string())),
            Tag::Table(aligns) => {
                self.flush_inline();
                self.table = Some(TableState {
                    aligns: aligns.into_iter().map(alignment).collect(),
                    head: Vec::new(),
                    rows: Vec::new(),
                    row: Vec::new(),
                });
            }
            Tag::TableHead | Tag::TableRow => {
                if let Some(table) = self.table.as_mut() {
                    table.row.clear();
                }
            }
            Tag::TableCell => self.start_inline(Target::Cell, None),
            Tag::Emphasis => self.inline_mut().counters.italic += 1,
            Tag::Strong => self.inline_mut().counters.bold += 1,
            Tag::Strikethrough => self.inline_mut().counters.strike += 1,
            Tag::Superscript => self.inline_mut().counters.sup += 1,
            Tag::Subscript => self.inline_mut().counters.sub += 1,
            Tag::Link { dest_url, .. } => {
                self.link_ids += 1;
                let id = self.link_ids;
                self.inline_mut().links.push((dest_url.to_string(), id));
            }
            Tag::Image { dest_url, .. } => {
                self.inline_mut().image = Some(ImageRef { src: dest_url.to_string(), ..ImageRef::default() });
            }
            Tag::MetadataBlock(_) => {
                self.flush_inline();
                self.metadata = Some(String::new());
            }
            Tag::DefinitionList | Tag::DefinitionListTitle | Tag::DefinitionListDefinition => self.flush_inline(),
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::TableCell => self.flush_inline(),
            TagEnd::BlockQuote(_) => self.close_until(|frame| matches!(frame, Frame::Quote(_))),
            TagEnd::List(_) => self.close_until(|frame| matches!(frame, Frame::List { .. })),
            TagEnd::Item => self.close_until(|frame| matches!(frame, Frame::Item { .. })),
            TagEnd::FootnoteDefinition => self.close_until(|frame| matches!(frame, Frame::Footnote(_))),
            TagEnd::Table => {
                self.flush_inline();
                if let Some(state) = self.table.take() {
                    let table = finish_table(state);
                    self.push_block(Block::Table(table));
                }
            }
            TagEnd::TableHead => {
                if let Some(table) = self.table.as_mut() {
                    table.head = std::mem::take(&mut table.row);
                }
            }
            TagEnd::TableRow => {
                if let Some(table) = self.table.as_mut() {
                    let row = std::mem::take(&mut table.row);
                    table.rows.push(row);
                }
            }
            TagEnd::Emphasis => self.adjust(|c| &mut c.italic),
            TagEnd::Strong => self.adjust(|c| &mut c.bold),
            TagEnd::Strikethrough => self.adjust(|c| &mut c.strike),
            TagEnd::Superscript => self.adjust(|c| &mut c.sup),
            TagEnd::Subscript => self.adjust(|c| &mut c.sub),
            TagEnd::Link => {
                if let Some((_, _, inline)) = self.inline.as_mut() {
                    inline.links.pop();
                }
            }
            TagEnd::Image => {
                if let Some((_, _, inline)) = self.inline.as_mut()
                    && let Some(image) = inline.image.take()
                {
                    inline.push_image(image);
                }
            }
            TagEnd::CodeBlock | TagEnd::HtmlBlock | TagEnd::MetadataBlock(_) => {}
            TagEnd::DefinitionList | TagEnd::DefinitionListTitle | TagEnd::DefinitionListDefinition => {
                self.flush_inline()
            }
        }
    }

    fn adjust(&mut self, slot: impl FnOnce(&mut Counters) -> &mut u32) {
        if let Some((_, _, inline)) = self.inline.as_mut() {
            let value = slot(&mut inline.counters);
            *value = value.saturating_sub(1);
        }
    }

    fn html_token(&mut self, token: HtmlToken, block: bool) {
        match token {
            HtmlToken::Text(text) => {
                let text = collapse_whitespace(&decode_entities(&text));
                if self.inline.is_none() && text.trim().is_empty() {
                    return;
                }
                let inline = self.html_inline_mut();
                let text = if inline.ends_with_space() { text.trim_start() } else { text.as_str() };
                inline.push_text(text);
            }
            HtmlToken::Open { name, attrs, self_closing } => {
                let attr = |key: &str| attrs.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
                match name.as_str() {
                    "br" => self.html_inline_mut().push_text("\n"),
                    "img" => {
                        let image = ImageRef {
                            src: attr("src").unwrap_or_default().to_string(),
                            alt: attr("alt").map(decode_entities).unwrap_or_default(),
                            width: parse_length(attr("width")),
                            height: parse_length(attr("height")),
                            link: None,
                        };
                        if !image.src.is_empty() {
                            self.html_inline_mut().push_image(image);
                        }
                    }
                    "a" => match attr("href") {
                        Some(href) => {
                            self.link_ids += 1;
                            let id = self.link_ids;
                            let href = decode_entities(href);
                            self.html_inline_mut().links.push((href, id));
                            self.html_links.push(true);
                        }
                        None => self.html_links.push(false),
                    },
                    "p" | "div" | "center" | "section" | "article" | "header" | "footer" | "main" | "nav"
                    | "figure" | "blockquote" => {
                        if !block {
                            return;
                        }
                        self.flush_inline();
                        if !self_closing {
                            let align = if name == "center" { Align::Center } else { parse_align(attr("align")) };
                            self.push_level(Frame::Html { tag: name, align });
                        }
                    }
                    "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                        let level = name.as_bytes()[1] - b'0';
                        let align = parse_align(attr("align"));
                        self.flush_inline();
                        if align != Align::Default {
                            self.push_level(Frame::Html { tag: name.clone(), align });
                        }
                        let id = attr("id").map(str::to_string);
                        self.start_inline(Target::Heading { level }, id);
                    }
                    "details" => {
                        self.flush_inline();
                        let id = self.details;
                        self.details += 1;
                        let open = attrs.iter().any(|(k, _)| k == "open");
                        self.push_level(Frame::Details { id, open, summary: Inline::default() });
                    }
                    "summary" => self.start_inline(Target::Summary, None),
                    "hr" => {
                        self.flush_inline();
                        self.push_block(Block::Rule);
                    }
                    "ul" | "ol" | "li" | "table" | "thead" | "tbody" | "tr" | "dl" | "dt" | "dd" | "pre" => {
                        if block {
                            self.flush_inline();
                        }
                    }
                    "td" | "th" => {
                        if let Some((_, _, inline)) = self.inline.as_mut()
                            && !inline.ends_with_space()
                        {
                            inline.push_text(" ");
                        }
                    }
                    other => {
                        if Counters::default().slot(other).is_some()
                            && let Some(slot) = self.html_inline_mut().counters.slot(other)
                        {
                            *slot += 1;
                        }
                    }
                }
            }
            HtmlToken::Close(name) => match name.as_str() {
                "a" => {
                    if self.html_links.pop() == Some(true)
                        && let Some((_, _, inline)) = self.inline.as_mut()
                    {
                        inline.links.pop();
                    }
                }
                "p" | "div" | "center" | "section" | "article" | "header" | "footer" | "main" | "nav" | "figure"
                | "blockquote" => {
                    if block {
                        self.flush_inline();
                        self.close_html(&name);
                    }
                }
                "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                    self.flush_inline();
                    self.close_html(&name);
                }
                "summary" => self.flush_inline(),
                "details" => {
                    self.flush_inline();
                    if self.levels.iter().any(|level| matches!(level.frame, Frame::Details { .. })) {
                        self.close_until(|frame| matches!(frame, Frame::Details { .. }));
                    }
                }
                "ul" | "ol" | "li" | "table" | "thead" | "tbody" | "tr" | "dl" | "dt" | "dd" | "pre" => {
                    if block {
                        self.flush_inline();
                    }
                }
                other => {
                    if let Some((_, _, inline)) = self.inline.as_mut()
                        && let Some(slot) = inline.counters.slot(other)
                    {
                        *slot = slot.saturating_sub(1);
                    }
                }
            },
        }
    }

    fn close_html(&mut self, tag: &str) {
        let found = self
            .levels
            .iter()
            .rev()
            .take_while(|level| matches!(level.frame, Frame::Html { .. }))
            .any(|level| matches!(&level.frame, Frame::Html { tag: t, .. } if t == tag));
        if found {
            self.close_until(|frame| matches!(frame, Frame::Html { tag: t, .. } if t == tag));
        }
    }

    fn push_level(&mut self, frame: Frame) {
        self.flush_inline();
        self.levels.push(Level { frame, blocks: Vec::new() });
    }

    fn close_until(&mut self, done: impl Fn(&Frame) -> bool) {
        self.flush_inline();
        while self.levels.len() > 1 {
            let level = self.levels.pop().expect("more than the root level");
            let matched = done(&level.frame);
            self.close_level(level);
            if matched {
                break;
            }
        }
    }

    fn close_level(&mut self, level: Level) {
        let Level { frame, blocks } = level;
        match frame {
            Frame::Root => {}
            Frame::Quote(None) => self.push_block(Block::Quote(blocks)),
            Frame::Quote(Some(kind)) => {
                let kind = match kind {
                    BlockQuoteKind::Note => AlertKind::Note,
                    BlockQuoteKind::Tip => AlertKind::Tip,
                    BlockQuoteKind::Important => AlertKind::Important,
                    BlockQuoteKind::Warning => AlertKind::Warning,
                    BlockQuoteKind::Caution => AlertKind::Caution,
                };
                self.push_block(Block::Alert { kind, blocks });
            }
            Frame::List { ordered, start, items } => {
                let loose = items.iter().any(|item| {
                    item.blocks.iter().any(|block| matches!(block, Block::Paragraph { tight: false, .. }))
                });
                self.push_block(Block::List { ordered, start, items, loose });
            }
            Frame::Item { task } => {
                if let Some(Level { frame: Frame::List { items, .. }, .. }) = self.levels.last_mut() {
                    items.push(ListItem { task, blocks });
                } else {
                    self.push_block(Block::Container { align: Align::Default, spaced: false, blocks });
                }
            }
            Frame::Footnote(label) => {
                self.footnote_defs.insert(label, blocks);
            }
            Frame::Html { tag, align } => {
                self.push_block(Block::Container { align, spaced: tag == "p", blocks });
            }
            Frame::Details { id, open, summary } => self.push_block(Block::Details { id, open, summary, blocks }),
        }
    }

    fn flush_inline(&mut self) {
        let Some((target, id, builder)) = self.inline.take() else { return };
        let content = builder.finish();
        match target {
            Target::Paragraph { tight, .. } => {
                if !content.is_blank() {
                    self.push_block(Block::Paragraph { content, tight });
                }
            }
            Target::Heading { level } => {
                let slug = match id {
                    Some(id) => {
                        self.slugger.occurrences.insert(id.clone(), 0);
                        id
                    }
                    None => self.slugger.slug(content.plain().trim()),
                };
                self.push_block(Block::Heading { level, content, slug });
            }
            Target::Cell => {
                if let Some(table) = self.table.as_mut() {
                    table.row.push(content);
                }
            }
            Target::Summary => {
                if let Some(Level { frame: Frame::Details { summary, .. }, .. }) = self
                    .levels
                    .iter_mut()
                    .rev()
                    .find(|level| matches!(level.frame, Frame::Details { .. }))
                {
                    *summary = content;
                } else if !content.is_blank() {
                    self.push_block(Block::Paragraph { content, tight: false });
                }
            }
        }
    }

    fn push_block(&mut self, block: Block) {
        if self.levels.len() == 1 {
            let index = self.levels[0].blocks.len();
            register_anchors(&block, index, &mut self.anchors);
            self.starts.push(self.mark.take().unwrap_or(self.offset));
        }
        self.levels.last_mut().expect("root level exists").blocks.push(block);
    }

    fn finish(mut self, text: &str) -> Document {
        self.offset = text.len();
        if let Some((lang, code)) = self.code.take() {
            self.push_block(Block::Code(CodeBlock { lang, code, highlights: Vec::new() }));
        }
        self.flush_inline();
        while self.levels.len() > 1 {
            let level = self.levels.pop().expect("more than the root level");
            self.close_level(level);
        }
        let mut notes = Vec::new();
        for (ix, label) in std::mem::take(&mut self.footnote_order).into_iter().enumerate() {
            if let Some(blocks) = self.footnote_defs.remove(&label) {
                notes.push(Footnote { number: ix + 1, label, blocks });
            }
        }
        if !notes.is_empty() {
            let index = self.levels[0].blocks.len();
            for note in &notes {
                self.anchors.insert(format!("footnote-{}", note.label), index);
                self.anchors.insert(format!("fn-{}", note.label), index);
            }
            self.mark = Some(text.len());
            self.push_block(Block::Footnotes(notes));
        }
        let mut line_starts = vec![0];
        line_starts.extend(text.match_indices('\n').map(|(ix, _)| ix + 1));
        let lines =
            self.starts.iter().map(|&offset| line_starts.partition_point(|&start| start <= offset).saturating_sub(1)).collect();
        let blocks = self.levels.pop().map(|level| level.blocks).unwrap_or_default();
        Document { blocks, lines, anchors: self.anchors }
    }
}

fn register_anchors(block: &Block, index: usize, anchors: &mut HashMap<String, usize>) {
    match block {
        Block::Heading { slug, .. } => {
            anchors.entry(slug.clone()).or_insert(index);
        }
        Block::Quote(children)
        | Block::Alert { blocks: children, .. }
        | Block::Container { blocks: children, .. }
        | Block::Details { blocks: children, .. } => {
            for child in children {
                register_anchors(child, index, anchors);
            }
        }
        Block::List { items, .. } => {
            for item in items {
                for child in &item.blocks {
                    register_anchors(child, index, anchors);
                }
            }
        }
        _ => {}
    }
}

fn finish_table(state: TableState) -> Table {
    let TableState { aligns, head, rows, .. } = state;
    let columns = aligns.len().max(head.len()).max(rows.iter().map(Vec::len).max().unwrap_or(0));
    let mut widths = vec![3usize; columns];
    for row in std::iter::once(&head).chain(rows.iter()) {
        for (ix, cell) in row.iter().enumerate() {
            let len = cell.plain().chars().count();
            if let Some(width) = widths.get_mut(ix) {
                *width = (*width).max(len.min(60));
            }
        }
    }
    let mut aligns = aligns;
    aligns.resize(columns, Align::Default);
    Table { aligns, head, rows, widths }
}

fn plain_inline(text: &str) -> Inline {
    if text.is_empty() {
        return Inline::default();
    }
    Inline {
        segments: vec![Segment::Text(Rich {
            text: text.to_string(),
            runs: vec![Run { range: 0..text.len(), style: Style::default(), link: None }],
            links: Vec::new(),
        })],
    }
}

fn unquote(value: &str) -> &str {
    let value = value.trim();
    for quote in ['"', '\''] {
        if value.len() >= 2 && value.starts_with(quote) && value.ends_with(quote) {
            return &value[1..value.len() - 1];
        }
    }
    value
}

pub fn front_matter(yaml: &str) -> Option<Table> {
    let mut keys: Vec<String> = Vec::new();
    let mut values: Vec<String> = Vec::new();
    for line in yaml.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed == "---" || trimmed == "..." {
            continue;
        }
        let nested = line.starts_with([' ', '\t', '-']);
        if !nested && let Some((key, value)) = line.split_once(':') {
            keys.push(key.trim().to_string());
            values.push(unquote(value).to_string());
            continue;
        }
        if let Some(last) = values.last_mut() {
            let item = unquote(trimmed.trim_start_matches('-').trim());
            if !last.is_empty() {
                last.push_str(", ");
            }
            last.push_str(item);
        }
    }
    if keys.is_empty() {
        return None;
    }
    let head = keys.iter().map(|key| plain_inline(key)).collect();
    let row = values.iter().map(|value| plain_inline(value)).collect();
    Some(finish_table(TableState { aligns: Vec::new(), head, rows: vec![row], row: Vec::new() }))
}

#[derive(Clone, Debug, PartialEq)]
pub enum HtmlToken {
    Open { name: String, attrs: Vec<(String, String)>, self_closing: bool },
    Close(String),
    Text(String),
}

pub fn tokenize(html: &str) -> Vec<HtmlToken> {
    let bytes = html.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0;
    let mut text_start = 0;
    let push_text = |tokens: &mut Vec<HtmlToken>, from: usize, to: usize| {
        if from < to {
            tokens.push(HtmlToken::Text(html[from..to].to_string()));
        }
    };
    while i < bytes.len() {
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }
        let rest = &html[i..];
        if let Some(comment) = rest.strip_prefix("<!--") {
            push_text(&mut tokens, text_start, i);
            i = comment.find("-->").map(|end| i + 4 + end + 3).unwrap_or(html.len());
            text_start = i;
            continue;
        }
        if rest.starts_with("<!") || rest.starts_with("<?") {
            push_text(&mut tokens, text_start, i);
            i = rest.find('>').map(|end| i + end + 1).unwrap_or(html.len());
            text_start = i;
            continue;
        }
        match parse_tag(rest) {
            Some((token, len)) => {
                push_text(&mut tokens, text_start, i);
                tokens.push(token);
                i += len;
                text_start = i;
            }
            None => i += 1,
        }
    }
    push_text(&mut tokens, text_start, html.len());
    tokens
}

fn parse_tag(s: &str) -> Option<(HtmlToken, usize)> {
    let b = s.as_bytes();
    let closing = b.get(1) == Some(&b'/');
    let name_start = if closing { 2 } else { 1 };
    if !b.get(name_start).is_some_and(u8::is_ascii_alphabetic) {
        return None;
    }
    let mut pos = name_start;
    while pos < b.len() && (b[pos].is_ascii_alphanumeric() || b[pos] == b'-') {
        pos += 1;
    }
    let name = s[name_start..pos].to_ascii_lowercase();
    let mut attrs = Vec::new();
    loop {
        while pos < b.len() && b[pos].is_ascii_whitespace() {
            pos += 1;
        }
        let &byte = b.get(pos)?;
        match byte {
            b'>' => {
                let token = if closing { HtmlToken::Close(name) } else { HtmlToken::Open { name, attrs, self_closing: false } };
                return Some((token, pos + 1));
            }
            b'/' if b.get(pos + 1) == Some(&b'>') => {
                let token = if closing { HtmlToken::Close(name) } else { HtmlToken::Open { name, attrs, self_closing: true } };
                return Some((token, pos + 2));
            }
            b'/' => pos += 1,
            _ => {
                let key_start = pos;
                while pos < b.len() && !b[pos].is_ascii_whitespace() && !matches!(b[pos], b'=' | b'>' | b'/') {
                    pos += 1;
                }
                let key = s[key_start..pos].to_ascii_lowercase();
                while pos < b.len() && b[pos].is_ascii_whitespace() {
                    pos += 1;
                }
                let mut value = String::new();
                if b.get(pos) == Some(&b'=') {
                    pos += 1;
                    while pos < b.len() && b[pos].is_ascii_whitespace() {
                        pos += 1;
                    }
                    match b.get(pos) {
                        Some(&quote) if quote == b'"' || quote == b'\'' => {
                            let value_start = pos + 1;
                            let end = s[value_start..].find(quote as char)? + value_start;
                            value = s[value_start..end].to_string();
                            pos = end + 1;
                        }
                        _ => {
                            let value_start = pos;
                            while pos < b.len() && !b[pos].is_ascii_whitespace() && b[pos] != b'>' {
                                pos += 1;
                            }
                            value = s[value_start..pos].to_string();
                        }
                    }
                }
                if !key.is_empty() {
                    attrs.push((key, value));
                }
            }
        }
    }
}

fn collapse_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut space = false;
    for c in text.chars() {
        if c.is_whitespace() && c != '\u{a0}' {
            if !space {
                out.push(' ');
            }
            space = true;
        } else {
            out.push(c);
            space = false;
        }
    }
    out
}

pub fn decode_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let tail = &rest[amp..];
        let decoded = tail.find(';').filter(|&end| end <= 10).and_then(|end| {
            let entity = &tail[1..end];
            let ch = match entity {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some('\u{a0}'),
                "copy" => Some('©'),
                "reg" => Some('®'),
                "trade" => Some('™'),
                "mdash" => Some('—'),
                "ndash" => Some('–'),
                "hellip" => Some('…'),
                "middot" => Some('·'),
                "bull" => Some('•'),
                "rarr" => Some('→'),
                "larr" => Some('←'),
                "times" => Some('×'),
                _ => {
                    if let Some(hex) = entity.strip_prefix("#x").or_else(|| entity.strip_prefix("#X")) {
                        u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)
                    } else if let Some(dec) = entity.strip_prefix('#') {
                        dec.parse::<u32>().ok().and_then(char::from_u32)
                    } else {
                        None
                    }
                }
            };
            ch.map(|ch| (ch, end + 1))
        });
        match decoded {
            Some((ch, len)) => {
                out.push(ch);
                rest = &tail[len..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(block: &Block) -> String {
        match block {
            Block::Heading { content, .. } | Block::Paragraph { content, .. } => content.plain(),
            _ => String::new(),
        }
    }

    #[test]
    fn github_slugs_match_github_slugger() {
        let mut slugger = Slugger::default();
        assert_eq!(slugger.slug("Hello World"), "hello-world");
        assert_eq!(slugger.slug("Hello World"), "hello-world-1");
        assert_eq!(slugger.slug("Hello World"), "hello-world-2");
        assert_eq!(slugger.slug("hello-world-1"), "hello-world-1-1");
        assert_eq!(slugger.slug("What's new in v1.2?"), "whats-new-in-v12");
        assert_eq!(slugger.slug("API: `foo_bar()`"), "api-foo_bar");
        assert_eq!(slugger.slug("Ünïcödé Straße"), "ünïcödé-straße");
        assert_eq!(slugger.slug("  two  spaces"), "--two--spaces");
        assert_eq!(slugger.slug("🚀 Launch"), "-launch");
    }

    #[test]
    fn headings_get_slugs_and_anchors() {
        let doc = Document::parse("# Intro\n\ntext\n\n## Intro\n\n### Custom {#my-id}\n");
        let slugs: Vec<_> = doc
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Heading { slug, level, .. } => Some((*level, slug.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(slugs, vec![(1, "intro".into()), (2, "intro-1".into()), (3, "my-id".into())]);
        assert_eq!(doc.anchor("#intro"), Some(0));
        assert_eq!(doc.anchor("#intro-1"), Some(2));
        assert_eq!(doc.anchor("#user-content-my-id"), Some(3));
        assert_eq!(doc.anchor("#missing"), None);
    }

    #[test]
    fn alerts_are_recognized() {
        let doc = Document::parse("> [!NOTE]\n> Useful info.\n\n> [!CAUTION]\n> Careful.\n\n> plain quote\n");
        assert!(matches!(&doc.blocks[0], Block::Alert { kind: AlertKind::Note, blocks } if text_of(&blocks[0]) == "Useful info."));
        assert!(matches!(&doc.blocks[1], Block::Alert { kind: AlertKind::Caution, .. }));
        assert!(matches!(&doc.blocks[2], Block::Quote(blocks) if text_of(&blocks[0]) == "plain quote"));
    }

    #[test]
    fn tables_keep_alignment_head_and_rows() {
        let doc = Document::parse("| Name | Count | Note |\n|:-----|------:|:----:|\n| a | 1 | x |\n| bb | 22 | yy |\n");
        let Block::Table(table) = &doc.blocks[0] else { panic!("expected table: {:?}", doc.blocks) };
        assert_eq!(table.aligns, vec![Align::Left, Align::Right, Align::Center]);
        assert_eq!(table.head.iter().map(Inline::plain).collect::<Vec<_>>(), vec!["Name", "Count", "Note"]);
        assert_eq!(table.rows.len(), 2);
        assert_eq!(table.rows[1][1].plain(), "22");
        assert_eq!(table.widths, vec![4, 5, 4]);
    }

    #[test]
    fn task_lists_and_nested_lists() {
        let doc = Document::parse("- [x] done\n- [ ] todo\n  - nested\n\n3. three\n4. four\n");
        let Block::List { ordered: false, items, loose: false, .. } = &doc.blocks[0] else { panic!("{:?}", doc.blocks) };
        assert_eq!(items[0].task, Some(true));
        assert_eq!(items[1].task, Some(false));
        assert_eq!(text_of(&items[0].blocks[0]), "done");
        assert!(matches!(&items[1].blocks[1], Block::List { ordered: false, items, .. } if items.len() == 1));
        assert!(matches!(&doc.blocks[1], Block::List { ordered: true, start: 3, items, .. } if items.len() == 2));
    }

    #[test]
    fn loose_lists_are_detected() {
        let doc = Document::parse("- one\n\n- two\n");
        assert!(matches!(&doc.blocks[0], Block::List { loose: true, .. }));
    }

    #[test]
    fn front_matter_becomes_a_table() {
        let doc = Document::parse("---\ntitle: \"Hello\"\ntags:\n  - a\n  - b\ndraft: false\n---\n\n# Body\n");
        let Block::Table(table) = &doc.blocks[0] else { panic!("{:?}", doc.blocks) };
        assert_eq!(table.head.iter().map(Inline::plain).collect::<Vec<_>>(), vec!["title", "tags", "draft"]);
        assert_eq!(table.rows[0].iter().map(Inline::plain).collect::<Vec<_>>(), vec!["Hello", "a, b", "false"]);
        assert!(matches!(&doc.blocks[1], Block::Heading { level: 1, .. }));
    }

    #[test]
    fn inline_styles_links_and_code() {
        let doc = Document::parse("Some **bold** and *it* `code` ~~gone~~ [link](https://x.y).\n");
        let Block::Paragraph { content, .. } = &doc.blocks[0] else { panic!() };
        let Segment::Text(rich) = &content.segments[0] else { panic!() };
        assert_eq!(rich.text, "Some bold and it code gone link.");
        let style_of = |needle: &str| {
            let start = rich.text.find(needle).unwrap();
            rich.runs.iter().find(|run| run.range.start <= start && start < run.range.end).unwrap().clone()
        };
        assert!(style_of("bold").style.bold);
        assert!(style_of("it ").style.italic);
        assert!(style_of("code").style.code);
        assert!(style_of("gone").style.strike);
        let link = style_of("link");
        assert_eq!(rich.links[link.link.unwrap()], "https://x.y");
    }

    #[test]
    fn centered_html_with_badges_and_heading() {
        let source = "<p align=\"center\">\n  <img src=\"logo.png\" width=\"120\" alt=\"Logo\">\n</p>\n\n<h1 align=\"center\">My App</h1>\n\n<p align=\"center\">\n<a href=\"https://ci\"><img src=\"https://badge/ci.svg\" alt=\"CI\"></a>\n<a href=\"https://lic\"><img src=\"https://badge/lic.svg\" alt=\"License\"></a>\n</p>\n";
        let doc = Document::parse(source);
        assert_eq!(doc.blocks.len(), 3, "{:#?}", doc.blocks);
        let Block::Container { align: Align::Center, spaced: true, blocks } = &doc.blocks[0] else { panic!() };
        let Block::Paragraph { content, .. } = &blocks[0] else { panic!() };
        assert!(matches!(&content.segments[0], Segment::Image(ImageRef { src, width: Some(w), alt, .. }) if src == "logo.png" && *w == 120. && alt == "Logo"));
        let Block::Container { align: Align::Center, blocks, .. } = &doc.blocks[1] else { panic!() };
        assert!(matches!(&blocks[0], Block::Heading { level: 1, slug, .. } if slug == "my-app"));
        assert_eq!(doc.anchor("#my-app"), Some(1));
        let Block::Container { blocks, .. } = &doc.blocks[2] else { panic!() };
        let Block::Paragraph { content, .. } = &blocks[0] else { panic!() };
        let images: Vec<_> = content
            .segments
            .iter()
            .filter_map(|s| match s {
                Segment::Image(image) => Some((image.alt.clone(), image.link.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(
            images,
            vec![("CI".into(), Some("https://ci".into())), ("License".into(), Some("https://lic".into()))]
        );
    }

    #[test]
    fn details_summary_and_inline_html() {
        let source = "<details>\n<summary>More <b>info</b></summary>\n\nHidden *text*.\n\n</details>\n\nPress <kbd>Ctrl</kbd>+<kbd>C</kbd><br>done &amp; dusted <span>kept</span>\n";
        let doc = Document::parse(source);
        let Block::Details { id: 0, open: false, summary, blocks } = &doc.blocks[0] else { panic!("{:#?}", doc.blocks) };
        assert_eq!(summary.plain(), "More info");
        assert_eq!(text_of(&blocks[0]), "Hidden text.");
        let Block::Paragraph { content, .. } = &doc.blocks[1] else { panic!() };
        let Segment::Text(rich) = &content.segments[0] else { panic!() };
        assert_eq!(rich.text, "Press Ctrl+C\ndone & dusted kept");
        let ctrl = rich.runs.iter().find(|run| &rich.text[run.range.clone()] == "Ctrl").unwrap();
        assert!(ctrl.style.kbd);
    }

    #[test]
    fn footnotes_collect_at_the_end_in_reference_order() {
        let doc = Document::parse("A[^b] and B[^a].\n\n[^a]: Alpha.\n[^b]: Beta.\n\nAfter.\n");
        let Block::Footnotes(notes) = doc.blocks.last().unwrap() else { panic!("{:#?}", doc.blocks) };
        assert_eq!(notes.iter().map(|n| (n.number, n.label.as_str())).collect::<Vec<_>>(), vec![(1, "b"), (2, "a")]);
        assert_eq!(text_of(&notes[0].blocks[0]), "Beta.");
        let Block::Paragraph { content, .. } = &doc.blocks[0] else { panic!() };
        assert_eq!(content.plain(), "A[1] and B[2].");
        assert_eq!(doc.anchor("#footnote-a"), Some(doc.blocks.len() - 1));
    }

    #[test]
    fn code_blocks_keep_language_and_text() {
        let doc = Document::parse("```rust title=x\nfn main() {}\n```\n\n    indented\n");
        assert!(matches!(&doc.blocks[0], Block::Code(CodeBlock { lang: Some(l), code, .. }) if l == "rust" && code == "fn main() {}"));
        assert!(matches!(&doc.blocks[1], Block::Code(CodeBlock { lang: None, code, .. }) if code == "indented"));
    }

    #[test]
    fn root_blocks_map_to_source_lines() {
        let doc = Document::parse("# A\n\npara\nmore\n\n- x\n- y\n\n```\ncode\n```\n");
        assert_eq!(doc.lines, vec![0, 2, 5, 8]);
        assert_eq!(doc.block_for_line(0), Some(0));
        assert_eq!(doc.block_for_line(3), Some(1));
        assert_eq!(doc.block_for_line(6), Some(2));
        assert_eq!(doc.block_for_line(100), Some(3));
    }

    #[test]
    fn rich_text_splits_on_hard_breaks() {
        let doc = Document::parse("**a**  \nb [c](x)\n");
        let Block::Paragraph { content, .. } = &doc.blocks[0] else { panic!() };
        let Segment::Text(rich) = &content.segments[0] else { panic!() };
        let lines = rich.split_lines();
        assert_eq!(lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), vec!["a", "b c"]);
        assert!(lines[0].runs[0].style.bold);
        let link = lines[1].runs.iter().find(|r| r.link.is_some()).unwrap();
        assert_eq!(link.range, 2..3);
        assert_eq!(lines[1].links[link.link.unwrap()], "x");
    }

    #[test]
    fn html_tokenizer_handles_attributes_and_comments() {
        let tokens = tokenize("<img src='a b.png' width=10 alt=\"x\"/><!-- c --><br>t</p>");
        assert_eq!(
            tokens,
            vec![
                HtmlToken::Open {
                    name: "img".into(),
                    attrs: vec![("src".into(), "a b.png".into()), ("width".into(), "10".into()), ("alt".into(), "x".into())],
                    self_closing: true
                },
                HtmlToken::Open { name: "br".into(), attrs: vec![], self_closing: false },
                HtmlToken::Text("t".into()),
                HtmlToken::Close("p".into()),
            ]
        );
        assert_eq!(decode_entities("a &lt;b&gt; &#65;&#x42; &bogus; &"), "a <b> AB &bogus; &");
    }
}
