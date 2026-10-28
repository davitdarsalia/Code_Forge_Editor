//! ⌘P: fuzzy file finder over the project index.

use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::component::input::{Escape, Input, InputEvent, InputState, MoveDown, MoveUp};
use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight,
    HighlightStyle, InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, StyledText, Subscription, Window, div,
    prelude::FluentBuilder as _, px,
};

use crate::search::{self, FileIndex, FuzzyHit};
use crate::theme::{Palette, ca};

const MAX_RESULTS: usize = 60;

pub enum QuickOpenEvent {
    Open(PathBuf),
    Dismiss,
}

pub struct QuickOpen {
    input: Entity<InputState>,
    index: Option<Arc<FileIndex>>,
    hits: Vec<FuzzyHit>,
    selected: usize,
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
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Open file by name"));
        let subscription = cx.subscribe(&input, |this, _, event, cx| match event {
            InputEvent::Change => this.update_hits(cx),
            InputEvent::PressEnter { .. } => this.confirm(cx),
            _ => {}
        });
        QuickOpen { input, index: None, hits: Vec::new(), selected: 0, _subscription: subscription }
    }

    pub fn show(&mut self, index: Option<Arc<FileIndex>>, window: &mut Window, cx: &mut Context<Self>) {
        self.index = index;
        self.input.update(cx, |input, cx| {
            input.set_value("", window, cx);
            input.focus(window, cx);
        });
        self.update_hits(cx);
    }

    fn update_hits(&mut self, cx: &mut Context<Self>) {
        let query = self.input.read(cx).value();
        self.hits = match self.index.as_ref() {
            Some(index) => search::fuzzy(index, &query, MAX_RESULTS),
            None => Vec::new(),
        };
        self.selected = 0;
        cx.notify();
    }

    fn confirm(&mut self, cx: &mut Context<Self>) {
        let (Some(index), Some(hit)) = (self.index.as_ref(), self.hits.get(self.selected)) else { return };
        cx.emit(QuickOpenEvent::Open(index.absolute(hit.ix)));
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.hits.is_empty() {
            return;
        }
        let len = self.hits.len() as isize;
        self.selected = ((self.selected as isize + delta).rem_euclid(len)) as usize;
        cx.notify();
    }
}

impl Render for QuickOpen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let index = self.index.clone();
        div()
            .id("quick-open")
            .w(px(620.))
            .max_h(px(460.))
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
            .capture_action(cx.listener(|_, _: &Escape, _, cx| {
                cx.emit(QuickOpenEvent::Dismiss);
                cx.stop_propagation();
            }))
            .on_click(|_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .p(px(10.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .border_b_1()
                    .border_color(Palette::border())
                    .child(Icon::new(IconName::Search).size(px(16.)).text_color(Palette::muted()))
                    .child(div().flex_1().child(Input::new(&self.input).appearance(false))),
            )
            .child(
                div()
                    .id("quick-open-list")
                    .flex_1()
                    .overflow_y_scroll()
                    .py(px(4.))
                    .when(index.is_none(), |el| {
                        el.child(div().p(px(12.)).text_size(px(12.)).text_color(Palette::faint()).child("Indexing…"))
                    })
                    .children(self.hits.iter().enumerate().filter_map(|(row, hit)| {
                        let index = index.as_ref()?;
                        let path = &index.files[hit.ix];
                        let name_start = path.rfind('/').map_or(0, |i| i + 1);
                        let name_char_start = path[..name_start].chars().count() as u32;
                        let name = &path[name_start..];
                        let dir = &path[..name_start.saturating_sub(1)];
                        let highlights = char_highlights(name, hit.indices.iter().filter_map(|&i| i.checked_sub(name_char_start)));
                        let selected = row == self.selected;
                        Some(
                            div()
                                .id(("hit", row))
                                .mx(px(6.))
                                .px(px(8.))
                                .h(px(30.))
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .rounded(px(6.))
                                .cursor_pointer()
                                .when(selected, |el| el.bg(Palette::row_selected()))
                                .when(!selected, |el| el.hover(|s| s.bg(Palette::row_hover())))
                                .child(Icon::new(IconName::FileCode).size(px(14.)).text_color(Palette::muted()))
                                .child(
                                    div()
                                        .text_size(px(13.))
                                        .text_color(Palette::text())
                                        .whitespace_nowrap()
                                        .child(StyledText::new(SharedString::from(name.to_string())).with_highlights(highlights)),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .text_size(px(11.5))
                                        .text_color(Palette::faint())
                                        .child(dir.to_string()),
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.selected = row;
                                    this.confirm(cx);
                                })),
                        )
                    })),
            )
    }
}

/// Bold accent highlight for the matched characters of `text`.
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
