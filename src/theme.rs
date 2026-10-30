use std::sync::{Arc, OnceLock};

use gpui_kit::component::highlighter::HighlightTheme;
use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::gpui::TextSystem;
use gpui_kit::{App, Hsla, font, px, rgb};

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

pub const MONO_FONT: &str = ".AppleSystemUIFontMonospaced";
pub const MONO_FALLBACK_FONT: &str = "Menlo";
pub const UI_FONT: &str = ".SystemUIFont";
pub const UI_FONT_SIZE: f32 = 13.;
pub const MONO_FONT_SIZE: f32 = 12.;
pub const EDITOR_LINE_HEIGHT: f32 = 1.25;

static RESOLVED_MONO: OnceLock<&'static str> = OnceLock::new();

pub fn mono_font() -> &'static str {
    RESOLVED_MONO.get().copied().unwrap_or(MONO_FONT)
}

pub fn is_monospace(text_system: &TextSystem, family: &str) -> bool {
    let id = text_system.resolve_font(&font(family.to_string()));
    let size = px(MONO_FONT_SIZE);
    match (text_system.advance(id, size, 'i'), text_system.advance(id, size, 'm')) {
        (Ok(narrow), Ok(wide)) => narrow.width > px(0.) && narrow.width == wide.width,
        _ => false,
    }
}

fn resolve_mono_font(cx: &App) -> &'static str {
    *RESOLVED_MONO.get_or_init(|| {
        if is_monospace(cx.text_system(), MONO_FONT) { MONO_FONT } else { MONO_FALLBACK_FONT }
    })
}

const XCODE_DARK: &str = include_str!("../assets/themes/xcode-dark.json");
const XCODE_LIGHT: &str = include_str!("../assets/themes/xcode-light.json");

pub fn highlight_theme(mode: ThemeMode) -> Arc<HighlightTheme> {
    let source = if mode.is_dark() { XCODE_DARK } else { XCODE_LIGHT };
    Arc::new(serde_json::from_str(source).expect("bundled Xcode theme is valid"))
}

pub fn apply(cx: &mut App) {
    Theme::change(ThemeMode::Dark, None, cx);
    let mono = resolve_mono_font(cx);
    let theme = Theme::global_mut(cx);
    theme.font_family = UI_FONT.into();
    theme.mono_font_family = mono.into();
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

    fn system() -> TextSystem {
        TextSystem::new(Arc::new(gpui_apple::AppleTextSystem::new()))
    }

    #[test]
    fn xcode_mono_font_is_monospace() {
        let text_system = system();
        assert!(is_monospace(&text_system, MONO_FONT));
        assert!(is_monospace(&text_system, MONO_FALLBACK_FONT));
        assert!(!is_monospace(&text_system, UI_FONT));
    }

    #[test]
    fn mono_advance_is_uniform_across_glyphs() {
        let text_system = system();
        let id = text_system.resolve_font(&font(MONO_FONT));
        let size = px(MONO_FONT_SIZE);
        let reference = text_system.advance(id, size, 'm').unwrap().width;
        for ch in ['i', 'W', '0', '1', ' ', '{', '~', 'g'] {
            assert_eq!(text_system.advance(id, size, ch).unwrap().width, reference, "{ch:?}");
        }
        assert!(reference > px(6.) && reference < px(9.));
    }
}
