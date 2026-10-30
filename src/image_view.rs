use std::cell::Cell;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{
    AppContext as _, Bounds, Context, CursorStyle, FocusHandle, Focusable, Image, ImageFormat, ImageSource,
    InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ObjectFit,
    ParentElement as _, PinchEvent, Pixels, Point, Render, ScrollEvent, SharedString, Styled as _, StyledImage as _, Task, Window, canvas, div, fill, img, point, px, size,
};

use crate::preview::{self, Debounce, RasterFormat, Zoom};
use crate::theme::{Palette, c};

const TILE: f32 = 8.;
const WHEEL_ZOOM_PER_PIXEL: f32 = 0.01;
const DRAG_THRESHOLD: f32 = 3.;

struct Loaded {
    image: Arc<Image>,
    previous: Option<Arc<Image>>,
    dimensions: (u32, u32),
    bytes: u64,
}

enum LoadState {
    Loading,
    Ready(Loaded),
    Failed(SharedString),
}

#[derive(Clone, Copy)]
struct Pan {
    start: Point<Pixels>,
    scroll: (f32, f32),
    moved: bool,
}

pub struct ImageStatus {
    pub dimensions: String,
    pub size: String,
    pub zoom: String,
}

pub struct ImageView {
    label: &'static str,
    state: LoadState,
    zoom: Zoom,
    scroll: (f32, f32),
    pane: Rc<Cell<Bounds<Pixels>>>,
    pan: Option<Pan>,
    focus: FocusHandle,
    debounce: Debounce,
    _load: Option<Task<()>>,
}

fn load(path: &Path, format: RasterFormat) -> Result<(Vec<u8>, (u32, u32)), String> {
    let bytes_len = std::fs::metadata(path).map(|m| m.len()).map_err(|e| format!("Cannot open {}: {e}", path.display()))?;
    if bytes_len > preview::MAX_IMAGE_BYTES {
        return Err(format!("{} is too large to preview", path.display()));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("Cannot open {}: {e}", path.display()))?;
    let dimensions = image::ImageReader::with_format(Cursor::new(&bytes), format.decoder())
        .into_dimensions()
        .map_err(|e| format!("Cannot decode {}: {e}", path.display()))?;
    Ok((bytes, dimensions))
}

pub fn svg_dimensions(bytes: &[u8]) -> Result<(u32, u32), String> {
    let tree = usvg::Tree::from_data(bytes, &usvg::Options::default()).map_err(|e| format!("Invalid SVG: {e}"))?;
    let size = tree.size();
    Ok((size.width().ceil().max(1.) as u32, size.height().ceil().max(1.) as u32))
}

impl ImageView {
    pub fn new(path: PathBuf, format: RasterFormat, cx: &mut Context<Self>) -> Self {
        let task = cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { load(&path, format) }).await;
            let _ = this.update(cx, |this, cx| {
                this.state = match result {
                    Ok((bytes, dimensions)) => LoadState::Ready(Loaded {
                        bytes: bytes.len() as u64,
                        image: Arc::new(Image::from_bytes(format.gpui(), bytes)),
                        previous: None,
                        dimensions,
                    }),
                    Err(message) => LoadState::Failed(message.into()),
                };
                cx.notify();
            });
        });
        let mut view = Self::empty(format.label(), cx);
        view._load = Some(task);
        view
    }

    pub fn svg(cx: &mut Context<Self>) -> Self {
        Self::empty("SVG", cx)
    }

    fn empty(label: &'static str, cx: &mut Context<Self>) -> Self {
        ImageView {
            label,
            state: LoadState::Loading,
            zoom: Zoom::Fit,
            scroll: (0., 0.),
            pane: Rc::new(Cell::new(Bounds::default())),
            pan: None,
            focus: cx.focus_handle(),
            debounce: Debounce::default(),
            _load: None,
        }
    }

    pub fn set_svg(&mut self, bytes: Vec<u8>, cx: &mut Context<Self>) {
        let generation = self.debounce.next();
        self._load = Some(cx.spawn(async move |this, cx| {
            let (bytes, result) = cx
                .background_spawn(async move {
                    let result = svg_dimensions(&bytes);
                    (bytes, result)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if !this.debounce.is_latest(generation) {
                    return;
                }
                this.apply_svg(bytes, result, cx);
            });
        }));
    }

    fn apply_svg(&mut self, bytes: Vec<u8>, result: Result<(u32, u32), String>, cx: &mut Context<Self>) {
        let dimensions = match result {
            Ok(dimensions) => dimensions,
            Err(message) => {
                if !matches!(self.state, LoadState::Ready(_)) {
                    self.state = LoadState::Failed(message.into());
                    cx.notify();
                }
                return;
            }
        };
        let byte_len = bytes.len() as u64;
        let image = Arc::new(Image::from_bytes(ImageFormat::Svg, bytes));
        match &mut self.state {
            LoadState::Ready(loaded) => {
                if loaded.image.id() == image.id() {
                    return;
                }
                if let Some(old) = loaded.previous.take() {
                    old.remove_asset(cx);
                }
                loaded.previous = Some(std::mem::replace(&mut loaded.image, image));
                loaded.dimensions = dimensions;
                loaded.bytes = byte_len;
            }
            _ => {
                self.state = LoadState::Ready(Loaded { image, previous: None, dimensions, bytes: byte_len });
            }
        }
        cx.notify();
    }

    fn natural(&self) -> Option<(f32, f32)> {
        match &self.state {
            LoadState::Ready(loaded) => Some((loaded.dimensions.0 as f32, loaded.dimensions.1 as f32)),
            _ => None,
        }
    }

    fn pane_size(&self) -> (f32, f32) {
        let bounds = self.pane.get();
        (f32::from(bounds.size.width), f32::from(bounds.size.height))
    }

    fn scale(&self) -> f32 {
        match (self.zoom, self.natural()) {
            (Zoom::Scale(scale), _) => scale,
            (Zoom::Fit, Some(natural)) => preview::fit_scale(natural, self.pane_size()),
            (Zoom::Fit, None) => 1.,
        }
    }

    fn content(&self, scale: f32) -> (f32, f32) {
        let natural = self.natural().unwrap_or((0., 0.));
        (natural.0 * scale, natural.1 * scale)
    }

    fn pannable(&self) -> bool {
        let content = self.content(self.scale());
        let pane = self.pane_size();
        content.0 > pane.0 + 0.5 || content.1 > pane.1 + 0.5
    }

    fn local(&self, position: Point<Pixels>) -> (f32, f32) {
        let origin = self.pane.get().origin;
        (f32::from(position.x - origin.x), f32::from(position.y - origin.y))
    }

    fn set_scale_at(&mut self, scale: f32, anchor: Option<(f32, f32)>, cx: &mut Context<Self>) {
        if self.natural().is_none() || !scale.is_finite() {
            return;
        }
        let old = self.content(self.scale());
        let scale = preview::clamp_scale(scale);
        let new = self.content(scale);
        let pane = self.pane_size();
        let anchor = anchor.unwrap_or((pane.0 / 2., pane.1 / 2.));
        self.scroll = (
            preview::anchored_scroll(anchor.0, old.0, new.0, pane.0, self.scroll.0),
            preview::anchored_scroll(anchor.1, old.1, new.1, pane.1, self.scroll.1),
        );
        self.zoom = Zoom::Scale(scale);
        cx.notify();
    }

    pub fn zoom_in(&mut self, cx: &mut Context<Self>) {
        self.set_scale_at(preview::zoom_in(self.scale()), None, cx);
    }

    pub fn zoom_out(&mut self, cx: &mut Context<Self>) {
        self.set_scale_at(preview::zoom_out(self.scale()), None, cx);
    }

    pub fn zoom_fit(&mut self, cx: &mut Context<Self>) {
        self.zoom = Zoom::Fit;
        self.scroll = (0., 0.);
        cx.notify();
    }

    fn pan_by(&mut self, dx: f32, dy: f32, cx: &mut Context<Self>) {
        let content = self.content(self.scale());
        let pane = self.pane_size();
        let next = (
            preview::clamp_scroll(self.scroll.0 + dx, content.0, pane.0),
            preview::clamp_scroll(self.scroll.1 + dy, content.1, pane.1),
        );
        if next != self.scroll {
            self.scroll = next;
            cx.notify();
        }
    }

    pub fn status(&self) -> Option<ImageStatus> {
        match &self.state {
            LoadState::Ready(loaded) => Some(ImageStatus {
                dimensions: preview::format_dimensions(loaded.dimensions.0, loaded.dimensions.1),
                size: preview::format_bytes(loaded.bytes),
                zoom: preview::zoom_label(self.zoom, self.scale()),
            }),
            _ => None,
        }
    }

    fn source(loaded: &Loaded) -> ImageSource {
        let current = loaded.image.clone();
        let Some(previous) = loaded.previous.clone() else { return ImageSource::Image(current) };
        ImageSource::Custom(Arc::new(move |window, cx| {
            if let Some(rendered) = current.clone().use_render_image(window, cx) {
                return Some(Ok(rendered));
            }
            previous.clone().get_render_image(window, cx).map(Ok)
        }))
    }

    fn on_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        self.pan = Some(Pan { start: event.position, scroll: self.scroll, moved: false });
    }

    fn on_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(mut pan) = self.pan else { return };
        if !event.dragging() {
            self.pan = None;
            return;
        }
        let dx = f32::from(event.position.x - pan.start.x);
        let dy = f32::from(event.position.y - pan.start.y);
        if !pan.moved && dx.hypot(dy) < DRAG_THRESHOLD {
            return;
        }
        pan.moved = true;
        self.pan = Some(pan);
        if self.pannable() {
            self.scroll = pan.scroll;
            self.pan_by(-dx, -dy, cx);
        }
    }

    fn on_up(&mut self, event: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(pan) = self.pan.take() else { return };
        if pan.moved {
            cx.notify();
            return;
        }
        let anchor = Some(self.local(event.position));
        let scale = if event.modifiers.alt { preview::zoom_out(self.scale()) } else { preview::zoom_in(self.scale()) };
        self.set_scale_at(scale, anchor, cx);
    }

    fn on_scroll(&mut self, event: &ScrollEvent, _: &mut Window, cx: &mut Context<Self>) {
        let delta = event.delta.pixel_delta(px(16.));
        if event.modifiers.platform || event.modifiers.control {
            let factor = (1. + f32::from(delta.y) * WHEEL_ZOOM_PER_PIXEL).clamp(0.5, 2.);
            let anchor = Some(self.local(event.position));
            self.set_scale_at(self.scale() * factor, anchor, cx);
        } else {
            self.pan_by(-f32::from(delta.x), -f32::from(delta.y), cx);
        }
        cx.stop_propagation();
    }

    fn on_pinch(&mut self, event: &PinchEvent, _: &mut Window, cx: &mut Context<Self>) {
        let anchor = Some(self.local(event.position));
        self.set_scale_at(self.scale() * (1. + event.delta), anchor, cx);
    }
}

impl Focusable for ImageView {
    fn focus_handle(&self, _: &gpui_kit::App) -> FocusHandle {
        self.focus.clone()
    }
}

pub fn checkerboard() -> impl IntoElement {
    canvas(
        |_, _, _| (),
        |bounds: Bounds<Pixels>, _, window, _| {
            let visible = bounds.intersect(&window.content_mask().bounds);
            if visible.size.width <= px(0.) || visible.size.height <= px(0.) {
                return;
            }
            window.paint_quad(fill(visible, c(0x2c2c31)));
            let (ox, oy) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
            let (left, top) = (f32::from(visible.origin.x), f32::from(visible.origin.y));
            let right = left + f32::from(visible.size.width);
            let bottom = top + f32::from(visible.size.height);
            let first_col = ((left - ox) / TILE).floor() as i32;
            let last_col = ((right - ox) / TILE).ceil() as i32;
            let first_row = ((top - oy) / TILE).floor() as i32;
            let last_row = ((bottom - oy) / TILE).ceil() as i32;
            let light = c(0x3c3c42);
            for row in first_row..last_row {
                for col in first_col..last_col {
                    if (row + col).rem_euclid(2) != 0 {
                        continue;
                    }
                    let tile = Bounds::new(
                        point(px(ox + col as f32 * TILE), px(oy + row as f32 * TILE)),
                        size(px(TILE), px(TILE)),
                    )
                    .intersect(&visible);
                    if tile.size.width > px(0.) && tile.size.height > px(0.) {
                        window.paint_quad(fill(tile, light));
                    }
                }
            }
        },
    )
    .absolute()
    .size_full()
}

impl Render for ImageView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let measure = {
            let cell = self.pane.clone();
            let view = cx.entity().downgrade();
            canvas(
                move |bounds, _, cx| {
                    let known = cell.get();
                    cell.set(bounds);
                    let resized = (f32::from(bounds.size.width) - f32::from(known.size.width)).abs() > 0.5
                        || (f32::from(bounds.size.height) - f32::from(known.size.height)).abs() > 0.5;
                    if resized {
                        cx.defer(move |cx| {
                            let _ = view.update(cx, |_, cx| cx.notify());
                        });
                    }
                },
                |_, _, _, _| {},
            )
            .absolute()
            .size_full()
        };
        let pane = self.pane_size();
        let body = match &self.state {
            LoadState::Loading => centered(format!("Loading {}…", self.label)),
            LoadState::Failed(message) => centered(message.clone()),
            LoadState::Ready(loaded) => {
                let (width, height) = self.content(self.scale());
                let left = preview::image_origin(width, pane.0, self.scroll.0);
                let top = preview::image_origin(height, pane.1, self.scroll.1);
                div()
                    .absolute()
                    .left(px(left))
                    .top(px(top))
                    .w(px(width))
                    .h(px(height))
                    .child(checkerboard())
                    .child(img(Self::source(loaded)).absolute().size_full().object_fit(ObjectFit::Fill))
                    .into_any_element()
            }
        };
        let cursor = match self.pan {
            Some(pan) if pan.moved && self.pannable() => CursorStyle::ClosedHand,
            _ if self.pannable() => CursorStyle::OpenHand,
            _ => CursorStyle::Arrow,
        };
        div()
            .id("image-view")
            .key_context("ImageView")
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .overflow_hidden()
            .bg(Palette::editor_bg())
            .cursor(cursor)
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_down))
            .on_mouse_move(cx.listener(Self::on_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_up))
            .on_scroll(cx.listener(Self::on_scroll))
            .on_pinch(cx.listener(Self::on_pinch))
            .child(measure)
            .child(body)
    }
}

fn centered(text: impl Into<SharedString>) -> gpui_kit::AnyElement {
    div()
        .absolute()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .px(px(16.))
        .text_color(Palette::faint())
        .child(text.into())
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn svg_dimensions_come_from_the_root_element() {
        assert_eq!(svg_dimensions(br#"<svg xmlns="http://www.w3.org/2000/svg" width="64" height="32"/>"#), Ok((64, 32)));
        assert_eq!(
            svg_dimensions(br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 12"><rect width="1" height="1"/></svg>"#),
            Ok((24, 12))
        );
        assert!(svg_dimensions(b"<svg").is_err());
    }
}
