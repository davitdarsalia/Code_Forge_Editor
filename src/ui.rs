use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::{
    App, ClickEvent, FontWeight, Hsla, InteractiveElement as _, IntoElement, MouseButton, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, prelude::FluentBuilder as _, px,
};

use crate::theme::{Palette, ca};

pub fn icon(name: IconName, size: f32, color: Hsla) -> Icon {
    crate::icons::ui_icon(name, size, color)
}

pub fn tool_button(
    id: &'static str,
    name: IconName,
    active: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .size(px(28.))
        .rounded(px(6.))
        .cursor_pointer()
        .hover(|s| s.bg(ca(0xffffff, 0.08)))
        .active(|s| s.bg(ca(0xffffff, 0.14)))
        .child(icon(name, 16., if active { Palette::accent() } else { Palette::muted() }))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(on_click)
}

pub fn text_button(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    primary: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let id: SharedString = id.into();
    div()
        .id(gpui_kit::ElementId::Name(id))
        .px(px(12.))
        .h(px(26.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .text_size(px(12.5))
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer()
        .when(primary, |el| {
            el.bg(Palette::accent())
                .text_color(gpui_kit::white())
                .hover(|s| s.bg(Palette::accent_hover()))
        })
        .when(!primary, |el| {
            el.bg(ca(0xffffff, 0.08))
                .text_color(Palette::text())
                .hover(|s| s.bg(ca(0xffffff, 0.13)))
        })
        .child(label.into())
        .on_click(on_click)
}

pub fn section_header(title: impl Into<SharedString>) -> impl IntoElement {
    div()
        .px(px(12.))
        .pt(px(12.))
        .pb(px(6.))
        .text_size(px(11.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(Palette::muted())
        .child(title.into())
}
