//! Find-in-project panel (⇧⌘F). Results stream in while the search runs and
//! render through a virtual list, so huge result sets stay cheap.

use std::collections::HashSet;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable,
    FontWeight, HighlightStyle, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, StyledText, Subscription, Task,
    Window, div, prelude::FluentBuilder as _, px, uniform_list,
};

use crate::file_icons;
use crate::search::{self, FileIndex, FileMatches, SearchHandle, SearchOptions};
use crate::theme::{Palette, ca};

const SEARCH_DEBOUNCE: Duration = Duration::from_millis(120);
const ROW_HEIGHT: f32 = 22.;

pub enum SearchEvent {
    Open { path: PathBuf, line: u32, column: u32 },
}

#[derive(Clone, Copy)]
enum RowRef {
    File(usize),
    Match(usize, usize),
}

pub struct SearchPanel {
    input: Entity<InputState>,
    options: SearchOptions,
    index: Option<Arc<FileIndex>>,
    results: Vec<FileMatches>,
    rows: Rc<Vec<RowRef>>,
    collapsed: HashSet<usize>,
    total: usize,
    running: bool,
    error: Option<SharedString>,
    handle: Option<SearchHandle>,
    task: Option<Task<()>>,
    _subscription: Subscription,
}

impl EventEmitter<SearchEvent> for SearchPanel {}

impl Focusable for SearchPanel {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.focus_handle(cx)
    }
}

impl SearchPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Find in project"));
        let subscription = cx.subscribe(&input, |this, _, event, cx| match event {
            InputEvent::Change => this.schedule(cx),
            InputEvent::PressEnter { .. } => this.run(cx),
            _ => {}
        });
        SearchPanel {
            input,
            options: SearchOptions::default(),
            index: None,
            results: Vec::new(),
            rows: Rc::new(Vec::new()),
            collapsed: HashSet::new(),
            total: 0,
            running: false,
            error: None,
            handle: None,
            task: None,
            _subscription: subscription,
        }
    }

    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| {
            input.focus(window, cx);
            input.select_all(window, cx);
        });
    }

    pub fn set_index(&mut self, index: Option<Arc<FileIndex>>, cx: &mut Context<Self>) {
        let changed_root = self.index.as_ref().map(|i| &i.root) != index.as_ref().map(|i| &i.root);
        self.index = index;
        if changed_root {
            self.cancel();
            self.results.clear();
            self.rebuild_rows();
            self.total = 0;
        }
        cx.notify();
    }

    fn schedule(&mut self, cx: &mut Context<Self>) {
        self.task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SEARCH_DEBOUNCE).await;
            let _ = this.update(cx, |this, cx| this.run(cx));
        }));
    }

    fn cancel(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.cancel();
        }
        self.task = None;
        self.running = false;
    }

    fn run(&mut self, cx: &mut Context<Self>) {
        self.cancel();
        self.results.clear();
        self.collapsed.clear();
        self.total = 0;
        self.error = None;
        let query = self.input.read(cx).value().to_string();
        let Some(index) = self.index.clone() else {
            self.rebuild_rows();
            cx.notify();
            return;
        };
        if query.is_empty() {
            self.rebuild_rows();
            cx.notify();
            return;
        }
        match search::search(index, &query, self.options) {
            Ok((handle, rx)) => {
                self.handle = Some(handle);
                self.running = true;
                self.task = Some(cx.spawn(async move |this, cx| {
                    while let Ok(batch) = rx.recv_async().await {
                        // Drain whatever else is queued so one frame takes many batches.
                        let mut batch = batch;
                        batch.extend(rx.drain().flatten());
                        if this.update(cx, |this, cx| this.append(batch, cx)).is_err() {
                            return;
                        }
                    }
                    let _ = this.update(cx, |this, cx| {
                        this.running = false;
                        this.results.sort_by(|a, b| a.path.cmp(&b.path));
                        this.rebuild_rows();
                        cx.notify();
                    });
                }));
            }
            Err(err) => {
                self.error = Some(err.to_string().into());
                self.rebuild_rows();
            }
        }
        cx.notify();
    }

    fn append(&mut self, batch: Vec<FileMatches>, cx: &mut Context<Self>) {
        for file in batch {
            self.total += file.matches.len();
            self.results.push(file);
        }
        self.rebuild_rows();
        cx.notify();
    }

    fn rebuild_rows(&mut self) {
        let mut rows = Vec::with_capacity(self.total + self.results.len());
        for (fi, file) in self.results.iter().enumerate() {
            rows.push(RowRef::File(fi));
            if !self.collapsed.contains(&fi) {
                rows.extend((0..file.matches.len()).map(|mi| RowRef::Match(fi, mi)));
            }
        }
        self.rows = Rc::new(rows);
    }

    fn toggle_option(&mut self, f: impl FnOnce(&mut SearchOptions), cx: &mut Context<Self>) {
        f(&mut self.options);
        self.run(cx);
    }

    fn click(&mut self, row: RowRef, cx: &mut Context<Self>) {
        match row {
            RowRef::File(fi) => {
                if !self.collapsed.remove(&fi) {
                    self.collapsed.insert(fi);
                }
                self.rebuild_rows();
                cx.notify();
            }
            RowRef::Match(fi, mi) => {
                let (Some(index), Some(file)) = (self.index.as_ref(), self.results.get(fi)) else { return };
                let m = &file.matches[mi];
                cx.emit(SearchEvent::Open {
                    path: index.root.join(&file.path),
                    line: m.line.saturating_sub(1) as u32,
                    column: m.column as u32,
                });
            }
        }
    }

    fn render_row(&self, ix: usize, row: RowRef, cx: &mut Context<Self>) -> AnyElement {
        let base = div()
            .id(("search-row", ix))
            .w_full()
            .h(px(crate::icons::row_height(ROW_HEIGHT, crate::file_icons::ICON_SIZE)))
            .px(px(6.))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| this.click(row, cx)));
        let inner = div()
            .size_full()
            .flex()
            .items_center()
            .gap(px(5.))
            .rounded(px(5.))
            .hover(|s| s.bg(Palette::row_hover()))
            .text_size(px(12.));
        match row {
            RowRef::File(fi) => {
                let file = &self.results[fi];
                let collapsed = self.collapsed.contains(&fi);
                let (dir, name) = match file.path.rsplit_once('/') {
                    Some((dir, name)) => (dir.to_string(), name.to_string()),
                    None => (String::new(), file.path.clone()),
                };
                base.child(
                    inner
                        .pl(px(4.))
                        .child(
                            Icon::new(if collapsed { IconName::ChevronRight } else { IconName::ChevronDown })
                                .size(crate::icons::scaled(11.))
                                .text_color(Palette::faint()),
                        )
                        .child(file_icons::file_icon_img(Path::new(&name)))
                        .child(div().font_weight(FontWeight::MEDIUM).text_color(Palette::text()).whitespace_nowrap().child(name))
                        .child(
                            div()
                                .flex_1()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_size(px(11.))
                                .text_color(Palette::faint())
                                .child(dir),
                        )
                        .child(
                            div()
                                .px(px(5.))
                                .rounded_full()
                                .bg(ca(0xffffff, 0.08))
                                .text_size(px(10.))
                                .text_color(Palette::muted())
                                .child(file.matches.len().to_string()),
                        ),
                )
                .into_any_element()
            }
            RowRef::Match(fi, mi) => {
                let m = &self.results[fi].matches[mi];
                let highlights: Vec<(Range<usize>, HighlightStyle)> = m
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
                let text = StyledText::new(SharedString::from(m.text.clone())).with_highlights(highlights);
                base.child(
                    inner
                        .pl(px(30.))
                        .child(
                            div()
                                .w(px(30.))
                                .flex_none()
                                .text_size(px(10.5))
                                .text_color(Palette::faint())
                                .child(m.line.to_string()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_color(Palette::muted())
                                .font_family(crate::theme::MONO_FONT)
                                .text_size(px(11.5))
                                .child(text),
                        ),
                )
                .into_any_element()
            }
        }
    }
}

fn option_toggle(
    id: &'static str,
    label: &'static str,
    on: bool,
    cx: &mut Context<SearchPanel>,
    f: fn(&mut SearchOptions),
) -> impl IntoElement {
    div()
        .id(id)
        .px(px(6.))
        .h(px(20.))
        .flex()
        .items_center()
        .rounded(px(4.))
        .text_size(px(11.))
        .font_weight(FontWeight::SEMIBOLD)
        .cursor_pointer()
        .when(on, |el| el.bg(Palette::chip_active()).text_color(gpui_kit::white()))
        .when(!on, |el| el.text_color(Palette::muted()).hover(|s| s.bg(ca(0xffffff, 0.08))))
        .child(label)
        .on_click(cx.listener(move |this, _, _, cx| this.toggle_option(f, cx)))
}

impl Render for SearchPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let summary: SharedString = if let Some(err) = &self.error {
            err.clone()
        } else if self.index.is_none() {
            "Indexing…".into()
        } else if self.running {
            format!("Searching… {} results", self.total).into()
        } else if self.input.read(cx).value().is_empty() {
            "".into()
        } else {
            let capped = if self.total >= search::MAX_MATCHES { "+" } else { "" };
            format!("{}{} results in {} files", self.total, capped, self.results.len()).into()
        };
        let rows = self.rows.clone();
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .p(px(10.))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .border_b_1()
                    .border_color(Palette::border())
                    .child(Input::new(&self.input))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .child(option_toggle("opt-case", "Aa", self.options.case_sensitive, cx, |o| {
                                o.case_sensitive = !o.case_sensitive
                            }))
                            .child(option_toggle("opt-word", "W", self.options.whole_word, cx, |o| {
                                o.whole_word = !o.whole_word
                            }))
                            .child(option_toggle("opt-regex", ".*", self.options.regex, cx, |o| o.regex = !o.regex))
                            .child(div().flex_1())
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .text_color(if self.error.is_some() { Palette::git_deleted() } else { Palette::faint() })
                                    .child(summary),
                            ),
                    ),
            )
            .child(
                uniform_list("search-results", rows.len(), cx.processor(move |this, range: Range<usize>, _, cx| {
                    range.map(|ix| this.render_row(ix, rows[ix], cx)).collect::<Vec<_>>()
                }))
                .flex_1()
                .py(px(4.)),
            )
    }
}
