use std::cell::Cell;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{
    AppContext as _, Bounds, Context, FocusHandle, Focusable, Image, ImageSource, InteractiveElement as _, IntoElement,
    ObjectFit, ParentElement as _, PinchEvent, Pixels, Render, ScrollEvent, SharedString,
    StatefulInteractiveElement as _, Styled as _, StyledImage as _, Task, Window, canvas, div, fill, img,
    point, px, size,
};

use crate::preview::{self, RasterFormat, Zoom};
use crate::theme::{Palette, c};

const TILE: f32 = 10.;
const FOOTER_HEIGHT: f32 = 24.;
const WHEEL_ZOOM_PER_PIXEL: f32 = 0.01;

struct Loaded {
    image: Arc<Image>,
    dimensions: (u32, u32),
    bytes: u64,
}

enum LoadState {
    Loading,
    Ready(Loaded),
    Failed(SharedString),
}

pub struct ImageView {
    format: RasterFormat,
    state: LoadState,
    zoom: Zoom,
    pane: Rc<Cell<(f32, f32)>>,
    focus: FocusHandle,
    _load: Task<()>,
}

fn load(path: &Path, format: RasterFormat) -> Result<Loaded, String> {
    let bytes_len = std::fs::metadata(path).map(|m| m.len()).map_err(|e| format!("Cannot open {}: {e}", path.display()))?;
    if bytes_len > preview::MAX_IMAGE_BYTES {
        return Err(format!("{} is too large to preview", path.display()));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("Cannot open {}: {e}", path.display()))?;
    let dimensions = image::ImageReader::with_format(Cursor::new(&bytes), format.decoder())
        .into_dimensions()
        .map_err(|e| format!("Cannot decode {}: {e}", path.display()))?;
    Ok(Loaded { image: Arc::new(Image::from_bytes(format.gpui(), bytes)), dimensions, bytes: bytes_len })
}

impl ImageView {
    pub fn new(path: PathBuf, format: RasterFormat, cx: &mut Context<Self>) -> Self {
        let task = cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { load(&path, format) }).await;
            let _ = this.update(cx, |this, cx| {
                this.state = match result {
                    Ok(loaded) => LoadState::Ready(loaded),
                    Err(message) => LoadState::Failed(message.into()),
                };
                cx.notify();
            });
        });
        ImageView {
            format,
            state: LoadState::Loading,
            zoom: Zoom::Fit,
            pane: Rc::new(Cell::new((0., 0.))),
            focus: cx.focus_handle(),
            _load: task,
        }
    }

    fn natural(&self) -> Option<(f32, f32)> {
        match &self.state {
            LoadState::Ready(loaded) => Some((loaded.dimensions.0 as f32, loaded.dimensions.1 as f32)),
            _ => None,
        }
    }

    fn scale(&self) -> f32 {
        match (self.zoom, self.natural()) {
            (Zoom::Scale(scale), _) => scale,
            (Zoom::Fit, Some(natural)) => preview::fit_scale(natural, self.pane.get()),
            (Zoom::Fit, None) => 1.,
        }
    }

    pub fn zoom_in(&mut self, cx: &mut Context<Self>) {
        self.zoom = Zoom::Scale(preview::zoom_in(self.scale()));
        cx.notify();
    }

    pub fn zoom_out(&mut self, cx: &mut Context<Self>) {
        self.zoom = Zoom::Scale(preview::zoom_out(self.scale()));
        cx.notify();
    }

    pub fn zoom_fit(&mut self, cx: &mut Context<Self>) {
        self.zoom = Zoom::Fit;
        cx.notify();
    }

    fn zoom_by(&mut self, factor: f32, cx: &mut Context<Self>) {
        if self.natural().is_none() || !factor.is_finite() {
            return;
        }
        self.zoom = Zoom::Scale(preview::clamp_scale(self.scale() * factor));
        cx.notify();
    }

    fn footer(&self) -> String {
        match &self.state {
            LoadState::Ready(loaded) => {
                let zoom = match self.zoom {
                    Zoom::Fit => format!("Fit {:.0}%", self.scale() * 100.),
                    Zoom::Scale(scale) => format!("{:.0}%", scale * 100.),
                };
                format!(
                    "{} × {} px   {}   {}   {}",
                    loaded.dimensions.0,
                    loaded.dimensions.1,
                    preview::format_bytes(loaded.bytes),
                    self.format.label(),
                    zoom
                )
            }
            LoadState::Loading => format!("Loading {}…", self.format.label()),
            LoadState::Failed(_) => self.format.label().to_string(),
        }
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
        let pane = self.pane.get();
        let measure = {
            let cell = self.pane.clone();
            let view = cx.entity().downgrade();
            canvas(
                move |bounds, _, cx| {
                    let measured = (f32::from(bounds.size.width), f32::from(bounds.size.height));
                    let known = cell.get();
                    if (measured.0 - known.0).abs() > 0.5 || (measured.1 - known.1).abs() > 0.5 {
                        cell.set(measured);
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
        let body = match &self.state {
            LoadState::Loading => centered("Loading…"),
            LoadState::Failed(message) => centered(message.clone()),
            LoadState::Ready(loaded) => {
                let scale = self.scale();
                let width = loaded.dimensions.0 as f32 * scale;
                let height = loaded.dimensions.1 as f32 * scale;
                let surface = div()
                    .flex_none()
                    .relative()
                    .w(px(width))
                    .h(px(height))
                    .child(checkerboard())
                    .child(
                        img(ImageSource::Image(loaded.image.clone()))
                            .absolute()
                            .size_full()
                            .object_fit(ObjectFit::Fill),
                    );
                div()
                    .id("image-scroll")
                    .absolute()
                    .size_full()
                    .overflow_scroll()
                    .on_scroll(cx.listener(|this, event: &ScrollEvent, _, cx| {
                        if event.modifiers.platform {
                            let delta = event.delta.pixel_delta(px(16.));
                            this.zoom_by(1. + f32::from(delta.y) * WHEEL_ZOOM_PER_PIXEL, cx);
                            cx.stop_propagation();
                        }
                    }))
                    .on_pinch(cx.listener(|this, event: &PinchEvent, _, cx| {
                        this.zoom_by(1. + event.delta, cx);
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .w(px(width.max(pane.0)))
                            .h(px(height.max(pane.1)))
                            .child(surface),
                    )
                    .into_any_element()
            }
        };
        div()
            .id("image-view")
            .key_context("ImageView")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .bg(Palette::editor_bg())
            .child(div().flex_1().min_h(px(0.)).relative().overflow_hidden().child(measure).child(body))
            .child(
                div()
                    .h(px(FOOTER_HEIGHT))
                    .flex_none()
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .border_t_1()
                    .border_color(Palette::border())
                    .bg(Palette::sidebar_bg())
                    .text_size(px(11.5))
                    .text_color(Palette::muted())
                    .child(self.footer()),
            )
    }
}

fn centered(text: impl Into<SharedString>) -> gpui_kit::AnyElement {
    div()
        .absolute()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .text_color(Palette::faint())
        .child(text.into())
        .into_any_element()
}
