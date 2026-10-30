use std::sync::Arc;

use gpui_kit::component::highlighter::HighlightTheme;
use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{App, Hsla, px, rgb};

pub fn c(hex: u32) -> Hsla {
    rgb(hex).into()
}

pub fn ca(hex: u32, alpha: f32) -> Hsla {
    let mut color = c(hex);
    color.a = alpha;
    color
}

pub struct Palette;

impl Palette {
    pub fn editor_bg() -> Hsla { c(0x1f1f24) }
    pub fn toolbar_bg() -> Hsla { c(0x2b2b30) }
    pub fn sidebar_bg() -> Hsla { c(0x26262b) }
    pub fn panel_bg() -> Hsla { c(0x1b1b1f) }
    pub fn border() -> Hsla { c(0x0f0f12) }
    pub fn hairline() -> Hsla { c(0x38383d) }
    pub fn text() -> Hsla { c(0xdfdfe0) }
    pub fn muted() -> Hsla { c(0x8e8e93) }
    pub fn faint() -> Hsla { c(0x5c5c61) }
    pub fn accent() -> Hsla { c(0x0a84ff) }
    pub fn accent_hover() -> Hsla { c(0x2a95ff) }
    pub fn chip_active() -> Hsla { ca(0x0a84ff, 0.3) }
    pub fn match_bg() -> Hsla { ca(0xd9a54a, 0.45) }
    pub fn drop_target() -> Hsla { ca(0x0a84ff, 0.08) }
    pub fn row_selected() -> Hsla { ca(0x0a84ff, 0.32) }
    pub fn row_hover() -> Hsla { ca(0xffffff, 0.05) }
    pub fn tab_active() -> Hsla { c(0x1f1f24) }
    pub fn tab_inactive() -> Hsla { c(0x26262b) }
    pub fn git_modified() -> Hsla { c(0xd9a54a) }
    pub fn git_added() -> Hsla { c(0x5fc27a) }
    pub fn git_deleted() -> Hsla { c(0xff6b6b) }
    pub fn git_untracked() -> Hsla { c(0x6aa9ff) }
    pub fn folder() -> Hsla { c(0x6aa9ff) }
}

pub const MONO_FONT: &str = "Menlo";
pub const UI_FONT_SIZE: f32 = 13.;
pub const MONO_FONT_SIZE: f32 = 13.;

const XCODE_DARK: &str = include_str!("../assets/themes/xcode-dark.json");
const XCODE_LIGHT: &str = include_str!("../assets/themes/xcode-light.json");

pub fn highlight_theme(mode: ThemeMode) -> Arc<HighlightTheme> {
    let source = if mode.is_dark() { XCODE_DARK } else { XCODE_LIGHT };
    Arc::new(serde_json::from_str(source).expect("bundled Xcode theme is valid"))
}

pub fn apply(cx: &mut App) {
    Theme::change(ThemeMode::Dark, None, cx);
    let theme = Theme::global_mut(cx);
    theme.mono_font_family = MONO_FONT.into();
    theme.mono_font_size = px(MONO_FONT_SIZE);
    theme.font_size = px(UI_FONT_SIZE);
    theme.background = Palette::editor_bg();
    theme.foreground = Palette::text();
    theme.border = Palette::hairline();
    theme.input = Palette::hairline();
    theme.muted_foreground = Palette::muted();
    theme.primary = Palette::accent();
    theme.primary_hover = Palette::accent_hover();
    theme.primary_active = c(0x0070e0);
    theme.primary_foreground = c(0xffffff);
    theme.highlight_theme = highlight_theme(ThemeMode::Dark);
    theme.selection = c(0x515b70);
    theme.caret = c(0xffffff);
    theme.popover = c(0x2b2b30);
    theme.list_active = Palette::row_selected();
    theme.list_hover = Palette::row_hover();
    theme.scrollbar = gpui_kit::transparent_black();
    theme.scrollbar_thumb = ca(0xffffff, 0.18);
    theme.scrollbar_thumb_hover = ca(0xffffff, 0.3);
    theme.title_bar = Palette::toolbar_bg();
    theme.title_bar_border = Palette::border();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_xcode_themes_parse() {
        let dark = highlight_theme(ThemeMode::Dark);
        let light = highlight_theme(ThemeMode::Light);
        assert!(dark.name.contains("Dark") && light.name.contains("Light"));
        assert_ne!(dark.style.editor_background, light.style.editor_background);
        assert_eq!(dark.style.editor_background, Some(c(0x1f1f24)));
        assert_eq!(dark.style.syntax.style("keyword").and_then(|s| s.color), Some(c(0xfc5fa3)));
        assert_eq!(light.style.syntax.style("string").and_then(|s| s.color), Some(c(0xc41a16)));
    }
}
