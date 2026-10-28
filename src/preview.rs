//! Live canvas for a single JSX/TSX file, in the spirit of Xcode previews.
//!
//! A native WKWebView (via wry) floats over the GPUI window, kept inside the
//! bounds of a placeholder element. The page compiles the source with Babel and
//! renders it with React; imports other than react/react-dom become labeled
//! stubs until real module resolution exists.

use std::cell::Cell;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::{App, Bounds, Pixels, Window};
use wry::dpi::{LogicalPosition, LogicalSize};
use wry::{Rect, WebView, WebViewBuilder};

/// How long after the last keystroke the canvas re-renders.
pub const UPDATE_DELAY: Duration = Duration::from_millis(150);

const PAGE: &str = include_str!("../assets/preview/index.html");

pub fn supports(path: &Path) -> bool {
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    matches!(ext.as_str(), "jsx" | "tsx")
}

pub struct Preview {
    webview: Option<Rc<WebView>>,
    visible: Rc<Cell<bool>>,
    sent: Option<u64>,
}

impl Preview {
    pub fn new() -> Self {
        Preview { webview: None, visible: Rc::new(Cell::new(false)), sent: None }
    }

    /// Renders `source` in the canvas, creating the web view on first use.
    pub fn update(&mut self, path: &Path, source: &str, window: &Window) {
        let mut hasher = DefaultHasher::new();
        (path, source).hash(&mut hasher);
        let hash = hasher.finish();
        if self.sent == Some(hash) {
            return;
        }
        let filename = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let call = format!("window.forgeUpdate({}, {});", js_string(source), js_string(&filename));
        match self.webview.as_ref() {
            Some(webview) => {
                if let Err(err) = webview.evaluate_script(&call) {
                    eprintln!("forge: preview update failed: {err}");
                    return;
                }
            }
            None => {
                let html = PAGE.replace("/*FORGE_INITIAL*/", &call);
                let built = WebViewBuilder::new()
                    .with_html(html)
                    .with_visible(false)
                    .with_bounds(Rect {
                        position: LogicalPosition::new(0., 0.).into(),
                        size: LogicalSize::new(1., 1.).into(),
                    })
                    .build_as_child(window);
                match built {
                    Ok(webview) => self.webview = Some(Rc::new(webview)),
                    Err(err) => {
                        eprintln!("forge: cannot create preview web view: {err}");
                        return;
                    }
                }
            }
        }
        self.sent = Some(hash);
    }

    pub fn hide(&self) {
        if let Some(webview) = self.webview.as_ref() {
            set_visible(webview, &self.visible, false);
        }
    }

    /// Prepaint callback for the placeholder element: moves the native view
    /// onto the element's bounds and shows it.
    pub fn place(&self) -> impl FnOnce(Bounds<Pixels>, &mut Window, &mut App) + 'static {
        let webview = self.webview.clone();
        let visible = self.visible.clone();
        move |bounds, _, _| {
            let Some(webview) = webview else { return };
            let rect = Rect {
                position: LogicalPosition::new(f64::from(f32::from(bounds.origin.x)), f64::from(f32::from(bounds.origin.y))).into(),
                size: LogicalSize::new(
                    f64::from(f32::from(bounds.size.width)).max(1.),
                    f64::from(f32::from(bounds.size.height)).max(1.),
                )
                .into(),
            };
            if let Err(err) = webview.set_bounds(rect) {
                eprintln!("forge: preview resize failed: {err}");
            }
            set_visible(&webview, &visible, true);
        }
    }
}

fn set_visible(webview: &WebView, visible: &Cell<bool>, value: bool) {
    if visible.get() == value {
        return;
    }
    match webview.set_visible(value) {
        Ok(()) => visible.set(value),
        Err(err) => eprintln!("forge: preview visibility failed: {err}"),
    }
}

/// JSON string literal that is also safe inside an inline `<script>`.
fn js_string(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_else(|_| "\"\"".into()).replace("</", "<\\/")
}
