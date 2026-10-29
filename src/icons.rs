use std::sync::atomic::{AtomicU32, Ordering};

use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::{Hsla, Pixels, Styled as _, px};

pub const DEFAULT_SCALE: f32 = 1.2;
pub const SCALE_RANGE: (f32, f32) = (0.8, 2.0);

static SCALE_BITS: AtomicU32 = AtomicU32::new(DEFAULT_SCALE.to_bits());

pub fn scale() -> f32 {
    f32::from_bits(SCALE_BITS.load(Ordering::Relaxed))
}

pub fn set_scale(scale: f32) {
    SCALE_BITS.store(scale.clamp(SCALE_RANGE.0, SCALE_RANGE.1).to_bits(), Ordering::Relaxed);
}

pub fn scaled(base: f32) -> Pixels {
    px((base * scale()).round())
}

pub fn ui_icon(name: IconName, base: f32, color: Hsla) -> Icon {
    Icon::new(name).size(scaled(base)).text_color(color)
}

pub fn row_height(base_row: f32, base_icon: f32) -> f32 {
    base_row.max((base_icon * scale()).round() + 6.)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_is_clamped_and_rows_grow() {
        set_scale(5.);
        assert_eq!(scale(), SCALE_RANGE.1);
        assert_eq!(row_height(22., 14.), 34.);
        set_scale(1.);
        assert_eq!(row_height(22., 14.), 22.);
        set_scale(DEFAULT_SCALE);
    }
}
