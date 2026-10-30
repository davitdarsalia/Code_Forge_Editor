use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui_kit::component::input::EditorState;
use gpui_kit::component::text::{TextView, TextViewState};
use gpui_kit::{
    App, AppContext as _, Context, Entity, ImageFormat, Image, ImageSource, IntoElement, ObjectFit, ParentElement as _,
    Render, SharedString, SharedUri, Styled as _, StyledImage as _, Task, Window, div, img, px,
};

use crate::image_view::checkerboard;
use crate::preview::{Debounce, PREVIEW_DEBOUNCE};
use crate::theme::Palette;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PreviewKind {
    Svg,
    Markdown,
}

pub struct PreviewPane {
    kind: PreviewKind,
    editor: Entity<EditorState>,
    base_dir: Arc<Path>,
    visible: bool,
    stale: bool,
    debounce: Debounce,
    task: Option<Task<()>>,
    current: Option<Arc<Image>>,
    previous: Option<Arc<Image>>,
    markdown: Entity<TextViewState>,
}

impl PreviewPane {
    pub fn new(kind: PreviewKind, editor: Entity<EditorState>, file: &Path, cx: &mut Context<Self>) -> Self {
        let markdown = cx.new(|cx| TextViewState::markdown("", cx).selectable(true).scrollable(true));
        let base_dir: PathBuf = file.parent().map(Path::to_path_buf).unwrap_or_default();
        PreviewPane {
            kind,
            editor,
            base_dir: base_dir.into(),
            visible: false,
            stale: true,
            debounce: Debounce::default(),
            task: None,
            current: None,
            previous: None,
            markdown,
        }
    }

    pub fn content_changed(&mut self, cx: &mut Context<Self>) {
        self.stale = true;
        if self.visible {
            self.schedule(cx);
        }
    }

    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if visible == self.visible {
            return;
        }
        self.visible = visible;
        if visible {
            if self.stale {
                self.refresh(cx);
            }
        } else {
            self.task = None;
        }
    }

    fn schedule(&mut self, cx: &mut Context<Self>) {
        let generation = self.debounce.next();
        self.task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(PREVIEW_DEBOUNCE).await;
            let _ = this.update(cx, |this, cx| {
                if this.debounce.is_latest(generation) && this.visible {
                    this.refresh(cx);
                }
            });
        }));
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.stale = false;
        self.task = None;
        let text = self.editor.read(cx).value();
        match self.kind {
            PreviewKind::Markdown => {
                self.markdown.update(cx, |state, cx| state.set_text(&text, cx));
            }
            PreviewKind::Svg => {
                let image = Arc::new(Image::from_bytes(ImageFormat::Svg, text.as_bytes().to_vec()));
                if self.current.as_ref().is_some_and(|current| current.id() == image.id()) {
                    return;
                }
                if let Some(old) = self.previous.take() {
                    old.remove_asset(cx);
                }
                self.previous = self.current.replace(image);
            }
        }
        cx.notify();
    }

    fn svg_source(&self) -> Option<ImageSource> {
        let current = self.current.clone()?;
        let previous = self.previous.clone();
        Some(ImageSource::Custom(Arc::new(move |window, cx| {
            if let Some(rendered) = current.clone().use_render_image(window, cx) {
                return Some(Ok(rendered));
            }
            previous.clone().and_then(|image| image.get_render_image(window, cx)).map(Ok)
        })))
    }
}

pub fn resolve_image(base: &Path, uri: &SharedUri) -> ImageSource {
    let raw: &str = uri.as_ref();
    if raw.contains("://") || raw.starts_with("data:") {
        return ImageSource::from(uri.clone());
    }
    let path = raw.strip_prefix("file://").unwrap_or(raw);
    let path = path.split(['?', '#']).next().unwrap_or(path);
    let path = Path::new(path);
    if path.is_absolute() {
        ImageSource::from(path.to_path_buf())
    } else {
        ImageSource::from(base.join(path))
    }
}

pub fn open_link(base: &Path, url: &str, cx: &mut App) {
    let external = url.contains("://") || url.starts_with("mailto:");
    if external {
        cx.open_url(url);
        return;
    }
    let target = url.split(['?', '#']).next().unwrap_or(url);
    if target.is_empty() {
        return;
    }
    let path = if Path::new(target).is_absolute() { PathBuf::from(target) } else { base.join(target) };
    cx.open_url(&format!("file://{}", path.display()));
}

impl Render for PreviewPane {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let body = match self.kind {
            PreviewKind::Markdown => {
                let base = self.base_dir.clone();
                let link_base = self.base_dir.clone();
                div()
                    .size_full()
                    .px(px(20.))
                    .py(px(14.))
                    .child(
                        TextView::new(&self.markdown)
                            .scrollable(true)
                            .selectable(true)
                            .size_full()
                            .image_source(move |uri| resolve_image(&base, uri))
                            .on_link_click(move |url: &SharedString, _, _, cx| open_link(&link_base, url, cx)),
                    )
                    .into_any_element()
            }
            PreviewKind::Svg => {
                let mut surface = div().size_full().relative().child(checkerboard());
                if let Some(source) = self.svg_source() {
                    surface = surface.child(img(source).absolute().size_full().object_fit(ObjectFit::Contain));
                }
                surface.into_any_element()
            }
        };
        div().size_full().bg(Palette::editor_bg()).child(body)
    }
}
