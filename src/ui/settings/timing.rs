//! Opt-in, process-local Settings startup measurements.
use std::time::Instant;

#[derive(Clone, Copy)]
pub struct Start {
    wall: Instant,
    cpu: std::time::Duration,
}

fn cpu_time() -> std::time::Duration {
    let mut time = nix::libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: time points to valid storage; this clock measures the calling thread.
    unsafe {
        nix::libc::clock_gettime(nix::libc::CLOCK_THREAD_CPUTIME_ID, &mut time);
    }
    std::time::Duration::new(time.tv_sec as u64, time.tv_nsec as u32)
}

pub fn start() -> Option<Start> {
    (std::env::var_os("RIFT_SETTINGS_STARTUP_TIMING").as_deref() == Some(std::ffi::OsStr::new("1")))
        .then(|| Start {
            wall: Instant::now(),
            cpu: cpu_time(),
        })
}

pub fn finish(start: Option<Start>, stage: &str) {
    if let Some(start) = start {
        eprintln!(
            "settings-timing pid={} {stage}: {:.3} ms cpu={:.3} ms",
            std::process::id(),
            start.wall.elapsed().as_secs_f64() * 1000.0,
            cpu_time().saturating_sub(start.cpu).as_secs_f64() * 1000.0
        );
    }
}
