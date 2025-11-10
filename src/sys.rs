//! Utilities for interfacing with OS-specific APIs.

use objc2_core_graphics::CGError;

pub mod accessibility;
pub mod app;
pub mod axuielement;
pub mod carbon;

pub mod cgs_window;
pub mod dispatch;
pub mod display_link;
pub mod enhanced_ui;
pub mod event;
pub mod event_tap;
pub mod executor;
pub mod geometry;
pub mod haptics;
pub mod hotkey;
pub mod mach;
pub mod observer;
pub mod power;
pub mod process;
pub mod run_loop;
pub mod screen;
pub mod service;
pub mod skylight;
pub mod timer;
pub mod window_notify;
pub mod window_server;

#[inline(always)]
pub fn cg_ok(err: CGError) -> Result<(), CGError> {
    if err == CGError::Success {
        Ok(())
    } else {
        Err(err)
    }
}
