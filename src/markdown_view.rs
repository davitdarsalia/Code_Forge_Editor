use std::collections::HashSet;
use std::ops::Range;
use std::path::Path;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::base::Scrollbar;
use gpui_kit::component::highlighter::{HighlightTheme, LanguageRegistry, SyntaxHighlighter};
use gpui_kit::component::input::Rope;
use gpui_kit::component::{Icon, ThemeMode};
use gpui_kit::{
    AnyElement, App, ClipboardItem, Context, FontStyle, FontWeight, HighlightStyle, Hsla, InteractiveElement as _,
    InteractiveText, IntoElement, ListAlignment, ListOffset, ListState, ParentElement as _, Render, SharedString,
    SharedUri, StatefulInteractiveElement as _, StrikethroughStyle, Styled as _, StyledImage as _, StyledText, Task,
    UnderlineStyle, WeakEntity, Window, div, img, list, prelude::FluentBuilder as _, px, relative, rgba,
};

use crate::markdown_model::{AlertKind, Align, Block, CodeBlock, Document, Footnote, ImageRef, Inline, ListItem, Rich, Segment, Table};
use crate::preview_pane::{open_link, resolve_image};
use crate::theme::{UI_FONT, c, mono_font};

const BASE_SIZE: f32 = 16.;
const CODE_SIZE: f32 = BASE_SIZE * 0.85;
const MAX_WIDTH: f32 = 980.;
const WIDE_PANE: f32 = 768.;
const PAD_NARROW: f32 = 32.;
const PAD_WIDE: f32 = 45.;
const MARGIN: f32 = 16.;
const HEADING_TOP: f32 = 24.;
const LIST_INDENT: f32 = 32.;
const MAX_HIGHLIGHT_BYTES: usize = 256 * 1024;
const COPIED_FOR: Duration = Duration::from_secs(2);
const CHAR_WIDTH: f32 = 8.2;

#[derive(Clone, Copy)]
pub struct Gh {
    pub bg: Hsla,
    pub fg: Hsla,
    pub muted: Hsla,
    pub border: Hsla,
    pub border_muted: Hsla,
    pub link: Hsla,
    pub code_bg: Hsla,
    pub pre_bg: Hsla,
    pub zebra: Hsla,
    pub alerts: [Hsla; 5],
}

fn hex_alpha(hex: u32) -> Hsla {
    rgba(hex).into()
}

impl Gh {
    pub fn new(dark: bool) -> Gh {
        if dark {
            Gh {
                bg: c(0x0d1117),
                fg: c(0xf0f6fc),
                muted: c(0x9198a1),
                border: c(0x3d444d),
                border_muted: hex_alpha(0x3d444db3),
                link: c(0x4493f8),
                code_bg: hex_alpha(0x656c7633),
                pre_bg: c(0x151b23),
                zebra: c(0x151b23),
                alerts: [c(0x4493f8), c(0x3fb950), c(0xab7df8), c(0xd29922), c(0xf85149)],
            }
        } else {
            Gh {
                bg: c(0xffffff),
                fg: c(0x1f2328),
                muted: c(0x59636e),
                border: c(0xd1d9e0),
                border_muted: hex_alpha(0xd1d9e0b3),
                link: c(0x0969da),
                code_bg: hex_alpha(0x818b981f),
                pre_bg: c(0xf6f8fa),
                zebra: c(0xf6f8fa),
                alerts: [c(0x0969da), c(0x1a7f37), c(0x8250df), c(0x9a6700), c(0xd1242f)],
            }
        }
    }

    fn alert(&self, kind: AlertKind) -> Hsla {
        self.alerts[match kind {
            AlertKind::Note => 0,
            AlertKind::Tip => 1,
            AlertKind::Important => 2,
            AlertKind::Warning => 3,
            AlertKind::Caution => 4,
        }]
    }
}

pub fn is_dark(cx: &App) -> bool {
    gpui_kit::component::Theme::global(cx).mode.is_dark()
}

fn alert_icon(kind: AlertKind) -> IconName {
    match kind {
        AlertKind::Note => IconName::Info,
        AlertKind::Tip => IconName::Lightbulb,
        AlertKind::Important => IconName::MessageSquareWarning,
        AlertKind::Warning => IconName::TriangleAlert,
        AlertKind::Caution => IconName::OctagonAlert,
    }
}

pub fn heading_size(level: u8) -> f32 {
    BASE_SIZE
        * match level {
            1 => 2.,
            2 => 1.5,
            3 => 1.25,
            4 => 1.,
            5 => 0.875,
            _ => 0.85,
        }
}

pub fn article_padding(width: f32) -> f32 {
    if width >= WIDE_PANE { PAD_WIDE } else { PAD_NARROW }
}

pub fn column_width(chars: usize) -> f32 {
    (chars as f32 * CHAR_WIDTH + 28.).clamp(48., 520.)
}

pub fn margins(block: &Block, in_item: bool) -> (f32, f32) {
    match block {
        Block::Heading { .. } => (HEADING_TOP, MARGIN),
        Block::Paragraph { tight: true, .. } => (0., 0.),
        Block::List { .. } if in_item => (0., 0.),
        Block::Rule => (HEADING_TOP, HEADING_TOP),
        Block::Footnotes(_) => (MARGIN, 0.),
        Block::Container { spaced, blocks, .. } => {
            let top = blocks.first().map(|b| margins(b, in_item).0).unwrap_or(0.);
            let bottom = blocks.last().map(|b| margins(b, in_item).1).unwrap_or(0.);
            (top, if *spaced { bottom.max(MARGIN) } else { bottom })
        }
        _ => (0., MARGIN),
    }
}

pub fn bullet(depth: usize) -> &'static str {
    ["•", "◦", "▪"][depth % 3]
}

pub fn fence_language(lang: &str) -> Option<String> {
    let lower = lang.trim().to_ascii_lowercase();
    if lower.is_empty() {
        return None;
    }
    let registry = LanguageRegistry::singleton();
    let alias = match lower.as_str() {
        "sh" | "shell" | "zsh" | "console" | "shellsession" | "shell-session" => "bash",
        "js" | "jsx" | "mjs" | "cjs" | "node" => "javascript",
        "ts" | "mts" | "cts" => "typescript",
        "py" | "python3" => "python",
        "rb" => "ruby",
        "rs" => "rust",
        "yml" => "yaml",
        "kt" | "kts" => "kotlin",
        "c++" | "cc" | "cxx" | "hpp" => "cpp",
        "golang" => "go",
        "md" => "markdown",
        "jsonc" | "json5" => "json",
        other => other,
    };
    if registry.language(alias).is_some() {
        return Some(alias.to_string());
    }
    let by_extension = crate::files::language_for(Path::new(&format!("x.{lower}")));
    (by_extension != "text" && registry.language(by_extension).is_some()).then(|| by_extension.to_string())
}

fn theme(dark: bool) -> Arc<HighlightTheme> {
    static DARK: OnceLock<Arc<HighlightTheme>> = OnceLock::new();
    static LIGHT: OnceLock<Arc<HighlightTheme>> = OnceLock::new();
    let (cell, mode) = if dark { (&DARK, ThemeMode::Dark) } else { (&LIGHT, ThemeMode::Light) };
    cell.get_or_init(|| crate::theme::highlight_theme(mode)).clone()
}

pub fn highlight(doc: &mut Document, dark: bool) {
    let theme = theme(dark);
    for code in doc.code_blocks_mut() {
        if code.code.len() > MAX_HIGHLIGHT_BYTES {
            continue;
        }
        let Some(lang) = code.lang.as_deref().and_then(fence_language) else { continue };
        let mut highlighter = SyntaxHighlighter::new(&lang);
        highlighter.update(None, &Rope::from(code.code.as_str()), None);
        code.highlights = normalize(highlighter.styles(&(0..code.code.len()), &*theme), &code.code);
    }
}

pub fn normalize(mut spans: Vec<(Range<usize>, HighlightStyle)>, text: &str) -> Vec<(Range<usize>, HighlightStyle)> {
    spans.sort_by_key(|(range, _)| range.start);
    let mut out: Vec<(Range<usize>, HighlightStyle)> = Vec::with_capacity(spans.len());
    let mut end = 0;
    for (range, style) in spans {
        let start = range.start.max(end);
        if start >= range.end || range.end > text.len() || !text.is_char_boundary(start) || !text.is_char_boundary(range.end) {
            continue;
        }
        end = range.end;
        out.push((start..range.end, style));
    }
    out
}

pub fn parse(text: &str, dark: bool) -> Document {
    let mut doc = Document::parse(text);
    highlight(&mut doc, dark);
    doc
}

pub struct MarkdownView {
    doc: Arc<Document>,
    list: ListState,
    base: Arc<Path>,
    toggled: HashSet<usize>,
    hovered: Option<(SharedString, usize)>,
    copied: Option<SharedString>,
    copied_task: Option<Task<()>>,
    padding: f32,
}

impl MarkdownView {
    pub fn new(base: Arc<Path>) -> Self {
        MarkdownView {
            doc: Arc::new(Document::default()),
            list: ListState::new(0, ListAlignment::Top, px(600.)),
            base,
            toggled: HashSet::new(),
            hovered: None,
            copied: None,
            copied_task: None,
            padding: PAD_NARROW,
        }
    }

    pub fn set_document(&mut self, doc: Document, cx: &mut Context<Self>) {
        let top = self.list.logical_scroll_top();
        let count = doc.blocks.len();
        self.list.reset(count);
        if top.item_ix < count {
            self.list.scroll_to(top);
        }
        self.doc = Arc::new(doc);
        self.hovered = None;
        cx.notify();
    }

    pub fn reveal_line(&mut self, line: usize, cx: &mut Context<Self>) {
        let Some(ix) = self.doc.block_for_line(line) else { return };
        if self.list.logical_scroll_top().item_ix == ix {
            return;
        }
        self.list.scroll_to(ListOffset { item_ix: ix, offset_in_item: px(0.) });
        cx.notify();
    }

    fn open(&mut self, url: &str, cx: &mut Context<Self>) {
        if url.starts_with('#') {
            if let Some(ix) = self.doc.anchor(url) {
                self.list.scroll_to(ListOffset { item_ix: ix, offset_in_item: px(0.) });
                cx.notify();
            }
            return;
        }
        open_link(&self.base, url, cx);
    }

    fn set_hover(&mut self, hovered: Option<(SharedString, usize)>, cx: &mut Context<Self>) {
        if self.hovered != hovered {
            self.hovered = hovered;
            cx.notify();
        }
    }

    fn toggle(&mut self, id: usize, root: usize, cx: &mut Context<Self>) {
        if !self.toggled.remove(&id) {
            self.toggled.insert(id);
        }
        self.list.remeasure_items(root..root + 1);
        cx.notify();
    }

    fn copy(&mut self, id: SharedString, code: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(code));
        self.copied = Some(id);
        self.copied_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(COPIED_FOR).await;
            let _ = this.update(cx, |this, cx| {
                this.copied = None;
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn render_item(&mut self, ix: usize, _: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let doc = self.doc.clone();
        let Some(block) = doc.blocks.get(ix) else { return div().into_any_element() };
        let pad = self.padding;
        let top = match ix.checked_sub(1).and_then(|prev| doc.blocks.get(prev)) {
            Some(prev) => margins(prev, false).1.max(margins(block, false).0),
            None => pad,
        };
        let bottom = if ix + 1 == doc.blocks.len() { pad } else { 0. };
        let mut ctx = Ctx {
            gh: Gh::new(is_dark(cx)),
            view: cx.entity().downgrade(),
            base: self.base.clone(),
            hovered: self.hovered.clone(),
            copied: self.copied.clone(),
            toggled: &self.toggled,
            root: ix,
            counter: 0,
            align: Align::Default,
        };
        let element = ctx.block(block, 0, false);
        div()
            .w_full()
            .flex()
            .justify_center()
            .child(div().w_full().max_w(px(MAX_WIDTH)).px(px(pad)).pt(px(top)).pb(px(bottom)).child(element))
            .into_any_element()
    }
}

struct Ctx<'a> {
    gh: Gh,
    view: WeakEntity<MarkdownView>,
    base: Arc<Path>,
    hovered: Option<(SharedString, usize)>,
    copied: Option<SharedString>,
    toggled: &'a HashSet<usize>,
    root: usize,
    counter: usize,
    align: Align,
}

impl Ctx<'_> {
    fn id(&mut self, kind: &str) -> SharedString {
        self.counter += 1;
        format!("md-{}-{kind}-{}", self.root, self.counter).into()
    }

    fn blocks(&mut self, blocks: &[Block], depth: usize, in_item: bool) -> Vec<AnyElement> {
        let mut out = Vec::with_capacity(blocks.len());
        let mut previous: Option<f32> = None;
        for block in blocks {
            let (top, bottom) = margins(block, in_item);
            let gap = previous.map(|prev| prev.max(top)).unwrap_or(0.);
            out.push(div().pt(px(gap)).child(self.block(block, depth, in_item)).into_any_element());
            previous = Some(bottom);
        }
        out
    }

    fn block(&mut self, block: &Block, depth: usize, in_item: bool) -> AnyElement {
        let gh = self.gh;
        match block {
            Block::Heading { level, content, .. } => {
                let size = heading_size(*level);
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_size(px(size))
                    .line_height(relative(1.25))
                    .when(*level <= 2, |el| el.pb(px(size * 0.3)).border_b_1().border_color(gh.border_muted))
                    .when(*level == 6, |el| el.text_color(gh.muted))
                    .child(self.inline(content))
                    .into_any_element()
            }
            Block::Paragraph { content, .. } => self.inline(content),
            Block::Code(code) => self.code(code),
            Block::Quote(children) => div()
                .border_l_4()
                .border_color(gh.border)
                .px(px(16.))
                .text_color(gh.muted)
                .children(self.blocks(children, depth, false))
                .into_any_element(),
            Block::Alert { kind, blocks } => {
                let color = gh.alert(*kind);
                div()
                    .border_l_4()
                    .border_color(color)
                    .px(px(16.))
                    .py(px(8.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .mb(px(MARGIN))
                            .line_height(relative(1.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(color)
                            .child(Icon::new(alert_icon(*kind)).size(px(16.)).text_color(color))
                            .child(kind.title()),
                    )
                    .children(self.blocks(blocks, depth, false))
                    .into_any_element()
            }
            Block::List { ordered, start, items, loose } => self.list(*ordered, *start, items, *loose, depth),
            Block::Table(table) => self.table(table),
            Block::Rule => div().h(px(4.)).bg(gh.border).into_any_element(),
            Block::Container { align, blocks, .. } => {
                let saved = self.align;
                if *align != Align::Default {
                    self.align = *align;
                }
                let children = self.blocks(blocks, depth, in_item);
                let el = div().flex().flex_col().children(children);
                let el = match self.align {
                    Align::Center => el.text_center(),
                    Align::Right => el.text_right(),
                    _ => el,
                };
                self.align = saved;
                el.into_any_element()
            }
            Block::Details { id, open, summary, blocks } => {
                let open = *open != self.toggled.contains(id);
                let (details_id, root) = (*id, self.root);
                let view = self.view.clone();
                let summary_id = self.id("summary");
                let summary = self.inline(summary);
                div()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .id(summary_id)
                            .flex()
                            .gap(px(6.))
                            .cursor_pointer()
                            .child(div().flex_none().text_color(gh.muted).child(if open { "▼" } else { "▶" }))
                            .child(div().flex_1().min_w(px(0.)).child(summary))
                            .on_click(move |_, _, cx| {
                                let _ = view.update(cx, |view, cx| view.toggle(details_id, root, cx));
                            }),
                    )
                    .when(open, |el| el.child(div().pt(px(8.)).children(self.blocks(blocks, depth, false))))
                    .into_any_element()
            }
            Block::Footnotes(notes) => self.footnotes(notes),
        }
    }

    fn rich(&mut self, rich: &Rich) -> AnyElement {
        let gh = self.gh;
        let id = self.id("text");
        let hovered_link = self.hovered.as_ref().filter(|(hovered, _)| *hovered == id).map(|(_, link)| *link);
        let mut highlights = Vec::new();
        let mut fonts: Vec<(Range<usize>, SharedString)> = Vec::new();
        let mut clicks: Vec<Range<usize>> = Vec::new();
        let mut targets: Vec<String> = Vec::new();
        let mut hover_map: Vec<(Range<usize>, usize)> = Vec::new();
        for run in &rich.runs {
            let mut style = HighlightStyle::default();
            if run.style.bold {
                style.font_weight = Some(FontWeight::SEMIBOLD);
            }
            if run.style.italic {
                style.font_style = Some(FontStyle::Italic);
            }
            if run.style.strike {
                style.strikethrough = Some(StrikethroughStyle { thickness: px(1.), color: None });
            }
            if run.style.code || run.style.kbd {
                style.background_color = Some(gh.code_bg);
                fonts.push((run.range.clone(), mono_font().into()));
            }
            if let Some(link) = run.link {
                style.color = Some(gh.link);
                if hovered_link == Some(link) {
                    style.underline = Some(UnderlineStyle { thickness: px(1.), color: Some(gh.link), wavy: false });
                }
                if let Some(url) = rich.links.get(link) {
                    clicks.push(run.range.clone());
                    targets.push(url.clone());
                    hover_map.push((run.range.clone(), link));
                }
            }
            if style != HighlightStyle::default() {
                highlights.push((run.range.clone(), style));
            }
        }
        let text = StyledText::new(SharedString::from(rich.text.clone()))
            .with_highlights(highlights)
            .with_font_family_overrides(fonts);
        if clicks.is_empty() {
            return text.into_any_element();
        }
        let click_view = self.view.clone();
        let hover_view = self.view.clone();
        let hover_id = id.clone();
        InteractiveText::new(id, text)
            .on_click(clicks, move |ix, _, cx| {
                if let Some(url) = targets.get(ix).cloned() {
                    let _ = click_view.update(cx, |view, cx| view.open(&url, cx));
                }
            })
            .on_hover(move |index, _, _, cx| {
                let link = index.and_then(|index| {
                    hover_map.iter().find(|(range, _)| range.contains(&index)).map(|(_, link)| *link)
                });
                let hovered = link.map(|link| (hover_id.clone(), link));
                let _ = hover_view.update(cx, |view, cx| view.set_hover(hovered, cx));
            })
            .into_any_element()
    }

    fn image(&mut self, image: &ImageRef) -> AnyElement {
        let gh = self.gh;
        let source = resolve_image(&self.base, &SharedUri::from(image.src.clone()));
        let alt = SharedString::from(image.alt.clone());
        let mut element = img(source).max_w(relative(1.)).with_fallback(move || {
            div().text_color(gh.muted).child(alt.clone()).into_any_element()
        });
        if let Some(width) = image.width {
            element = element.w(px(width));
        }
        if let Some(height) = image.height {
            element = element.h(px(height));
        }
        match image.link.clone() {
            Some(url) => {
                let view = self.view.clone();
                div()
                    .id(self.id("img"))
                    .max_w(relative(1.))
                    .cursor_pointer()
                    .child(element)
                    .on_click(move |_, _, cx| {
                        let _ = view.update(cx, |view, cx| view.open(&url, cx));
                    })
                    .into_any_element()
            }
            None => element.into_any_element(),
        }
    }

    fn inline(&mut self, content: &Inline) -> AnyElement {
        if let [Segment::Text(rich)] = content.segments.as_slice() {
            return self.rich(rich);
        }
        let mut rows: Vec<Vec<AnyElement>> = vec![Vec::new()];
        for segment in &content.segments {
            match segment {
                Segment::Image(image) => {
                    let element = self.image(image);
                    rows.last_mut().expect("rows start non-empty").push(element);
                }
                Segment::Text(rich) => {
                    for (ix, line) in rich.split_lines().into_iter().enumerate() {
                        if ix > 0 {
                            rows.push(Vec::new());
                        }
                        if line.text.is_empty() {
                            continue;
                        }
                        let element = if line.text.trim().is_empty() {
                            div().w(px(4.)).into_any_element()
                        } else {
                            self.rich(&line)
                        };
                        rows.last_mut().expect("rows start non-empty").push(element);
                    }
                }
            }
        }
        let align = self.align;
        div()
            .flex()
            .flex_col()
            .children(rows.into_iter().map(|row| {
                div()
                    .flex()
                    .flex_wrap()
                    .items_end()
                    .min_h(px(BASE_SIZE * 1.5))
                    .when(align == Align::Center, |el| el.justify_center())
                    .when(align == Align::Right, |el| el.justify_end())
                    .children(row)
            }))
            .into_any_element()
    }

    fn code(&mut self, code: &CodeBlock) -> AnyElement {
        let gh = self.gh;
        let id = self.id("code");
        let copied = self.copied.as_ref() == Some(&id);
        let text = if code.code.is_empty() { " ".to_string() } else { code.code.clone() };
        let view = self.view.clone();
        let source = code.code.clone();
        let copy_id = id.clone();
        div()
            .group(id.clone())
            .relative()
            .rounded(px(6.))
            .bg(gh.pre_bg)
            .child(
                div()
                    .id(SharedString::from(format!("{id}-scroll")))
                    .overflow_x_scroll()
                    .w_full()
                    .child(
                        div()
                            .p(px(16.))
                            .whitespace_nowrap()
                            .font_family(mono_font())
                            .text_size(px(CODE_SIZE))
                            .line_height(relative(1.45))
                            .child(StyledText::new(SharedString::from(text)).with_highlights(code.highlights.clone())),
                    ),
            )
            .child(
                div()
                    .id(SharedString::from(format!("{id}-copy")))
                    .absolute()
                    .top(px(8.))
                    .right(px(8.))
                    .size(px(28.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(gh.border)
                    .bg(gh.pre_bg)
                    .cursor_pointer()
                    .opacity(if copied { 1. } else { 0. })
                    .group_hover(id, |style| style.opacity(1.))
                    .hover(|style| style.bg(gh.code_bg))
                    .child(
                        Icon::new(if copied { IconName::Check } else { IconName::Copy })
                            .size(px(14.))
                            .text_color(if copied { gh.alerts[1] } else { gh.muted }),
                    )
                    .on_click(move |_, _, cx| {
                        let (id, source) = (copy_id.clone(), source.clone());
                        let _ = view.update(cx, |view, cx| view.copy(id, source, cx));
                    }),
            )
            .into_any_element()
    }

    fn list(&mut self, ordered: bool, start: u64, items: &[ListItem], loose: bool, depth: usize) -> AnyElement {
        let gh = self.gh;
        let mut rows = Vec::with_capacity(items.len());
        for (ix, item) in items.iter().enumerate() {
            let gap = if ix == 0 { 0. } else if loose { MARGIN } else { 4. };
            let marker = match item.task {
                Some(checked) => checkbox(checked, gh),
                None if ordered => div().child(format!("{}.", start + ix as u64)).into_any_element(),
                None => div().child(bullet(depth)).into_any_element(),
            };
            let children = self.blocks(&item.blocks, depth + 1, true);
            rows.push(
                div()
                    .pt(px(gap))
                    .flex()
                    .child(div().flex_none().w(px(LIST_INDENT)).flex().justify_end().pr(px(6.)).child(marker))
                    .child(div().flex_1().min_w(px(0.)).flex().flex_col().children(children)),
            );
        }
        div().flex().flex_col().children(rows).into_any_element()
    }

    fn table(&mut self, table: &Table) -> AnyElement {
        let gh = self.gh;
        let widths: Vec<f32> = table.widths.iter().map(|&chars| column_width(chars)).collect();
        let total: f32 = widths.iter().sum::<f32>() + 1.;
        let mut rows = Vec::with_capacity(table.rows.len() + 1);
        let header = self.table_row(&table.head, &widths, &table.aligns, true, None);
        rows.push(header);
        for (ix, row) in table.rows.iter().enumerate() {
            let bg = (ix % 2 == 1).then_some(gh.zebra);
            let row = self.table_row(row, &widths, &table.aligns, false, bg);
            rows.push(row);
        }
        div()
            .id(self.id("table"))
            .overflow_x_scroll()
            .w_full()
            .child(div().w(px(total)).flex().flex_col().border_t_1().border_l_1().border_color(gh.border).children(rows))
            .into_any_element()
    }

    fn table_row(&mut self, cells: &[Inline], widths: &[f32], aligns: &[Align], header: bool, bg: Option<Hsla>) -> AnyElement {
        let gh = self.gh;
        let saved = self.align;
        let mut row = div().flex().when_some(bg, |el, bg| el.bg(bg));
        for (ix, width) in widths.iter().enumerate() {
            let align = aligns.get(ix).copied().unwrap_or(Align::Default);
            let align = if header && align == Align::Default { Align::Center } else { align };
            self.align = align;
            let content = cells.get(ix).map(|cell| self.inline(cell));
            let cell = div()
                .w(px(*width))
                .flex_none()
                .px(px(13.))
                .py(px(6.))
                .border_r_1()
                .border_b_1()
                .border_color(gh.border)
                .when(header, |el| el.font_weight(FontWeight::SEMIBOLD))
                .when(align == Align::Center, |el| el.text_center())
                .when(align == Align::Right, |el| el.text_right())
                .children(content);
            row = row.child(cell);
        }
        self.align = saved;
        row.into_any_element()
    }

    fn footnotes(&mut self, notes: &[Footnote]) -> AnyElement {
        let gh = self.gh;
        let mut rows = Vec::with_capacity(notes.len());
        for (ix, note) in notes.iter().enumerate() {
            let children = self.blocks(&note.blocks, 1, true);
            rows.push(
                div()
                    .when(ix > 0, |el| el.pt(px(4.)))
                    .flex()
                    .child(div().flex_none().w(px(LIST_INDENT)).flex().justify_end().pr(px(6.)).child(format!("{}.", note.number)))
                    .child(div().flex_1().min_w(px(0.)).flex().flex_col().children(children)),
            );
        }
        div()
            .border_t_1()
            .border_color(gh.border)
            .pt(px(MARGIN))
            .text_size(px(12.))
            .text_color(gh.muted)
            .flex()
            .flex_col()
            .children(rows)
            .into_any_element()
    }
}

fn checkbox(checked: bool, gh: Gh) -> AnyElement {
    div()
        .mt(px(5.))
        .size(px(13.))
        .rounded(px(3.))
        .border_1()
        .flex()
        .items_center()
        .justify_center()
        .when(checked, |el| el.bg(gh.link).border_color(gh.link))
        .when(!checked, |el| el.border_color(gh.muted))
        .opacity(0.75)
        .when(checked, |el| el.child(Icon::new(IconName::Check).size(px(10.)).text_color(gh.bg)))
        .into_any_element()
}

impl Render for MarkdownView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.padding = article_padding(f32::from(self.list.viewport_bounds().size.width));
        let gh = Gh::new(is_dark(cx));
        div()
            .size_full()
            .relative()
            .bg(gh.bg)
            .text_color(gh.fg)
            .font_family(UI_FONT)
            .text_size(px(BASE_SIZE))
            .line_height(relative(1.5))
            .child(list(self.list.clone(), cx.processor(Self::render_item)).size_full())
            .child(Scrollbar::vertical(&self.list))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heading_sizes_follow_github() {
        assert_eq!(heading_size(1), 32.);
        assert_eq!(heading_size(2), 24.);
        assert_eq!(heading_size(3), 20.);
        assert_eq!(heading_size(4), 16.);
        assert_eq!(heading_size(5), 14.);
        assert_eq!(heading_size(6), 13.6);
    }

    #[test]
    fn padding_widens_on_large_panes() {
        assert_eq!(article_padding(500.), 32.);
        assert_eq!(article_padding(768.), 45.);
    }

    #[test]
    fn sibling_margins_collapse_like_css() {
        let doc = Document::parse("para\n\n# Head\n\n---\n\n- a\n");
        let m: Vec<_> = doc.blocks.iter().map(|b| margins(b, false)).collect();
        assert_eq!(m, vec![(0., 16.), (24., 16.), (24., 24.), (0., 16.)]);
        let centered = Document::parse("<p align=\"center\">\n<img src=\"x.png\">\n</p>\n");
        assert_eq!(margins(&centered.blocks[0], false), (0., 16.));
    }

    #[test]
    fn bullets_cycle_by_depth() {
        assert_eq!((bullet(0), bullet(1), bullet(2), bullet(3)), ("•", "◦", "▪", "•"));
    }

    #[test]
    fn fence_languages_resolve_through_aliases() {
        crate::languages::register();
        assert_eq!(fence_language("rs").as_deref(), Some("rust"));
        assert_eq!(fence_language("Rust").as_deref(), Some("rust"));
        assert_eq!(fence_language("sh").as_deref(), Some("bash"));
        assert_eq!(fence_language("ts").as_deref(), Some("typescript"));
        assert_eq!(fence_language("swift").as_deref(), Some("swift"));
        assert_eq!(fence_language("nonsense-lang"), None);
        assert_eq!(fence_language(""), None);
    }

    #[test]
    fn code_blocks_are_highlighted_off_thread() {
        crate::languages::register();
        let doc = std::thread::spawn(|| parse("```rust\nfn main() { let x = 1; }\n```\n", true)).join().unwrap();
        let Block::Code(code) = &doc.blocks[0] else { panic!() };
        assert!(code.highlights.iter().any(|(range, style)| &code.code[range.clone()] == "fn" && style.color.is_some()));
    }

    #[test]
    fn highlight_spans_are_sorted_and_disjoint() {
        let style = HighlightStyle::default();
        let spans = vec![(4..8, style), (0..3, style), (2..5, style), (6..7, style), (9..20, style)];
        let ranges: Vec<_> = normalize(spans, "abcdefghij").into_iter().map(|(range, _)| range).collect();
        assert_eq!(ranges, vec![0..3, 3..5, 5..8]);
    }

    #[test]
    fn github_palettes() {
        assert_eq!(Gh::new(false).bg, c(0xffffff));
        assert_eq!(Gh::new(true).bg, c(0x0d1117));
        assert!((Gh::new(true).border_muted.a - 0xb3 as f32 / 255.).abs() < 0.01);
    }
}
