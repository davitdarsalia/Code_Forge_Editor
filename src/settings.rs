//! User settings, persisted as JSON next to `recent.json`, plus the small
//! window that edits them (Forge ▸ Settings…, ⌘,).

use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::{
    App, AppContext as _, Context, Entity, Global, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window, div, prelude::FluentBuilder as _, px,
};
use serde::{Deserialize, Serialize};

use crate::theme::{Palette, ca};

/// Everything the user can tune. Unknown or missing fields fall back to the
/// defaults so old files keep loading after new settings are added.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub autosave_delay_ms: u64,
    pub editor_font_size: f32,
    pub terminal_font_size: f32,
    pub soft_wrap: bool,
    pub show_inspector: bool,
    pub open_last_project: bool,
    pub launch_full_screen: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            autosave_delay_ms: 200,
            editor_font_size: 13.,
            terminal_font_size: 12.5,
            soft_wrap: false,
            show_inspector: true,
            open_last_project: true,
            launch_full_screen: true,
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

    /// Keeps hand-edited files from producing unusable values.
    pub fn sanitized(mut self) -> Self {
        let defaults = Settings::default();
        self.autosave_delay_ms = self.autosave_delay_ms.clamp(AUTOSAVE_RANGE.0, AUTOSAVE_RANGE.1);
        for (value, default) in [
            (&mut self.editor_font_size, defaults.editor_font_size),
            (&mut self.terminal_font_size, defaults.terminal_font_size),
        ] {
            *value = if value.is_finite() { value.clamp(FONT_RANGE.0, FONT_RANGE.1) } else { default };
        }
        self
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

/// Generation of the newest settings handed to a background save.
static SAVE_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// Generation last written; the lock also keeps two writes from interleaving.
static SAVED_GENERATION: std::sync::Mutex<u64> = std::sync::Mutex::new(0);

/// Saves on a pool thread can run in any order (typing "500" in the autosave
/// field queues three); an older snapshot must never land after a newer one.
fn save_generation(path: &Path, settings: &Settings, generation: u64) -> std::io::Result<()> {
    let mut saved = SAVED_GENERATION.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if generation <= *saved {
        return Ok(());
    }
    save_to(path, settings)?;
    *saved = generation;
    Ok(())
}

/// Replaces the global settings, persists them and redraws every window so
/// live settings (fonts, wrap) apply at once.
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
    cx.set_global(settings);
    cx.refresh_windows();
}

// ---- Settings window -------------------------------------------------------

pub struct SettingsView {
    autosave: Entity<InputState>,
    editor_font: Entity<InputState>,
    terminal_font: Entity<InputState>,
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

        // Values apply as soon as they parse; out-of-range input is clamped on save.
        let subscriptions = vec![
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
        SettingsView { autosave, editor_font, terminal_font, _subscriptions: subscriptions }
    }
}

fn format_size(size: f32) -> String {
    if size.fract() == 0. { format!("{size:.0}") } else { format!("{size}") }
}

/// Font sizes must be in range to apply live: typing "1" on the way to "14"
/// must not shrink everything for a moment.
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
            .font_family(".SystemUIFont")
            .child(section("EDITOR"))
            .child(row("Autosave delay", number_field(&self.autosave, "ms")))
            .child(row("Editor font size", number_field(&self.editor_font, "pt")))
            .child(row("Soft wrap", toggle("soft-wrap", s.soft_wrap, |s, v| s.soft_wrap = v)))
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
            show_inspector: false,
            open_last_project: false,
            launch_full_screen: false,
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
    fn partial_json_fills_defaults() {
        let settings = Settings::from_json(br#"{"soft_wrap": true}"#);
        assert_eq!(settings, Settings { soft_wrap: true, ..Settings::default() });
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
