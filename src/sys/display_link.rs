use std::ffi::c_void;
use std::ptr;
use std::sync::Arc;

use parking_lot::Mutex;

pub type CVReturn = i32;
pub type CVOptionFlags = u32;
#[allow(non_camel_case_types)]
pub type CVDisplayLinkRef = *mut c_void;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct CVTimeStamp {
    pub version: u32,
    pub video_time_scale: i32,
    pub video_time: i64,
    pub host_time: u64,
    pub rate_scalar: f64,
    pub video_refresh_period: u64,
    pub smpte_time: u64,
    pub flags: u64,
    pub reserved: u64,
}

// display_link has bindings in its own file because (1) it is CV not sls (2) i like it to be segmented away
unsafe extern "C" {
    fn CVDisplayLinkCreateWithActiveCGDisplays(link: *mut CVDisplayLinkRef) -> CVReturn;
    fn CVDisplayLinkSetOutputCallback(
        link: CVDisplayLinkRef,
        callback: extern "C" fn(
            CVDisplayLinkRef,
            *const CVTimeStamp,
            *const CVTimeStamp,
            CVOptionFlags,
            *mut CVOptionFlags,
            *mut c_void,
        ) -> CVReturn,
        user_info: *mut c_void,
    ) -> CVReturn;
    fn CVDisplayLinkStart(link: CVDisplayLinkRef) -> CVReturn;
    fn CVDisplayLinkStop(link: CVDisplayLinkRef) -> CVReturn;
    fn CVDisplayLinkRelease(link: CVDisplayLinkRef);
}

struct CallbackData {
    callback: Box<dyn FnMut() -> bool + Send>,
    refresh_rate: Arc<Mutex<Option<f64>>>,
}

extern "C" fn display_link_callback(
    link: CVDisplayLinkRef,
    _now: *const CVTimeStamp,
    output: *const CVTimeStamp,
    _flags_in: CVOptionFlags,
    _flags_out: *mut CVOptionFlags,
    user_info: *mut c_void,
) -> CVReturn {
    if user_info.is_null() {
        return 0;
    }

    let data = unsafe { &mut *(user_info as *mut CallbackData) };

    if !output.is_null() {
        let timestamp = unsafe { &*output };
        if timestamp.video_refresh_period > 0 && timestamp.video_time_scale > 0 {
            let refresh_rate =
                timestamp.video_time_scale as f64 / timestamp.video_refresh_period as f64;
            let mut rate = data.refresh_rate.lock();
            *rate = Some(refresh_rate);
        }
    }

    let keep_running = (data.callback)();
    if !keep_running {
        unsafe { CVDisplayLinkStop(link) };
    }
    0
}

pub struct DisplayLink {
    link: CVDisplayLinkRef,
    cb_ptr: *mut CallbackData,
    refresh_rate: Arc<Mutex<Option<f64>>>,
}

impl DisplayLink {
    pub fn new<F>(callback: F) -> Result<Self, CVReturn>
    where
        F: FnMut() -> bool + Send + 'static,
    {
        let mut link: CVDisplayLinkRef = ptr::null_mut();
        let status = unsafe { CVDisplayLinkCreateWithActiveCGDisplays(&mut link) };
        if status != 0 {
            return Err(status);
        }

        let refresh_rate = Arc::new(Mutex::new(None));
        let callback_data = CallbackData {
            callback: Box::new(callback),
            refresh_rate: refresh_rate.clone(),
        };
        let cb_ptr = Box::into_raw(Box::new(callback_data));

        let status = unsafe {
            CVDisplayLinkSetOutputCallback(link, display_link_callback, cb_ptr as *mut c_void)
        };
        if status != 0 {
            unsafe {
                CVDisplayLinkRelease(link);
                let _ = Box::from_raw(cb_ptr);
            }
            return Err(status);
        }

        Ok(DisplayLink { link, cb_ptr, refresh_rate })
    }

    pub fn start(&self) {
        unsafe {
            CVDisplayLinkStart(self.link);
        }
    }

    pub fn stop(&self) {
        unsafe {
            CVDisplayLinkStop(self.link);
        }
    }

    /// Get the display's refresh rate in Hz (frames per second).
    /// Returns None if the refresh rate hasn't been determined yet.
    /// You may need to start the DisplayLink briefly to get this information.
    pub fn refresh_rate(&self) -> Option<f64> {
        *self.refresh_rate.lock()
    }

    /// Get the display's refresh rate, starting the DisplayLink briefly if needed.
    /// This is a convenience method that will start the DisplayLink for a short time
    /// to determine the refresh rate, then stop it.
    pub fn get_refresh_rate(&self) -> Option<f64> {
        if let Some(rate) = self.refresh_rate() {
            return Some(rate);
        }

        self.start();

        std::thread::sleep(std::time::Duration::from_millis(20));

        let rate = self.refresh_rate();
        self.stop();

        rate
    }
}

impl Drop for DisplayLink {
    fn drop(&mut self) {
        unsafe {
            CVDisplayLinkStop(self.link);
            CVDisplayLinkRelease(self.link);
            drop(Box::from_raw(self.cb_ptr));
        }
    }
}

/// Get the display's refresh rate in Hz without creating a persistent DisplayLink.
/// This is a convenience function for one-off refresh rate queries.
/// Returns None if the refresh rate cannot be determined.
pub fn get_display_refresh_rate() -> Option<f64> {
    let link = DisplayLink::new(|| false).ok()?;
    link.get_refresh_rate()
}
