use accessibility::AXUIElement;
use objc2_core_foundation::CGRect;

use super::skylight::{CGSGetWindowBounds, G_CONNECTION};
use crate::actor::app::WindowId;

pub trait AXUIElementExt {
    /// fast frame access
    fn fframe(&self, wid: WindowId) -> Result<CGRect, accessibility::Error>;
}

impl AXUIElementExt for AXUIElement {
    fn fframe(&self, wid: WindowId) -> Result<CGRect, accessibility::Error> {
        let mut frame = CGRect::default();
        let res = unsafe { CGSGetWindowBounds(*G_CONNECTION, wid.idx.get(), &mut frame) };
        if res == 0 {
            return Ok(frame);
        } else {
            return Err(accessibility::Error::Ax(res));
        }
    }
}
