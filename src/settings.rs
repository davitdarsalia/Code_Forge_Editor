use std::path::{Path, PathBuf};
use std::time::Duration;

use std::path::Path as StdPath;

use gpui_kit::assets::IconName;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::{
    App, AppContext as _, Context, Entity, Global, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window, div, prelude::FluentBuilder as _, px,
};
use serde::{Deserialize, Serialize};

use crate::theme::{Palette, ca};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub autosave_delay_ms: u64,
    pub editor_font_size: f32,
    pub terminal_font_size: f32,
    pub soft_wrap: bool,
    pub format_on_save: bool,
    pub show_inspector: bool,
    pub open_last_project: bool,
    pub launch_full_screen: bool,
    pub icon_scale: f32,
    pub preview_fraction: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            autosave_delay_ms: 200,
            editor_font_size: 12.,
            terminal_font_size: 12.,
            soft_wrap: false,
            format_on_save: true,
            show_inspector: true,
            open_last_project: true,
            launch_full_screen: true,
            icon_scale: crate::icons::DEFAULT_SCALE,
            preview_fraction: crate::preview::DEFAULT_FRACTION,
        }
    }
}

impl Global for Settings {}

pub const AUTOSAVE_RANGE: (u64, u64) = (50, 10_000);
pub const FONT_RANGE: (f32, f32) = (8., 32.);

impl Settings {
    pub fn get(cx: &App) -> &Settings {
        cx.try_global::<Settings>().unwrap_or_else(|| default_settings())
    }

    pub fn autosave_delay(&self) -> Duration {
        Duration::from_millis(self.autosave_delay_ms)
    }

    pub fn sanitized(mut self) -> Self {
        let defaults = Settings::default();
        self.autosave_delay_ms = self.autosave_delay_ms.clamp(AUTOSAVE_RANGE.0, AUTOSAVE_RANGE.1);
        for (value, default) in [
            (&mut self.editor_font_size, defaults.editor_font_size),
            (&mut self.terminal_font_size, defaults.terminal_font_size),
        ] {
            *value = if value.is_finite() { value.clamp(FONT_RANGE.0, FONT_RANGE.1) } else { default };
        }
        let (lo, hi) = crate::icons::SCALE_RANGE;
        self.icon_scale = if self.icon_scale.is_finite() { self.icon_scale.clamp(lo, hi) } else { defaults.icon_scale };
        self.preview_fraction = crate::preview::clamp_fraction(self.preview_fraction);
        self
    }

    pub fn apply_statics(&self) {
        crate::icons::set_scale(self.icon_scale);
    }

    pub fn from_json(bytes: &[u8]) -> Settings {
        match serde_json::from_slice::<Settings>(bytes) {
            Ok(settings) => settings.sanitized(),
            Err(err) => {
                eprintln!("forge: settings file is invalid, using defaults: {err}");
                Settings::default()
            }
        }
    }
}

fn default_settings() -> &'static Settings {
    static DEFAULT: std::sync::OnceLock<Settings> = std::sync::OnceLock::new();
    DEFAULT.get_or_init(Settings::default)
}

fn store_path() -> Option<PathBuf> {
    dirs::data_dir().map(|dir| dir.join("Forge").join("settings.json"))
}

pub fn load() -> Settings {
    store_path().map(|p| load_from(&p)).unwrap_or_default()
}

fn load_from(path: &Path) -> Settings {
    match std::fs::read(path) {
        Ok(bytes) => Settings::from_json(&bytes),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Settings::default(),
        Err(err) => {
            eprintln!("forge: cannot read settings: {err}");
            Settings::default()
        }
    }
}

fn save_to(path: &Path, settings: &Settings) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec_pretty(settings).map_err(std::io::Error::other)?;
    crate::files::write_atomic(path, &bytes)
}

static SAVE_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static SAVED_GENERATION: std::sync::Mutex<u64> = std::sync::Mutex::new(0);

fn save_generation(path: &Path, settings: &Settings, generation: u64) -> std::io::Result<()> {
    let mut saved = SAVED_GENERATION.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if generation <= *saved {
        return Ok(());
    }
    save_to(path, settings)?;
    *saved = generation;
    Ok(())
}

pub fn update(cx: &mut App, change: impl FnOnce(&mut Settings)) {
    let mut settings = Settings::get(cx).clone();
    change(&mut settings);
    let settings = settings.sanitized();
    if &settings == Settings::get(cx) {
        return;
    }
    if let Some(path) = store_path() {
        let snapshot = settings.clone();
        let generation = SAVE_GENERATION.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        cx.background_spawn(async move {
            if let Err(err) = save_generation(&path, &snapshot, generation) {
                eprintln!("forge: failed to save settings: {err}");
            }
        })
        .detach();
    }
    settings.apply_statics();
    cx.set_global(settings);
    cx.refresh_windows();
}

pub struct SettingsView {
    autosave: Entity<InputState>,
    editor_font: Entity<InputState>,
    terminal_font: Entity<InputState>,
    icon_size: Entity<SliderState>,
    _subscriptions: Vec<Subscription>,
}

impl SettingsView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let current = Settings::get(cx).clone();
        let field = |value: String, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| InputState::new(window, cx).default_value(value))
        };
        let autosave = field(current.autosave_delay_ms.to_string(), window, cx);
        let editor_font = field(format_size(current.editor_font_size), window, cx);
        let terminal_font = field(format_size(current.terminal_font_size), window, cx);

        let (lo, hi) = crate::icons::SCALE_RANGE;
        let icon_size = cx.new(|_| {
            SliderState::new()
                .min(lo * 100.)
                .max(hi * 100.)
                .step(5.)
                .default_value(current.icon_scale * 100.)
        });

        let subscriptions = vec![
            cx.subscribe(&icon_size, |_, _, event: &SliderEvent, cx| {
                let (SliderEvent::Change(value) | SliderEvent::Release(value)) = event;
                let percent = value.start();
                update(cx, |s| s.icon_scale = percent / 100.);
            }),
            cx.subscribe(&autosave, |_, input, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    if let Ok(ms) = input.read(cx).value().trim().parse::<u64>() {
                        update(cx, |s| s.autosave_delay_ms = ms);
                    }
                }
            }),
            cx.subscribe(&editor_font, |_, input, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    if let Some(size) = parse_size(&input.read(cx).value()) {
                        update(cx, |s| s.editor_font_size = size);
                    }
                }
            }),
            cx.subscribe(&terminal_font, |_, input, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    if let Some(size) = parse_size(&input.read(cx).value()) {
                        update(cx, |s| s.terminal_font_size = size);
                    }
                }
            }),
        ];
        SettingsView { autosave, editor_font, terminal_font, icon_size, _subscriptions: subscriptions }
    }
}

fn format_size(size: f32) -> String {
    if size.fract() == 0. { format!("{size:.0}") } else { format!("{size}") }
}

fn parse_size(text: &str) -> Option<f32> {
    text.trim().parse::<f32>().ok().filter(|s| (FONT_RANGE.0..=FONT_RANGE.1).contains(s))
}

fn row(label: &'static str, control: impl IntoElement) -> impl IntoElement {
    div()
        .h(px(34.))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(12.))
        .child(div().text_size(px(13.)).text_color(Palette::text()).child(label))
        .child(control)
}

fn number_field(input: &Entity<InputState>, unit: &'static str) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(6.))
        .child(div().w(px(80.)).child(Input::new(input)))
        .child(div().w(px(20.)).text_size(px(11.)).text_color(Palette::faint()).child(unit))
}

fn icon_preview() -> impl IntoElement {
    const FILES: &[&str] = &["App.swift", "MainActivity.kt", "build.gradle.kts", "AndroidManifest.xml", "Gemfile", "package.json", "main.rs"];
    let tree_row = |name: &'static str, folder: bool, depth: f32| {
        let path = StdPath::new(name);
        div()
            .h(px(crate::icons::row_height(22., crate::file_icons::ICON_SIZE)))
            .pl(px(8. + depth * 14.))
            .flex()
            .items_center()
            .gap(px(5.))
            .text_size(px(12.5))
            .child(if folder {
                crate::file_icons::folder_icon_img(path, true)
            } else {
                crate::file_icons::file_icon_img(path)
            })
            .child(name)
    };
    div()
        .mt(px(6.))
        .p(px(10.))
        .rounded(px(8.))
        .bg(Palette::sidebar_bg())
        .border_1()
        .border_color(Palette::hairline())
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(10.))
                .children(FILES.iter().map(|name| crate::file_icons::file_icon_img(StdPath::new(name))))
                .child(crate::file_icons::folder_icon_img(StdPath::new("src"), false))
                .child(crate::file_icons::folder_icon_img(StdPath::new("android"), false)),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .children(
                    [IconName::PanelLeft, IconName::Search, IconName::GitBranch, IconName::SquareTerminal, IconName::PanelRight]
                        .into_iter()
                        .map(|name| crate::icons::ui_icon(name, 16., Palette::muted())),
                ),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .child(tree_row("app", true, 0.))
                .child(tree_row("MainActivity.kt", false, 1.))
                .child(tree_row("build.gradle.kts", false, 1.)),
        )
}

fn toggle(
    id: &'static str,
    on: bool,
    change: impl Fn(&mut Settings, bool) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .w(px(34.))
        .h(px(20.))
        .flex_none()
        .rounded_full()
        .p(px(2.))
        .flex()
        .map(|el| if on { el.justify_end() } else { el.justify_start() })
        .bg(if on { Palette::accent() } else { ca(0xffffff, 0.15) })
        .cursor_pointer()
        .child(div().size(px(16.)).rounded_full().bg(gpui_kit::white()))
        .on_click(move |_, _, cx| update(cx, |s| change(s, !on)))
}

impl Render for SettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let s = Settings::get(cx).clone();
        let section = |title: &'static str| {
            div()
                .pt(px(14.))
                .pb(px(4.))
                .text_size(px(11.))
                .text_color(Palette::muted())
                .child(SharedString::from(title))
        };
        div()
            .size_full()
            .px(px(20.))
            .pt(px(36.))
            .pb(px(16.))
            .flex()
            .flex_col()
            .bg(Palette::editor_bg())
            .text_color(Palette::text())
            .font_family(crate::theme::UI_FONT)
            .child(section("EDITOR"))
            .child(row("Autosave delay", number_field(&self.autosave, "ms")))
            .child(row("Editor font size", number_field(&self.editor_font, "pt")))
            .child(row("Soft wrap", toggle("soft-wrap", s.soft_wrap, |s, v| s.soft_wrap = v)))
            .child(row("Format on save", toggle("format-on-save", s.format_on_save, |s, v| s.format_on_save = v)))
            .child(section("ICONS"))
            .child(row(
                "Icon size",
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(div().w(px(180.)).child(Slider::new(&self.icon_size)))
                    .child(
                        div()
                            .w(px(40.))
                            .text_size(px(12.))
                            .text_color(Palette::muted())
                            .child(format!("{:.0}%", s.icon_scale * 100.)),
                    ),
            ))
            .child(icon_preview())
            .child(section("TERMINAL"))
            .child(row("Terminal font size", number_field(&self.terminal_font, "pt")))
            .child(section("WINDOW"))
            .child(row("Show inspector by default", toggle("show-inspector", s.show_inspector, |s, v| s.show_inspector = v)))
            .child(row("Open last project on launch", toggle("open-last", s.open_last_project, |s, v| s.open_last_project = v)))
            .child(row("Launch full screen", toggle("full-screen", s.launch_full_screen, |s, v| s.launch_full_screen = v)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_gives_defaults() {
        let dir = std::env::temp_dir().join(format!("forge-settings-missing-{}", std::process::id()));
        assert_eq!(load_from(&dir.join("settings.json")), Settings::default());
    }

    #[test]
    fn save_load_round_trip() {
        let dir = std::env::temp_dir().join(format!("forge-settings-rt-{}", std::process::id()));
        let path = dir.join("Forge").join("settings.json");
        let settings = Settings {
            autosave_delay_ms: 750,
            editor_font_size: 15.,
            terminal_font_size: 11.5,
            soft_wrap: true,
            format_on_save: false,
            show_inspector: false,
            open_last_project: false,
            launch_full_screen: false,
            icon_scale: 1.5,
            preview_fraction: 0.35,
        };
        save_to(&path, &settings).unwrap();
        assert_eq!(load_from(&path), settings);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn older_save_never_overwrites_a_newer_one() {
        let dir = std::env::temp_dir().join(format!("forge-settings-gen-{}", std::process::id()));
        let path = dir.join("settings.json");
        let base = *SAVED_GENERATION.lock().unwrap();
        let newer = Settings { autosave_delay_ms: 500, ..Settings::default() };
        let older = Settings { autosave_delay_ms: 5, ..Settings::default() };
        save_generation(&path, &newer, base + 2).unwrap();
        save_generation(&path, &older, base + 1).unwrap();
        assert_eq!(load_from(&path), newer);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn preview_fraction_is_clamped_on_load() {
        let wide = Settings::from_json(br#"{"preview_fraction": 3.0}"#);
        assert_eq!(wide.preview_fraction, crate::preview::FRACTION_RANGE.1);
        let narrow = Settings::from_json(br#"{"preview_fraction": -1.0}"#);
        assert_eq!(narrow.preview_fraction, crate::preview::FRACTION_RANGE.0);
    }

    #[test]
    fn partial_json_fills_defaults() {
        let settings = Settings::from_json(br#"{"soft_wrap": true}"#);
        assert_eq!(settings, Settings { soft_wrap: true, ..Settings::default() });
    }

    #[test]
    fn format_on_save_defaults_on_and_can_be_disabled() {
        assert!(Settings::default().format_on_save);
        assert!(Settings::from_json(b"{}").format_on_save);
        assert!(!Settings::from_json(br#"{"format_on_save": false}"#).format_on_save);
    }

    #[test]
    fn invalid_json_gives_defaults() {
        assert_eq!(Settings::from_json(b"not json"), Settings::default());
    }

    #[test]
    fn out_of_range_values_are_clamped() {
        let settings = Settings::from_json(br#"{"autosave_delay_ms": 0, "editor_font_size": 400}"#);
        assert_eq!(settings.autosave_delay_ms, AUTOSAVE_RANGE.0);
        assert_eq!(settings.editor_font_size, FONT_RANGE.1);
    }

    #[test]
    fn parse_size_rejects_out_of_range() {
        assert_eq!(parse_size("14"), Some(14.));
        assert_eq!(parse_size("1"), None);
        assert_eq!(parse_size("x"), None);
    }
}
