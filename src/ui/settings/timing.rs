//! Opt-in Settings phase measurements and parent-to-visible startup latency.
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

// CLOCK_MONOTONIC has the same origin in the daemon and child; Instant is
// deliberately process-local. Pass this only on timing-enabled launches.
pub fn spawn_time_ns() -> u64 {
    let mut time = nix::libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: time points to valid storage.
    unsafe {
        nix::libc::clock_gettime(nix::libc::CLOCK_MONOTONIC, &mut time);
    }
    time.tv_sec as u64 * 1_000_000_000 + time.tv_nsec as u64
}

pub fn finish_from_parent(stage: &str) {
    if start().is_none() {
        return;
    }
    if let Some(parent) = std::env::var("RIFT_SETTINGS_SPAWN_TIME_NS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
    {
        eprintln!(
            "settings-timing pid={} parent spawn to {stage}: {:.3} ms",
            std::process::id(),
            spawn_time_ns().saturating_sub(parent) as f64 / 1_000_000.0
        );
    }
}
