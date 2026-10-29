use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

static START: OnceLock<Instant> = OnceLock::new();
static ENABLED: AtomicBool = AtomicBool::new(false);

pub fn init() {
    START.get_or_init(Instant::now);
    let on = std::env::var_os("FORGE_TRACE_STARTUP").is_some_and(|v| v != "0" && !v.is_empty());
    ENABLED.store(on, Ordering::Relaxed);
    mark("main");
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

pub fn mark(label: &str) {
    if !enabled() {
        return;
    }
    let Some(start) = START.get() else { return };
    let ms = start.elapsed().as_secs_f64() * 1000.;
    eprintln!("forge-startup: {ms:8.1} ms  {label}");
}

pub fn mark_once(flag: &AtomicBool, label: &str) {
    if enabled() && !flag.swap(true, Ordering::Relaxed) {
        mark(label);
    }
}
