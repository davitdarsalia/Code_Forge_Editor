use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gpui_kit::{
    App, BoxShadow, Bounds, ContentMask, Context, Corners, Hsla, IntoElement, Pixels, Render, Rgba,
    Styled as _, Window, canvas, div, fill, linear_color_stop, linear_gradient, point, px, size,
};

pub const HOLD: Duration = Duration::from_millis(1500);
pub const FADE_IN: Duration = Duration::from_millis(220);
pub const FADE_OUT: Duration = Duration::from_millis(500);
const ROTATION: Duration = Duration::from_millis(4200);
const PULSE: Duration = Duration::from_millis(1800);
const LINE: f32 = 2.5;
const SEGMENT: f32 = 22.;
const GLOW_EVERY: usize = 3;
const GLOW_BLUR: f32 = 22.;
const GLOW_SPREAD: f32 = 3.;
const STOPS: [(f32, f32, f32); 5] = [
    (0.31, 0.55, 1.0),
    (0.55, 0.36, 0.96),
    (0.93, 0.29, 0.60),
    (1.0, 0.55, 0.24),
    (0.98, 0.42, 0.70),
];

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EditPulse {
    started: Option<Instant>,
    last: Option<Instant>,
}

impl EditPulse {
    pub fn note(&mut self, now: Instant) {
        if !self.is_active(now) {
            self.started = Some(now);
        }
        self.last = Some(now);
    }

    pub fn is_active(&self, now: Instant) -> bool {
        self.last.is_some_and(|last| now.saturating_duration_since(last) < HOLD + FADE_OUT)
    }

    pub fn intensity(&self, now: Instant) -> f32 {
        let (Some(started), Some(last)) = (self.started, self.last) else { return 0. };
        let rise = (now.saturating_duration_since(started).as_secs_f32() / FADE_IN.as_secs_f32()).min(1.);
        let since = now.saturating_duration_since(last);
        if since <= HOLD {
            return ease(rise);
        }
        let fall = (since - HOLD).as_secs_f32() / FADE_OUT.as_secs_f32();
        if fall >= 1. { 0. } else { ease(rise) * (1. - ease(fall)) }
    }

    pub fn clear(&mut self) {
        *self = EditPulse::default();
    }
}

fn ease(t: f32) -> f32 {
    let t = t.clamp(0., 1.);
    t * t * (3. - 2. * t)
}

pub fn palette(t: f32) -> (f32, f32, f32) {
    let t = t.rem_euclid(1.) * STOPS.len() as f32;
    let ix = t.floor() as usize % STOPS.len();
    let next = (ix + 1) % STOPS.len();
    let f = t - t.floor();
    let (a, b) = (STOPS[ix], STOPS[next]);
    (a.0 + (b.0 - a.0) * f, a.1 + (b.1 - a.1) * f, a.2 + (b.2 - a.2) * f)
}

fn color(t: f32, alpha: f32) -> Hsla {
    let (r, g, b) = palette(t);
    Rgba { r, g, b, a: alpha.clamp(0., 1.) }.into()
}

struct Segment {
    bounds: Bounds<Pixels>,
    angle: f32,
    from: f32,
    to: f32,
}

fn segments(bounds: Bounds<Pixels>) -> Vec<Segment> {
    let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    if w < 4. || h < 4. {
        return Vec::new();
    }
    let (x0, y0) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
    let total = 2. * (w + h);
    let mut out = Vec::new();
    let mut walk = |start: f32, length: f32, angle: f32, rect: &dyn Fn(f32, f32) -> Bounds<Pixels>| {
        let count = (length / SEGMENT).ceil().max(1.) as usize;
        let step = length / count as f32;
        for i in 0..count {
            let a = i as f32 * step;
            out.push(Segment {
                bounds: rect(a, step),
                angle,
                from: (start + a) / total,
                to: (start + a + step) / total,
            });
        }
    };
    let quad = |x: f32, y: f32, w: f32, h: f32| Bounds { origin: point(px(x), px(y)), size: size(px(w), px(h)) };
    walk(0., w, 90., &|a, s| quad(x0 + a, y0, s, LINE));
    walk(w, h, 180., &|a, s| quad(x0 + w - LINE, y0 + a, LINE, s));
    walk(w + h, w, 270., &|a, s| quad(x0 + w - a - s, y0 + h - LINE, s, LINE));
    walk(2. * w + h, h, 0., &|a, s| quad(x0, y0 + h - a - s, LINE, s));
    out
}

fn paint(bounds: Bounds<Pixels>, phase: f32, strength: f32, window: &mut Window) {
    if strength <= 0.001 {
        return;
    }
    let parts = segments(bounds);
    window.with_content_mask(Some(ContentMask { bounds }), |window| {
        for (ix, part) in parts.iter().enumerate() {
            if ix % GLOW_EVERY != 0 {
                continue;
            }
            let shadow = BoxShadow::new(px(0.), px(0.), color(part.from + phase, 0.55 * strength))
                .blur_radius(px(GLOW_BLUR))
                .spread_radius(px(GLOW_SPREAD));
            window.paint_drop_shadows(part.bounds, Corners::default(), &[shadow]);
        }
        for part in &parts {
            let gradient = linear_gradient(
                part.angle,
                linear_color_stop(color(part.from + phase, strength), 0.),
                linear_color_stop(color(part.to + phase, strength), 1.),
            );
            window.paint_quad(fill(part.bounds, gradient));
        }
    });
}

pub fn line_tint() -> Hsla {
    Rgba { r: 0.66, g: 0.33, b: 0.97, a: 0.16 }.into()
}

pub fn sparkle_dot() -> impl IntoElement {
    let (r, g, b) = palette(0.6);
    div()
        .size(px(7.))
        .flex_none()
        .rounded_full()
        .bg(linear_gradient(
            135.,
            linear_color_stop(color(0., 1.), 0.),
            linear_color_stop(color(0.5, 1.), 1.),
        ))
        .shadow(vec![BoxShadow::new(px(0.), px(0.), Rgba { r, g, b, a: 0.7 }.into()).blur_radius(px(5.))])
}

pub struct AgentGlow {
    path: Option<PathBuf>,
    pulse: EditPulse,
    epoch: Instant,
}

impl AgentGlow {
    pub fn new() -> Self {
        AgentGlow { path: None, pulse: EditPulse::default(), epoch: Instant::now() }
    }

    pub fn note(&mut self, path: &Path, cx: &mut Context<Self>) {
        let now = Instant::now();
        if self.path.as_deref() != Some(path) {
            self.pulse.clear();
            self.path = Some(path.to_path_buf());
        }
        self.pulse.note(now);
        cx.notify();
    }

    pub fn is_showing(&self, path: &Path) -> bool {
        self.path.as_deref() == Some(path) && self.pulse.is_active(Instant::now())
    }
}

impl Render for AgentGlow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = Instant::now();
        if !self.pulse.is_active(now) {
            if self.path.take().is_some() {
                self.pulse.clear();
            }
            return div().into_any_element();
        }
        window.request_animation_frame();
        let elapsed = now.duration_since(self.epoch).as_secs_f32();
        let still = still_motion(cx);
        let phase = if still { 0. } else { elapsed / ROTATION.as_secs_f32() };
        let breathe = if still { 1. } else { 0.82 + 0.18 * (elapsed * std::f32::consts::TAU / PULSE.as_secs_f32()).sin() };
        let strength = self.pulse.intensity(now) * breathe;
        canvas(|_, _, _| {}, move |bounds, _, window, _| paint(bounds, phase, strength, window))
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .into_any_element()
    }
}

fn still_motion(cx: &App) -> bool {
    cx.reduce_motion()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_pulse_is_inactive_and_dark() {
        let pulse = EditPulse::default();
        let now = Instant::now();
        assert!(!pulse.is_active(now));
        assert_eq!(pulse.intensity(now), 0.);
    }

    #[test]
    fn pulse_fades_in_holds_then_fades_out() {
        let t0 = Instant::now();
        let mut pulse = EditPulse::default();
        pulse.note(t0);
        assert_eq!(pulse.intensity(t0), 0.);
        assert!(pulse.intensity(t0 + FADE_IN / 2) > 0.2);
        assert_eq!(pulse.intensity(t0 + FADE_IN), 1.);
        assert_eq!(pulse.intensity(t0 + HOLD), 1.);
        let midway = pulse.intensity(t0 + HOLD + FADE_OUT / 2);
        assert!(midway > 0.2 && midway < 0.8);
        assert!(pulse.is_active(t0 + HOLD + FADE_OUT - Duration::from_millis(1)));
        assert!(!pulse.is_active(t0 + HOLD + FADE_OUT));
        assert_eq!(pulse.intensity(t0 + HOLD + FADE_OUT), 0.);
    }

    #[test]
    fn new_edits_extend_the_hold_without_restarting_the_fade_in() {
        let t0 = Instant::now();
        let mut pulse = EditPulse::default();
        pulse.note(t0);
        let t1 = t0 + Duration::from_millis(1200);
        pulse.note(t1);
        assert_eq!(pulse.intensity(t1), 1.);
        assert_eq!(pulse.intensity(t0 + HOLD + FADE_OUT), 1.);
        assert!(pulse.is_active(t1 + HOLD));
        assert!(!pulse.is_active(t1 + HOLD + FADE_OUT));
    }

    #[test]
    fn a_late_edit_restarts_the_fade_in() {
        let t0 = Instant::now();
        let mut pulse = EditPulse::default();
        pulse.note(t0);
        let later = t0 + HOLD + FADE_OUT + Duration::from_millis(10);
        pulse.note(later);
        assert_eq!(pulse.intensity(later), 0.);
        assert_eq!(pulse.intensity(later + FADE_IN), 1.);
    }

    #[test]
    fn palette_cycles_smoothly() {
        assert_eq!(palette(0.), STOPS[0]);
        assert_eq!(palette(1.), STOPS[0]);
        assert_eq!(palette(-0.8), palette(0.2));
        let (r, _, _) = palette(0.1);
        assert!(r > STOPS[0].0 && r < STOPS[1].0);
    }

    #[test]
    fn border_segments_cover_the_perimeter() {
        let bounds = Bounds { origin: point(px(10.), px(20.)), size: size(px(200.), px(100.)) };
        let parts = segments(bounds);
        let length: f32 = parts.iter().map(|p| f32::from(p.bounds.size.width.max(p.bounds.size.height))).sum();
        assert!((length - 600.).abs() < 0.5);
        assert_eq!(parts.first().map(|p| p.from), Some(0.));
        assert!((parts.last().unwrap().to - 1.).abs() < 1e-4);
        assert!(segments(Bounds { origin: point(px(0.), px(0.)), size: size(px(2.), px(100.)) }).is_empty());
    }
}
