use std::path::Path;
use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::base::input::DiagnosticSeverity;
use gpui_kit::{
    AnyElement, App, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, Window, div, prelude::FluentBuilder as _, px, uniform_list,
};

use crate::diagnostics::{self, Row, Store};
use crate::file_icons;
use crate::theme::Palette;

const ROW_HEIGHT: f32 = 24.;

pub type OpenHandler = Rc<dyn Fn(&Path, u32, u32, &mut Window, &mut App)>;

pub fn severity_icon(severity: DiagnosticSeverity) -> IconName {
    match severity {
        DiagnosticSeverity::Error => IconName::CircleX,
        DiagnosticSeverity::Warning => IconName::TriangleAlert,
        DiagnosticSeverity::Info | DiagnosticSeverity::Hint => IconName::Info,
    }
}

pub fn counts_badge(errors: usize, warnings: usize) -> AnyElement {
    let part = |name: IconName, count: usize, severity: DiagnosticSeverity| {
        let color = if count > 0 { diagnostics::foreground(severity) } else { Palette::faint() };
        div()
            .flex()
            .items_center()
            .gap(px(3.))
            .child(crate::icons::ui_icon(name, 11., color))
            .child(div().text_color(if count > 0 { Palette::text() } else { Palette::faint() }).child(count.to_string()))
    };
    div()
        .flex()
        .items_center()
        .gap(px(7.))
        .text_size(px(11.))
        .child(part(IconName::CircleX, errors, DiagnosticSeverity::Error))
        .child(part(IconName::TriangleAlert, warnings, DiagnosticSeverity::Warning))
        .into_any_element()
}

fn row_height() -> f32 {
    crate::icons::row_height(ROW_HEIGHT, file_icons::ICON_SIZE)
}

fn render_row(ix: usize, row: &Row, on_open: &OpenHandler) -> AnyElement {
    match row {
        Row::File { path, name, dir, errors, warnings } => div()
            .id(("lsp-issue-file", ix))
            .h(px(row_height()))
            .px(px(10.))
            .flex()
            .items_center()
            .gap(px(6.))
            .text_size(px(12.))
            .child(file_icons::file_icon_img(path))
            .child(div().flex_none().font_weight(FontWeight::SEMIBOLD).text_color(Palette::text()).child(name.clone()))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_color(Palette::muted())
                    .child(dir.clone()),
            )
            .child(counts_badge(*errors, *warnings))
            .into_any_element(),
        Row::Issue { path, item } => {
            let (line, column) = (item.start.line, item.start.character);
            let path = path.clone();
            let on_open = on_open.clone();
            div()
                .id(("lsp-issue", ix))
                .h(px(row_height()))
                .pl(px(30.))
                .pr(px(10.))
                .flex()
                .items_center()
                .gap(px(6.))
                .text_size(px(12.))
                .cursor_pointer()
                .hover(|s| s.bg(Palette::row_hover()))
                .on_click(move |_, window, cx| on_open(&path, line, column, window, cx))
                .child(crate::icons::ui_icon(severity_icon(item.severity), 13., diagnostics::foreground(item.severity)))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_color(Palette::text())
                        .child(item.headline()),
                )
                .when_some(item.source.clone(), |el, source| {
                    el.child(div().flex_none().text_color(Palette::faint()).child(source))
                })
                .child(div().flex_none().text_color(Palette::muted()).child(format!("{}:{}", line + 1, column + 1)))
                .into_any_element()
        }
    }
}

pub fn render(store: &Store, root: Option<&Path>, on_open: OpenHandler) -> AnyElement {
    if store.is_empty() {
        return div()
            .p(px(12.))
            .text_size(px(12.))
            .text_color(Palette::muted())
            .child("No issues")
            .into_any_element();
    }
    let rows = store.rows(root);
    uniform_list("lsp-issues", rows.len(), move |range, _window, _cx| {
        range.filter_map(|ix| Some(render_row(ix, rows.get(ix)?, &on_open))).collect::<Vec<_>>()
    })
    .size_full()
    .into_any_element()
}
