//! TO-DO PATCH: hooks the to-do desktop app needs from the platform layer.
//!
//! Everything the vendored copy adds on top of upstream is marked `TO-DO PATCH` in the sources.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;

/// `f32::to_bits` of the forced scale factor; zero means "use the monitor's DPI".
static SCALE_FACTOR_OVERRIDE: AtomicU32 = AtomicU32::new(0);

/// Forces the scale factor of every window opened afterwards, ignoring the monitor's DPI.
///
/// The `--screenshot` mode uses it to render 150 % galleries on a 100 % monitor and in CI.
/// `None` (or a non-positive value) restores the default behaviour.
pub fn set_scale_factor_override(scale: Option<f32>) {
    let bits = match scale {
        Some(s) if s.is_finite() && s > 0.0 => s.to_bits(),
        _ => 0,
    };
    SCALE_FACTOR_OVERRIDE.store(bits, Ordering::Relaxed);
}

/// The forced scale factor, if any.
pub fn scale_factor_override() -> Option<f32> {
    match SCALE_FACTOR_OVERRIDE.load(Ordering::Relaxed) {
        0 => None,
        bits => Some(f32::from_bits(bits)),
    }
}

enum Capture {
    Pending,
    Done(anyhow::Result<image::RgbaImage>),
}

/// Capture slots keyed by `HWND`.
static CAPTURES: Mutex<Option<HashMap<isize, Capture>>> = Mutex::new(None);

fn with_captures<R>(f: impl FnOnce(&mut HashMap<isize, Capture>) -> R) -> R {
    let mut guard = CAPTURES.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    f(guard.get_or_insert_with(HashMap::new))
}

/// Asks the renderer to read back the next frame it draws for `hwnd`, before presenting it.
///
/// The caller then makes the window draw a frame and collects the image with [`take_capture`].
pub fn request_capture(hwnd: isize) {
    with_captures(|c| {
        c.insert(hwnd, Capture::Pending);
    });
}

/// The captured frame for `hwnd`, once the renderer has drawn one since [`request_capture`].
pub fn take_capture(hwnd: isize) -> Option<anyhow::Result<image::RgbaImage>> {
    with_captures(|c| match c.remove(&hwnd) {
        Some(Capture::Done(image)) => Some(image),
        Some(pending @ Capture::Pending) => {
            c.insert(hwnd, pending);
            None
        }
        None => None,
    })
}

pub(crate) fn capture_requested(hwnd: isize) -> bool {
    with_captures(|c| matches!(c.get(&hwnd), Some(Capture::Pending)))
}

pub(crate) fn deliver_capture(hwnd: isize, image: anyhow::Result<image::RgbaImage>) {
    with_captures(|c| {
        c.insert(hwnd, Capture::Done(image));
    });
}

/// When each window presented its first and its latest frame, keyed by `HWND`.
static PRESENTS: Mutex<Option<HashMap<isize, (Instant, Instant)>>> = Mutex::new(None);

pub(crate) fn record_present(hwnd: isize) {
    let now = Instant::now();
    let mut guard = PRESENTS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    guard.get_or_insert_with(HashMap::new).entry(hwnd).and_modify(|(_, last)| *last = now).or_insert((now, now));
}

/// When `hwnd` presented its first frame, if it has.
pub fn first_present(hwnd: isize) -> Option<Instant> {
    let guard = PRESENTS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    guard.as_ref()?.get(&hwnd).map(|(first, _)| *first)
}

/// When `hwnd` presented its latest frame, if it has.
pub fn last_present(hwnd: isize) -> Option<Instant> {
    let guard = PRESENTS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    guard.as_ref()?.get(&hwnd).map(|(_, last)| *last)
}
