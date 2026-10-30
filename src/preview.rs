use std::path::Path;
use std::time::Duration;

pub const PREVIEW_DEBOUNCE: Duration = Duration::from_millis(150);
pub const DEFAULT_FRACTION: f32 = 0.5;
pub const FRACTION_RANGE: (f32, f32) = (0.15, 0.85);
pub const ZOOM_STEPS: [f32; 18] = [0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1., 1.5, 2., 3., 5., 7., 10., 15., 20.];
pub const ZOOM_RANGE: (f32, f32) = (ZOOM_STEPS[0], ZOOM_STEPS[ZOOM_STEPS.len() - 1]);
const STEP_EPSILON: f32 = 1e-3;
pub const MAX_IMAGE_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RasterFormat {
    Png,
    Jpeg,
    Gif,
    Webp,
    Bmp,
    Ico,
    Tiff,
}

impl RasterFormat {
    pub fn label(self) -> &'static str {
        match self {
            RasterFormat::Png => "PNG",
            RasterFormat::Jpeg => "JPEG",
            RasterFormat::Gif => "GIF",
            RasterFormat::Webp => "WebP",
            RasterFormat::Bmp => "BMP",
            RasterFormat::Ico => "ICO",
            RasterFormat::Tiff => "TIFF",
        }
    }

    pub fn gpui(self) -> gpui_kit::ImageFormat {
        match self {
            RasterFormat::Png => gpui_kit::ImageFormat::Png,
            RasterFormat::Jpeg => gpui_kit::ImageFormat::Jpeg,
            RasterFormat::Gif => gpui_kit::ImageFormat::Gif,
            RasterFormat::Webp => gpui_kit::ImageFormat::Webp,
            RasterFormat::Bmp => gpui_kit::ImageFormat::Bmp,
            RasterFormat::Ico => gpui_kit::ImageFormat::Ico,
            RasterFormat::Tiff => gpui_kit::ImageFormat::Tiff,
        }
    }

    pub fn decoder(self) -> image::ImageFormat {
        match self {
            RasterFormat::Png => image::ImageFormat::Png,
            RasterFormat::Jpeg => image::ImageFormat::Jpeg,
            RasterFormat::Gif => image::ImageFormat::Gif,
            RasterFormat::Webp => image::ImageFormat::WebP,
            RasterFormat::Bmp => image::ImageFormat::Bmp,
            RasterFormat::Ico => image::ImageFormat::Ico,
            RasterFormat::Tiff => image::ImageFormat::Tiff,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    Text,
    Svg,
    Markdown,
    Raster(RasterFormat),
}

pub fn file_kind(path: &Path) -> FileKind {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else { return FileKind::Text };
    match ext.to_ascii_lowercase().as_str() {
        "svg" => FileKind::Svg,
        "md" | "markdown" | "mdx" => FileKind::Markdown,
        "png" => FileKind::Raster(RasterFormat::Png),
        "jpg" | "jpeg" => FileKind::Raster(RasterFormat::Jpeg),
        "gif" => FileKind::Raster(RasterFormat::Gif),
        "webp" => FileKind::Raster(RasterFormat::Webp),
        "bmp" => FileKind::Raster(RasterFormat::Bmp),
        "ico" => FileKind::Raster(RasterFormat::Ico),
        "tif" | "tiff" => FileKind::Raster(RasterFormat::Tiff),
        _ => FileKind::Text,
    }
}

pub fn clamp_fraction(fraction: f32) -> f32 {
    if fraction.is_finite() {
        fraction.clamp(FRACTION_RANGE.0, FRACTION_RANGE.1)
    } else {
        DEFAULT_FRACTION
    }
}

pub fn fraction_for_pointer(pointer_x: f32, area_left: f32, area_width: f32) -> f32 {
    if area_width <= 1. {
        return DEFAULT_FRACTION;
    }
    clamp_fraction((area_left + area_width - pointer_x) / area_width)
}

#[derive(Default)]
pub struct Debounce {
    generation: u64,
}

impl Debounce {
    pub fn next(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.generation
    }

    pub fn is_latest(&self, generation: u64) -> bool {
        self.generation == generation
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Zoom {
    Fit,
    Scale(f32),
}

pub fn fit_scale(natural: (f32, f32), pane: (f32, f32)) -> f32 {
    if natural.0 <= 0. || natural.1 <= 0. || pane.0 <= 0. || pane.1 <= 0. {
        return 1.;
    }
    (pane.0 / natural.0).min(pane.1 / natural.1).min(1.)
}

pub fn clamp_scale(scale: f32) -> f32 {
    if scale.is_finite() { scale.clamp(ZOOM_RANGE.0, ZOOM_RANGE.1) } else { 1. }
}

pub fn zoom_in(scale: f32) -> f32 {
    ZOOM_STEPS.iter().copied().find(|&step| step > scale + STEP_EPSILON).unwrap_or(ZOOM_RANGE.1)
}

pub fn zoom_out(scale: f32) -> f32 {
    ZOOM_STEPS.iter().rev().copied().find(|&step| step < scale - STEP_EPSILON).unwrap_or(ZOOM_RANGE.0)
}

pub fn zoom_label(zoom: Zoom, scale: f32) -> String {
    match zoom {
        Zoom::Fit => "Whole Image".to_string(),
        Zoom::Scale(_) => format!("{}%", (scale * 100.).round() as i64),
    }
}

pub fn clamp_scroll(scroll: f32, content: f32, pane: f32) -> f32 {
    if content <= pane || !scroll.is_finite() { 0. } else { scroll.clamp(0., content - pane) }
}

pub fn image_origin(content: f32, pane: f32, scroll: f32) -> f32 {
    if content <= pane { (pane - content) / 2. } else { -clamp_scroll(scroll, content, pane) }
}

pub fn anchored_scroll(pointer: f32, old_content: f32, new_content: f32, pane: f32, scroll: f32) -> f32 {
    let origin = image_origin(old_content, pane, scroll);
    let fraction = if old_content > 0. { ((pointer - origin) / old_content).clamp(0., 1.) } else { 0.5 };
    clamp_scroll(fraction * new_content - pointer, new_content, pane)
}

pub fn format_dimensions(width: u32, height: u32) -> String {
    format!("{width}×{height}")
}

pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024. && unit < UNITS.len() - 1 {
        value /= 1024.;
        unit += 1;
    }
    if unit == 0 { format!("{bytes} B") } else { format!("{value:.1} {}", UNITS[unit]) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_file_kinds_case_insensitively() {
        assert_eq!(file_kind(Path::new("a/logo.SVG")), FileKind::Svg);
        assert_eq!(file_kind(Path::new("README.md")), FileKind::Markdown);
        assert_eq!(file_kind(Path::new("x.markdown")), FileKind::Markdown);
        assert_eq!(file_kind(Path::new("x.mdx")), FileKind::Markdown);
        assert_eq!(file_kind(Path::new("a.PNG")), FileKind::Raster(RasterFormat::Png));
        assert_eq!(file_kind(Path::new("a.jpeg")), FileKind::Raster(RasterFormat::Jpeg));
        assert_eq!(file_kind(Path::new("a.jpg")), FileKind::Raster(RasterFormat::Jpeg));
        assert_eq!(file_kind(Path::new("a.tif")), FileKind::Raster(RasterFormat::Tiff));
        assert_eq!(file_kind(Path::new("a.heic")), FileKind::Text);
        assert_eq!(file_kind(Path::new("Makefile")), FileKind::Text);
        assert_eq!(file_kind(Path::new("a.rs")), FileKind::Text);
    }

    #[test]
    fn fractions_are_clamped() {
        assert_eq!(clamp_fraction(0.0), FRACTION_RANGE.0);
        assert_eq!(clamp_fraction(2.0), FRACTION_RANGE.1);
        assert_eq!(clamp_fraction(f32::NAN), DEFAULT_FRACTION);
        assert_eq!(clamp_fraction(0.4), 0.4);
    }

    #[test]
    fn pointer_maps_to_preview_share_of_the_right_side() {
        assert_eq!(fraction_for_pointer(500., 0., 1000.), 0.5);
        assert_eq!(fraction_for_pointer(700., 0., 1000.), 0.3);
        assert_eq!(fraction_for_pointer(0., 0., 1000.), FRACTION_RANGE.1);
        assert_eq!(fraction_for_pointer(1000., 0., 1000.), FRACTION_RANGE.0);
        assert_eq!(fraction_for_pointer(300., 100., 0.), DEFAULT_FRACTION);
    }

    #[test]
    fn debounce_only_lets_the_latest_generation_through() {
        let mut debounce = Debounce::default();
        let first = debounce.next();
        let second = debounce.next();
        assert!(!debounce.is_latest(first));
        assert!(debounce.is_latest(second));
        let third = debounce.next();
        assert!(!debounce.is_latest(second));
        assert!(debounce.is_latest(third));
    }

    #[test]
    fn fit_never_upscales_and_keeps_aspect() {
        assert_eq!(fit_scale((100., 100.), (400., 400.)), 1.);
        assert_eq!(fit_scale((1000., 500.), (500., 500.)), 0.5);
        assert_eq!(fit_scale((500., 1000.), (500., 500.)), 0.5);
        assert_eq!(fit_scale((0., 10.), (500., 500.)), 1.);
    }

    #[test]
    fn zoom_steps_stay_in_range() {
        assert!(zoom_in(1.) > 1.);
        assert!(zoom_out(1.) < 1.);
        assert_eq!(zoom_in(ZOOM_RANGE.1), ZOOM_RANGE.1);
        assert_eq!(zoom_out(ZOOM_RANGE.0), ZOOM_RANGE.0);
        assert_eq!(clamp_scale(f32::INFINITY), 1.);
    }

    #[test]
    fn zoom_steps_follow_vscode() {
        assert_eq!(zoom_in(1.), 1.5);
        assert_eq!(zoom_in(1.2), 1.5);
        assert_eq!(zoom_in(0.05), 0.1);
        assert_eq!(zoom_in(15.), 20.);
        assert_eq!(zoom_out(1.), 0.9);
        assert_eq!(zoom_out(1.2), 1.);
        assert_eq!(zoom_out(0.37), 0.3);
        assert_eq!(zoom_out(0.1), 0.1);
        let mut scale = 0.1;
        let mut seen = vec![scale];
        while scale < ZOOM_RANGE.1 {
            scale = zoom_in(scale);
            seen.push(scale);
        }
        assert_eq!(seen, ZOOM_STEPS.to_vec());
    }

    #[test]
    fn zoom_labels() {
        assert_eq!(zoom_label(Zoom::Fit, 0.42), "Whole Image");
        assert_eq!(zoom_label(Zoom::Scale(1.5), 1.5), "150%");
        assert_eq!(zoom_label(Zoom::Scale(0.1), 0.1), "10%");
        assert_eq!(format_dimensions(1024, 768), "1024×768");
    }

    #[test]
    fn small_images_center_and_large_images_scroll() {
        assert_eq!(image_origin(100., 400., 50.), 150.);
        assert_eq!(image_origin(1000., 400., 50.), -50.);
        assert_eq!(image_origin(1000., 400., 900.), -600.);
        assert_eq!(clamp_scroll(-5., 1000., 400.), 0.);
        assert_eq!(clamp_scroll(f32::NAN, 1000., 400.), 0.);
    }

    #[test]
    fn zoom_keeps_the_point_under_the_pointer() {
        let scroll = anchored_scroll(200., 400., 800., 400., 0.);
        assert_eq!(scroll, 200.);
        assert_eq!(image_origin(800., 400., scroll) + 0.5 * 800., 200.);
        let scroll = anchored_scroll(100., 1000., 2000., 400., 300.);
        assert_eq!(scroll, 700.);
        assert_eq!(anchored_scroll(100., 1000., 200., 400., 300.), 0.);
    }

    #[test]
    fn formats_byte_sizes() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1023), "1023 B");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(5 * 1024 * 1024), "5.0 MB");
    }
}
