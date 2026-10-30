use std::path::Path;
use std::time::Duration;

pub const PREVIEW_DEBOUNCE: Duration = Duration::from_millis(150);
pub const DEFAULT_FRACTION: f32 = 0.5;
pub const FRACTION_RANGE: (f32, f32) = (0.15, 0.85);
pub const ZOOM_RANGE: (f32, f32) = (0.02, 32.);
pub const ZOOM_STEP: f32 = 1.25;
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
    clamp_scale(scale * ZOOM_STEP)
}

pub fn zoom_out(scale: f32) -> f32 {
    clamp_scale(scale / ZOOM_STEP)
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
    fn formats_byte_sizes() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1023), "1023 B");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(5 * 1024 * 1024), "5.0 MB");
    }
}
