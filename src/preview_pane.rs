use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui_kit::component::input::EditorState;
use gpui_kit::{
    App, AppContext as _, Context, Entity, ImageSource, IntoElement, ParentElement as _, Render, SharedUri, Styled as _,
    Task, Window, div,
};

use crate::image_view::ImageView;
use crate::markdown_view::{self, MarkdownView};
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
    visible: bool,
    stale: bool,
    debounce: Debounce,
    task: Option<Task<()>>,
    markdown: Option<Entity<MarkdownView>>,
    image: Option<Entity<ImageView>>,
    synced_line: Option<usize>,
}

impl PreviewPane {
    pub fn new(kind: PreviewKind, editor: Entity<EditorState>, file: &Path, cx: &mut Context<Self>) -> Self {
        let base_dir: Arc<Path> = file.parent().map(Path::to_path_buf).unwrap_or_default().into();
        let (markdown, image) = match kind {
            PreviewKind::Markdown => (Some(cx.new(|_| MarkdownView::new(base_dir))), None),
            PreviewKind::Svg => (None, Some(cx.new(ImageView::svg))),
        };
        PreviewPane {
            kind,
            editor,
            visible: false,
            stale: true,
            debounce: Debounce::default(),
            task: None,
            markdown,
            image,
            synced_line: None,
        }
    }

    pub fn image_view(&self) -> Option<Entity<ImageView>> {
        self.image.clone()
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
        } else if self.task.take().is_some() {
            self.stale = true;
        }
    }

    pub fn sync_line(&mut self, line: usize, cx: &mut Context<Self>) {
        if self.synced_line == Some(line) {
            return;
        }
        self.synced_line = Some(line);
        if let Some(markdown) = self.markdown.as_ref() {
            markdown.update(cx, |view, cx| view.reveal_line(line, cx));
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
                let generation = self.debounce.next();
                let dark = markdown_view::is_dark(cx);
                self.task = Some(cx.spawn(async move |this, cx| {
                    let doc = cx.background_spawn(async move { markdown_view::parse(&text, dark) }).await;
                    let _ = this.update(cx, |this, cx| {
                        this.task = None;
                        if !this.debounce.is_latest(generation) {
                            return;
                        }
                        if let Some(markdown) = this.markdown.as_ref() {
                            markdown.update(cx, |view, cx| view.set_document(doc, cx));
                        }
                    });
                }));
            }
            PreviewKind::Svg => {
                if let Some(image) = self.image.as_ref() {
                    let bytes = text.as_bytes().to_vec();
                    image.update(cx, |view, cx| view.set_svg(bytes, cx));
                }
            }
        }
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
        div()
            .size_full()
            .bg(Palette::editor_bg())
            .children(self.markdown.clone().map(|view| div().size_full().child(view)))
            .children(self.image.clone().map(|view| div().size_full().child(view)))
    }
}
