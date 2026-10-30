use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::input::{
    EditorState, Escape, IndentInline, Input, InputEvent, InputState, MoveDown, MoveUp, OutdentInline,
};
use gpui_kit::{
    Action, AnyElement, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable,
    FontWeight, HighlightStyle, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    ScrollHandle, SharedString, StatefulInteractiveElement as _, Styled as _, StyledText,
    Subscription, Task, Window, div, prelude::FluentBuilder as _, px,
};

use crate::commands::{self, COMMANDS};
use crate::file_icons;
use crate::search::{self, FileIndex, FuzzyHit, LineMatch, SearchHandle, SearchOptions};
use crate::symbols::{self, Symbol};
use crate::theme::{Palette, ca};

const MAX_RESULTS: usize = 60;
const ASYNC_FUZZY_MIN_FILES: usize = 5000;
const MAX_TEXT_RESULTS: usize = 200;
const TEXT_DEBOUNCE: Duration = Duration::from_millis(80);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Files,
    Text,
    Line,
    Commands,
    Symbols,
}

impl Mode {
    pub const ALL: [Mode; 5] = [Mode::Files, Mode::Text, Mode::Line, Mode::Commands, Mode::Symbols];

    pub fn prefix(self) -> &'static str {
        match self {
            Mode::Files => "",
            Mode::Text => "#",
            Mode::Line => ":",
            Mode::Commands => ">",
            Mode::Symbols => "@",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Mode::Files => "Files",
            Mode::Text => "Text",
            Mode::Line => "Line",
            Mode::Commands => "Commands",
            Mode::Symbols => "Symbols",
        }
    }

    fn placeholder(self) -> &'static str {
        match self {
            Mode::Files => "Open file by name",
            Mode::Text => "Search text in project",
            Mode::Line => "Line number, or line:column",
            Mode::Commands => "Run a command",
            Mode::Symbols => "Go to symbol in file",
        }
    }

    fn cycle(self, delta: isize) -> Mode {
        let ix = Mode::ALL.iter().position(|m| *m == self).unwrap_or(0) as isize;
        Mode::ALL[(ix + delta).rem_euclid(Mode::ALL.len() as isize) as usize]
    }
}

pub fn parse_query(raw: &str) -> (Mode, &str) {
    for mode in Mode::ALL {
        if !mode.prefix().is_empty() {
            if let Some(rest) = raw.strip_prefix(mode.prefix()) {
                return (mode, rest);
            }
        }
    }
    (Mode::Files, raw)
}

pub fn parse_line(query: &str) -> Option<(u32, u32)> {
    let query = query.trim();
    let (line, column) = match query.split_once([':', ',']) {
        Some((line, column)) => (line.trim(), Some(column.trim())),
        None => (query, None),
    };
    let line: u32 = line.parse().ok()?;
    let column: u32 = match column {
        Some("") | None => 1,
        Some(c) => c.parse().ok()?,
    };
    Some((line.saturating_sub(1), column.saturating_sub(1)))
}

pub enum QuickOpenEvent {
    Open(PathBuf),
    OpenAt { path: PathBuf, line: u32, column: u32 },
    Run(Box<dyn Action>),
    Dismiss,
}

struct TextHit {
    path: String,
    m: LineMatch,
}

struct ActiveFile {
    path: PathBuf,
    editor: Entity<EditorState>,
}

pub struct QuickOpen {
    input: Entity<InputState>,
    mode: Mode,
    index: Option<Arc<FileIndex>>,
    active: Option<ActiveFile>,
    hits: Vec<FuzzyHit>,
    text_hits: Vec<TextHit>,
    text_running: bool,
    text_error: Option<SharedString>,
    search: Option<SearchHandle>,
    search_task: Option<Task<()>>,
    symbols: Option<Arc<Vec<Symbol>>>,
    symbols_task: Option<Task<()>>,
    files_task: Option<Task<()>>,
    line_target: Option<(u32, u32)>,
    selected: usize,
    scroll: ScrollHandle,
    _subscription: Subscription,
}

impl EventEmitter<QuickOpenEvent> for QuickOpen {}

impl Focusable for QuickOpen {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.focus_handle(cx)
    }
}

impl QuickOpen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(Mode::Files.placeholder()));
        let subscription = cx.subscribe(&input, |this, _, event, cx| match event {
            InputEvent::Change => this.update_hits(cx),
            InputEvent::PressEnter { .. } => this.confirm(cx),
            _ => {}
        });
        QuickOpen {
            input,
            mode: Mode::Files,
            index: None,
            active: None,
            hits: Vec::new(),
            text_hits: Vec::new(),
            text_running: false,
            text_error: None,
            search: None,
            search_task: None,
            symbols: None,
            symbols_task: None,
            files_task: None,
            line_target: None,
            selected: 0,
            scroll: ScrollHandle::new(),
            _subscription: subscription,
        }
    }

    pub fn show(
        &mut self,
        index: Option<Arc<FileIndex>>,
        active: Option<(PathBuf, Entity<EditorState>)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.index = index;
        self.active = active.map(|(path, editor)| ActiveFile { path, editor });
        self.symbols = None;
        self.symbols_task = None;
        self.cancel_search();
        self.text_hits.clear();
        self.input.update(cx, |input, cx| {
            input.set_value("", window, cx);
            input.focus(window, cx);
        });
        self.update_hits(cx);
        self.sync_placeholder(window, cx);
    }

    fn update_hits(&mut self, cx: &mut Context<Self>) {
        let raw = self.input.read(cx).value();
        let (mode, query) = parse_query(&raw);
        if mode != Mode::Text {
            self.cancel_search();
        }
        self.mode = mode;
        self.hits.clear();
        self.line_target = None;
        match mode {
            Mode::Files => {
                self.files_task = None;
                if let Some(index) = self.index.clone() {
                    if query.trim().is_empty() || index.files.len() <= ASYNC_FUZZY_MIN_FILES {
                        self.hits = search::fuzzy(&index, query, MAX_RESULTS);
                    } else {
                        let query = query.to_string();
                        self.files_task = Some(cx.spawn(async move |this, cx| {
                            let hits = cx
                                .background_spawn(async move { search::fuzzy(&index, &query, MAX_RESULTS) })
                                .await;
                            let _ = this.update(cx, |this, cx| {
                                this.files_task = None;
                                if this.mode == Mode::Files {
                                    this.hits = hits;
                                    this.selected = 0;
                                    this.scroll.scroll_to_item(0);
                                    cx.notify();
                                }
                            });
                        }));
                    }
                }
            }
            Mode::Text => self.schedule_search(query.to_string(), cx),
            Mode::Line => self.line_target = parse_line(query),
            Mode::Commands => self.hits = search::fuzzy_list(commands::names(), query, COMMANDS.len()),
            Mode::Symbols => match self.symbols.clone() {
                Some(symbols) => {
                    self.hits = search::fuzzy_list(symbols.iter().map(|s| s.name.as_str()), query, usize::MAX)
                }
                None => self.scan_symbols(cx),
            },
        }
        self.selected = 0;
        self.scroll.scroll_to_item(0);
        cx.notify();
    }

    fn cancel_search(&mut self) {
        if let Some(handle) = self.search.take() {
            handle.cancel();
        }
        self.search_task = None;
        self.text_running = false;
    }

    fn schedule_search(&mut self, query: String, cx: &mut Context<Self>) {
        self.cancel_search();
        self.text_error = None;
        if query.trim().is_empty() {
            self.text_hits.clear();
            return;
        }
        self.search_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(TEXT_DEBOUNCE).await;
            let _ = this.update(cx, |this, cx| this.run_search(&query, cx));
        }));
    }

    fn run_search(&mut self, query: &str, cx: &mut Context<Self>) {
        self.text_hits.clear();
        self.selected = 0;
        let Some(index) = self.index.clone() else {
            cx.notify();
            return;
        };
        match search::search(index, query, SearchOptions::default()) {
            Ok((handle, rx)) => {
                self.search = Some(handle);
                self.text_running = true;
                self.search_task = Some(cx.spawn(async move |this, cx| {
                    while let Ok(batch) = rx.recv_async().await {
                        let mut batch = batch;
                        batch.extend(rx.drain().flatten());
                        let full = this
                            .update(cx, |this, cx| {
                                for file in batch {
                                    let room = MAX_TEXT_RESULTS.saturating_sub(this.text_hits.len());
                                    let path = file.path;
                                    this.text_hits.extend(
                                        file.matches.into_iter().take(room).map(|m| TextHit { path: path.clone(), m }),
                                    );
                                }
                                cx.notify();
                                this.text_hits.len() >= MAX_TEXT_RESULTS
                            })
                            .unwrap_or(true);
                        if full {
                            break;
                        }
                    }
                    let _ = this.update(cx, |this, cx| {
                        if let Some(handle) = this.search.take() {
                            handle.cancel();
                        }
                        this.text_running = false;
                        cx.notify();
                    });
                }));
            }
            Err(err) => self.text_error = Some(err.to_string().into()),
        }
        cx.notify();
    }

    fn scan_symbols(&mut self, cx: &mut Context<Self>) {
        if self.symbols_task.is_some() {
            return;
        }
        let Some(active) = self.active.as_ref() else {
            self.symbols = Some(Arc::new(Vec::new()));
            return;
        };
        let rope = active.editor.read(cx).text().clone();
        self.symbols_task = Some(cx.spawn(async move |this, cx| {
            let symbols = cx.background_spawn(async move { symbols::extract(&rope.to_string()) }).await;
            let _ = this.update(cx, |this, cx| {
                this.symbols = Some(Arc::new(symbols));
                this.symbols_task = None;
                if this.mode == Mode::Symbols {
                    this.update_hits(cx);
                }
            });
        }));
    }

    fn row_count(&self) -> usize {
        match self.mode {
            Mode::Text => self.text_hits.len(),
            Mode::Line => usize::from(self.line_target.is_some() && self.active.is_some()),
            _ => self.hits.len(),
        }
    }

    fn confirm(&mut self, cx: &mut Context<Self>) {
        let event = match self.mode {
            Mode::Files => {
                let (Some(index), Some(hit)) = (self.index.as_ref(), self.hits.get(self.selected)) else { return };
                QuickOpenEvent::Open(index.absolute(hit.ix))
            }
            Mode::Text => {
                let (Some(index), Some(hit)) = (self.index.as_ref(), self.text_hits.get(self.selected)) else { return };
                QuickOpenEvent::OpenAt {
                    path: index.root.join(&hit.path),
                    line: hit.m.line.saturating_sub(1) as u32,
                    column: hit.m.column as u32,
                }
            }
            Mode::Line => {
                let (Some(active), Some((line, column))) = (self.active.as_ref(), self.line_target) else { return };
                QuickOpenEvent::OpenAt { path: active.path.clone(), line, column }
            }
            Mode::Commands => {
                let Some(hit) = self.hits.get(self.selected) else { return };
                QuickOpenEvent::Run((COMMANDS[hit.ix].action)())
            }
            Mode::Symbols => {
                let (Some(active), Some(symbols), Some(hit)) =
                    (self.active.as_ref(), self.symbols.as_ref(), self.hits.get(self.selected))
                else {
                    return;
                };
                let symbol = &symbols[hit.ix];
                QuickOpenEvent::OpenAt { path: active.path.clone(), line: symbol.line, column: symbol.column }
            }
        };
        self.cancel_search();
        cx.emit(event);
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let len = self.row_count();
        if len == 0 {
            return;
        }
        self.selected = ((self.selected as isize + delta).rem_euclid(len as isize)) as usize;
        self.scroll.scroll_to_item(self.selected);
        cx.notify();
    }

    fn set_mode(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<Self>) {
        let raw = self.input.read(cx).value();
        let (_, query) = parse_query(&raw);
        let value = format!("{}{}", mode.prefix(), query);
        self.input.update(cx, |input, cx| {
            input.set_value(value, window, cx);
            input.focus(window, cx);
        });
        self.update_hits(cx);
        self.sync_placeholder(window, cx);
    }

    fn sync_placeholder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let placeholder = self.mode.placeholder();
        self.input.update(cx, |input, cx| input.set_placeholder(placeholder, window, cx));
    }
}

impl Render for QuickOpen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows: Vec<AnyElement> = match self.mode {
            Mode::Files => self.file_rows(cx),
            Mode::Text => self.text_rows(cx),
            Mode::Line => self.line_rows(cx),
            Mode::Commands => self.command_rows(cx),
            Mode::Symbols => self.symbol_rows(cx),
        };
        let empty = self.empty_message();
        div()
            .id("quick-open")
            .w(px(640.))
            .max_h(px(480.))
            .flex()
            .flex_col()
            .rounded(px(12.))
            .bg(Palette::toolbar_bg())
            .border_1()
            .border_color(Palette::hairline())
            .shadow_lg()
            .overflow_hidden()
            .capture_action(cx.listener(|this, _: &MoveUp, _, cx| {
                this.move_selection(-1, cx);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this, _: &MoveDown, _, cx| {
                this.move_selection(1, cx);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this, _: &IndentInline, window, cx| {
                this.set_mode(this.mode.cycle(1), window, cx);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this, _: &OutdentInline, window, cx| {
                this.set_mode(this.mode.cycle(-1), window, cx);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this, _: &Escape, _, cx| {
                this.cancel_search();
                cx.emit(QuickOpenEvent::Dismiss);
                cx.stop_propagation();
            }))
            .on_click(|_, _, cx| cx.stop_propagation())
            .child(self.render_tabs(cx))
            .child(
                div()
                    .p(px(10.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .border_b_1()
                    .border_color(Palette::border())
                    .child(crate::icons::ui_icon(IconName::Search, 16., Palette::muted()))
                    .child(div().flex_1().child(Input::new(&self.input).appearance(false))),
            )
            .child(
                div()
                    .id("quick-open-list")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .py(px(4.))
                    .when_some(empty, |el, message| {
                        el.child(div().p(px(12.)).text_size(px(12.)).text_color(Palette::faint()).child(message))
                    })
                    .children(rows),
            )
            .child(self.render_footer())
    }
}

impl QuickOpen {
    fn render_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(10.))
            .pt(px(8.))
            .flex()
            .gap(px(2.))
            .children(Mode::ALL.into_iter().enumerate().map(|(i, mode)| {
                let on = mode == self.mode;
                div()
                    .id(("quick-open-tab", i))
                    .px(px(9.))
                    .h(px(22.))
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .rounded(px(5.))
                    .text_size(px(11.5))
                    .font_weight(FontWeight::MEDIUM)
                    .cursor_pointer()
                    .when(on, |el| el.bg(Palette::chip_active()).text_color(gpui_kit::white()))
                    .when(!on, |el| el.text_color(Palette::muted()).hover(|s| s.bg(ca(0xffffff, 0.08))))
                    .child(mode.label())
                    .when(!mode.prefix().is_empty(), |el| {
                        el.child(div().text_size(px(10.5)).text_color(Palette::faint()).child(mode.prefix()))
                    })
                    .on_click(cx.listener(move |this, _, window, cx| this.set_mode(mode, window, cx)))
            }))
    }

    fn render_footer(&self) -> impl IntoElement {
        let status = match self.mode {
            Mode::Text if self.text_running => format!("Searching… {}", self.text_hits.len()),
            Mode::Text if self.text_hits.len() >= MAX_TEXT_RESULTS => format!("First {MAX_TEXT_RESULTS} matches"),
            Mode::Text if !self.text_hits.is_empty() => format!("{} matches", self.text_hits.len()),
            _ => String::new(),
        };
        div()
            .px(px(12.))
            .h(px(26.))
            .flex()
            .items_center()
            .gap(px(12.))
            .border_t_1()
            .border_color(Palette::border())
            .text_size(px(11.))
            .text_color(Palette::faint())
            .child("# text   : line   > commands   @ symbols   ⇥ switch")
            .child(div().flex_1())
            .child(status)
    }

    fn empty_message(&self) -> Option<&'static str> {
        if self.row_count() > 0 {
            return None;
        }
        Some(match self.mode {
            Mode::Files | Mode::Text if self.index.is_none() => "Indexing…",
            Mode::Files => "No matching files",
            Mode::Text if self.text_error.is_some() => "Invalid search",
            Mode::Text if self.text_running || self.search_task.is_some() => "Searching…",
            Mode::Text => "Type to search file contents",
            Mode::Line if self.active.is_none() => "No file is open",
            Mode::Line => "Type a line number, e.g. 120 or 120:5",
            Mode::Commands => "No matching commands",
            Mode::Symbols if self.active.is_none() => "No file is open",
            Mode::Symbols if self.symbols.is_none() => "Scanning…",
            Mode::Symbols => "No matching symbols",
        })
    }

    fn row(&self, row: usize, cx: &mut Context<Self>) -> gpui_kit::Stateful<gpui_kit::Div> {
        let selected = row == self.selected;
        div()
            .id(("hit", row))
            .mx(px(6.))
            .px(px(8.))
            .flex()
            .items_center()
            .gap(px(8.))
            .rounded(px(6.))
            .cursor_pointer()
            .when(selected, |el| el.bg(Palette::row_selected()))
            .when(!selected, |el| el.hover(|s| s.bg(Palette::row_hover())))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.selected = row;
                this.confirm(cx);
            }))
    }

    fn file_rows(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(index) = self.index.clone() else { return Vec::new() };
        self.hits
            .iter()
            .enumerate()
            .map(|(row, hit)| {
                let path = &index.files[hit.ix];
                let name_start = path.rfind('/').map_or(0, |i| i + 1);
                let name_char_start = path[..name_start].chars().count() as u32;
                let name = &path[name_start..];
                let dir = &path[..name_start.saturating_sub(1)];
                let highlights = char_highlights(name, hit.indices.iter().filter_map(|&i| i.checked_sub(name_char_start)));
                self.row(row, cx)
                    .h(px(30.))
                    .child(file_icons::file_icon_img(Path::new(path)))
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(Palette::text())
                            .whitespace_nowrap()
                            .child(StyledText::new(SharedString::from(name.to_string())).with_highlights(highlights)),
                    )
                    .child(faint_ellipsis(dir.to_string()))
                    .into_any_element()
            })
            .collect()
    }

    fn text_rows(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        self.text_hits
            .iter()
            .enumerate()
            .map(|(row, hit)| {
                let highlights: Vec<(Range<usize>, HighlightStyle)> = hit
                    .m
                    .ranges
                    .iter()
                    .map(|&(start, end)| {
                        (start..end, HighlightStyle {
                            color: Some(gpui_kit::white()),
                            background_color: Some(Palette::match_bg()),
                            font_weight: Some(FontWeight::SEMIBOLD),
                            ..Default::default()
                        })
                    })
                    .collect();
                let snippet = StyledText::new(SharedString::from(hit.m.text.clone())).with_highlights(highlights);
                self.row(row, cx)
                    .h(px(40.))
                    .child(file_icons::file_icon_img(Path::new(&hit.path)))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_color(Palette::text())
                                    .font_family(crate::theme::mono_font())
                                    .text_size(px(12.))
                                    .child(snippet),
                            )
                            .child(faint_ellipsis(format!("{}:{}", hit.path, hit.m.line))),
                    )
                    .into_any_element()
            })
            .collect()
    }

    fn line_rows(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let (Some(active), Some((line, column))) = (self.active.as_ref(), self.line_target) else { return Vec::new() };
        let name = active.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        vec![
            self.row(0, cx)
                .h(px(30.))
                .child(file_icons::file_icon_img(&active.path))
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(Palette::text())
                        .child(format!("Go to line {}, column {}", line + 1, column + 1)),
                )
                .child(faint_ellipsis(name))
                .into_any_element(),
        ]
    }

    fn command_rows(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        self.hits
            .iter()
            .enumerate()
            .map(|(row, hit)| {
                let command = &COMMANDS[hit.ix];
                self.row(row, cx)
                    .h(px(30.))
                    .child(crate::icons::ui_icon(IconName::ChevronRight, 14., Palette::muted()))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(13.))
                            .text_color(Palette::text())
                            .whitespace_nowrap()
                            .child(
                                StyledText::new(command.name)
                                    .with_highlights(char_highlights(command.name, hit.indices.iter().copied())),
                            ),
                    )
                    .child(div().text_size(px(11.5)).text_color(Palette::faint()).child(command.keys))
                    .into_any_element()
            })
            .collect()
    }

    fn symbol_rows(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(symbols) = self.symbols.clone() else { return Vec::new() };
        self.hits
            .iter()
            .enumerate()
            .map(|(row, hit)| {
                let symbol = &symbols[hit.ix];
                self.row(row, cx)
                    .h(px(28.))
                    .child(
                        div()
                            .w(px(64.))
                            .flex_none()
                            .text_size(px(10.5))
                            .font_family(crate::theme::mono_font())
                            .text_color(Palette::accent())
                            .child(symbol.kind),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_size(px(13.))
                            .text_color(Palette::text())
                            .child(
                                StyledText::new(SharedString::from(symbol.name.clone()))
                                    .with_highlights(char_highlights(&symbol.name, hit.indices.iter().copied())),
                            ),
                    )
                    .child(div().text_size(px(11.)).text_color(Palette::faint()).child((symbol.line + 1).to_string()))
                    .into_any_element()
            })
            .collect()
    }
}

fn faint_ellipsis(text: String) -> impl IntoElement {
    div()
        .flex_1()
        .min_w(px(0.))
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .text_size(px(11.5))
        .text_color(Palette::faint())
        .child(text)
}

fn char_highlights(text: &str, chars: impl Iterator<Item = u32>) -> Vec<(Range<usize>, HighlightStyle)> {
    let offsets: Vec<(usize, usize)> = text.char_indices().map(|(i, c)| (i, i + c.len_utf8())).collect();
    let style = HighlightStyle {
        color: Some(Palette::accent()),
        font_weight: Some(FontWeight::BOLD),
        background_color: Some(ca(0x0a84ff, 0.0)),
        ..Default::default()
    };
    chars.filter_map(|c| offsets.get(c as usize).map(|&(s, e)| (s..e, style))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixes_pick_modes() {
        assert_eq!(parse_query("main.rs"), (Mode::Files, "main.rs"));
        assert_eq!(parse_query(""), (Mode::Files, ""));
        assert_eq!(parse_query("#todo fix"), (Mode::Text, "todo fix"));
        assert_eq!(parse_query(":12"), (Mode::Line, "12"));
        assert_eq!(parse_query(">toggle"), (Mode::Commands, "toggle"));
        assert_eq!(parse_query("@render"), (Mode::Symbols, "render"));
        assert_eq!(parse_query("a#b"), (Mode::Files, "a#b"));
    }

    #[test]
    fn mode_cycle_wraps() {
        assert_eq!(Mode::Files.cycle(1), Mode::Text);
        assert_eq!(Mode::Symbols.cycle(1), Mode::Files);
        assert_eq!(Mode::Files.cycle(-1), Mode::Symbols);
        for mode in Mode::ALL {
            assert_eq!(parse_query(&format!("{}x", mode.prefix())), (mode, "x"));
        }
    }

    #[test]
    fn line_numbers_are_one_based_input() {
        assert_eq!(parse_line("120"), Some((119, 0)));
        assert_eq!(parse_line(" 120:5 "), Some((119, 4)));
        assert_eq!(parse_line("7,3"), Some((6, 2)));
        assert_eq!(parse_line("7:"), Some((6, 0)));
        assert_eq!(parse_line("0"), Some((0, 0)));
        assert_eq!(parse_line(""), None);
        assert_eq!(parse_line("abc"), None);
        assert_eq!(parse_line("12:x"), None);
    }
}
