use std::sync::Mutex;
use std::time::Instant;
use windows_sys::Win32::Foundation::FILETIME;
use windows_sys::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows_sys::Win32::System::SystemInformation::GetSystemTimePreciseAsFileTime;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

static LINES: Mutex<Vec<String>> = Mutex::new(Vec::new());
static PENDING_IN: Mutex<Option<Instant>> = Mutex::new(None);
static PENDING_E2E: Mutex<Option<Instant>> = Mutex::new(None);
static LAT_IN: Mutex<Vec<f64>> = Mutex::new(Vec::new());
static LAT_E2E: Mutex<Vec<f64>> = Mutex::new(Vec::new());

fn ft(f: FILETIME) -> u64 {
    ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64
}

/// Milliseconds since this process was created (kernel creation time).
pub fn process_age_ms() -> f64 {
    unsafe {
        let (mut c, mut e, mut k, mut u) = (std::mem::zeroed(), std::mem::zeroed(), std::mem::zeroed(), std::mem::zeroed());
        GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u);
        let mut now: FILETIME = std::mem::zeroed();
        GetSystemTimePreciseAsFileTime(&mut now);
        (ft(now).saturating_sub(ft(c))) as f64 / 10_000.0
    }
}

pub fn working_set_mb() -> (f64, f64, f64) {
    unsafe {
        let mut m: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
        m.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        K32GetProcessMemoryInfo(GetCurrentProcess(), &mut m, m.cb);
        let mb = 1024.0 * 1024.0;
        (m.WorkingSetSize as f64 / mb, m.PeakWorkingSetSize as f64 / mb, m.PagefileUsage as f64 / mb)
    }
}

pub fn log(key: &str, val: impl std::fmt::Display) {
    let line = format!("{key} = {val}");
    println!("{line}");
    LINES.lock().unwrap().push(line);
}

pub fn logf(key: &str, ms: f64) {
    log(key, format!("{ms:.2}"));
}

pub fn mark_input() {
    *PENDING_IN.lock().unwrap() = Some(Instant::now());
}
pub fn mark_e2e() {
    *PENDING_E2E.lock().unwrap() = Some(Instant::now());
}
pub fn input_painted() {
    if let Some(t) = PENDING_IN.lock().unwrap().take() {
        LAT_IN.lock().unwrap().push(t.elapsed().as_secs_f64() * 1000.0);
    }
    if let Some(t) = PENDING_E2E.lock().unwrap().take() {
        LAT_E2E.lock().unwrap().push(t.elapsed().as_secs_f64() * 1000.0);
    }
}

pub fn stats(name: &str, mut v: Vec<f64>) {
    if v.is_empty() {
        log(name, "no samples");
        return;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p = |q: f64| v[((v.len() as f64 - 1.0) * q).round() as usize];
    log(
        name,
        format!("n={} min={:.2} median={:.2} p95={:.2} max={:.2}", v.len(), v[0], p(0.5), p(0.95), v[v.len() - 1]),
    );
}

pub fn typing_report() {
    stats("typing_handler_to_paint_ms", LAT_IN.lock().unwrap().clone());
    stats("typing_sendinput_to_paint_ms", LAT_E2E.lock().unwrap().clone());
}

pub fn flush(path: &str) {
    let s = LINES.lock().unwrap().join("\n") + "\n";
    let _ = std::fs::write(path, s);
}
