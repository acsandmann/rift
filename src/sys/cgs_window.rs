use std::fmt;
use std::ptr::{self, NonNull};

use objc2_core_foundation::{CFNumber, CFRetained, CFString, CFType, CGPoint, CGRect, Type};
use objc2_core_graphics::CGError;

use super::skylight::{
    CGRegionCreateEmptyRegion, CGSNewRegionWithRect, G_CONNECTION, SLSClearWindowTags,
    SLSNewWindowWithOpaqueShapeAndContext, SLSOrderWindow, SLSReleaseWindow, SLSSetWindowAlpha,
    SLSSetWindowBackgroundBlurRadiusStyle, SLSSetWindowLevel, SLSSetWindowOpacity,
    SLSSetWindowProperty, SLSSetWindowResolution, SLSSetWindowShape, SLSSetWindowSubLevel,
    SLSSetWindowTags, cid_t,
};
use crate::sys::cg_ok;
use crate::sys::skylight::SLSSetWindowBackgroundBlurRadius;

type WindowId = u32;
const TAG_BITSET_LEN: i32 = 64;
const DEFAULT_SUBLEVEL: i32 = 0;

#[repr(transparent)]
struct CFRegion(CFRetained<CFType>);

impl CFRegion {
    fn from_rect(rect: &CGRect) -> Result<Self, CGError> {
        let mut region: *mut CFType = ptr::null_mut();
        cg_ok(unsafe { CGSNewRegionWithRect(rect, &mut region) })?;
        Ok(Self(unsafe {
            CFRetained::from_raw(NonNull::new_unchecked(region))
        }))
    }

    fn empty() -> Self {
        Self(unsafe { CFRetained::from_raw(NonNull::new_unchecked(CGRegionCreateEmptyRegion())) })
    }

    #[inline]
    fn as_ptr(&self) -> *mut CFType { CFRetained::<CFType>::as_ptr(&self.0).as_ptr() }
}

impl Drop for CFRegion {
    // SAFETY: cfretained should be auto dropping here
    fn drop(&mut self) {}
}

#[derive(Debug)]
pub enum CgsWindowError {
    Region(CGError),
    Window(CGError),
    Resolution(CGError),
    Alpha(CGError),
    Blur(CGError),
    Level(CGError),
    Shape(CGError),
    Tags(CGError),
    Release(CGError),
    Property(CGError),
}

impl fmt::Display for CgsWindowError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use CgsWindowError::*;
        match self {
            Region(e) => write!(f, "CGS region error: {:?}", e),
            Window(e) => write!(f, "CGS window create error: {:?}", e),
            Resolution(e) => write!(f, "CGS window resolution error: {:?}", e),
            Alpha(e) => write!(f, "CGS window alpha/opacity error: {:?}", e),
            Blur(e) => write!(f, "CGS window blur error: {:?}", e),
            Level(e) => write!(f, "CGS window level/order error: {:?}", e),
            Shape(e) => write!(f, "CGS window shape error: {:?}", e),
            Tags(e) => write!(f, "CGS window tags error: {:?}", e),
            Release(e) => write!(f, "CGS window release error: {:?}", e),
            Property(e) => write!(f, "CGS window property error: {:?}", e),
        }
    }
}

impl std::error::Error for CgsWindowError {}

#[derive(Debug)]
pub struct CgsWindow {
    id: WindowId,
    connection: cid_t,
    owned: bool,
}

impl CgsWindow {
    pub fn new(frame: CGRect) -> Result<Self, CgsWindowError> {
        unsafe {
            let connection = *G_CONNECTION;

            let frame_region = CFRegion::from_rect(&frame).map_err(CgsWindowError::Region)?;
            let empty_region = CFRegion::empty();

            let mut tags: u64 = (1 << 1) | (1 << 9);

            let mut wid: WindowId = 0;
            cg_ok(SLSNewWindowWithOpaqueShapeAndContext(
                connection,
                2,
                frame_region.as_ptr(),
                empty_region.as_ptr(),
                13,
                &mut tags,
                0.0,
                0.0,
                TAG_BITSET_LEN,
                &mut wid,
                ptr::null_mut(),
            ))
            .map_err(CgsWindowError::Window)?;

            cg_ok(SLSSetWindowResolution(connection, wid, 1.0))
                .map_err(CgsWindowError::Resolution)?;

            Ok(Self {
                id: wid,
                connection,
                owned: true,
            })
        }
    }

    #[inline]
    pub fn id(&self) -> WindowId { self.id }

    #[inline]
    pub fn into_unowned(mut self) -> Self {
        self.owned = false;
        self
    }

    #[inline]
    pub fn from_existing(id: WindowId) -> Self {
        Self {
            id,
            connection: *G_CONNECTION,
            owned: false,
        }
    }

    #[inline]
    pub fn set_alpha(&self, alpha: f32) -> Result<(), CgsWindowError> {
        unsafe { cg_ok(SLSSetWindowAlpha(self.connection, self.id, alpha)) }
            .map_err(CgsWindowError::Alpha)
    }

    #[inline]
    pub fn set_opacity(&self, opaque: bool) -> Result<(), CgsWindowError> {
        unsafe { cg_ok(SLSSetWindowOpacity(self.connection, self.id, opaque)) }
            .map_err(CgsWindowError::Alpha)
    }

    #[inline]
    pub fn set_blur(&self, radius: i32, style: Option<i32>) -> Result<(), CgsWindowError> {
        unsafe {
            cg_ok(if let Some(style) = style {
                SLSSetWindowBackgroundBlurRadiusStyle(self.connection, self.id, radius, style)
            } else {
                SLSSetWindowBackgroundBlurRadius(self.connection, self.id, radius)
            })
        }
        .map_err(CgsWindowError::Blur)
    }

    #[inline]
    pub fn set_level(&self, level: i32) -> Result<(), CgsWindowError> {
        unsafe { cg_ok(SLSSetWindowLevel(self.connection, self.id, level)) }
            .map_err(CgsWindowError::Level)?;
        unsafe { cg_ok(SLSSetWindowSubLevel(self.connection, self.id, DEFAULT_SUBLEVEL)) }
            .map_err(CgsWindowError::Level)
    }

    #[inline]
    pub fn set_shape(&self, frame: CGRect) -> Result<(), CgsWindowError> {
        unsafe {
            let offset = frame.origin;
            let size_rect = CGRect::new(CGPoint::new(0.0, 0.0), frame.size);
            let region = CFRegion::from_rect(&size_rect).map_err(CgsWindowError::Region)?;
            let result = cg_ok(SLSSetWindowShape(
                self.connection,
                self.id,
                offset.x as f32,
                offset.y as f32,
                region.as_ptr(),
            ))
            .map_err(CgsWindowError::Shape);
            drop(region);
            result
        }
    }

    #[inline]
    pub fn set_tags(&self, tags: u64) -> Result<(), CgsWindowError> {
        unsafe {
            let mut t = tags;
            cg_ok(SLSSetWindowTags(
                self.connection,
                self.id,
                &mut t,
                TAG_BITSET_LEN,
            ))
            .map_err(CgsWindowError::Tags)
        }
    }

    #[inline]
    pub fn clear_tags(&self, tags: u64) -> Result<(), CgsWindowError> {
        unsafe {
            let mut t = tags;
            cg_ok(SLSClearWindowTags(
                self.connection,
                self.id,
                &mut t,
                TAG_BITSET_LEN,
            ))
            .map_err(CgsWindowError::Tags)
        }
    }

    #[inline]
    pub fn bind_to_context(&self, context_id: u32) -> Result<(), CgsWindowError> {
        let key = CFString::from_str("CAContextID");
        let value = CFNumber::new_i32(context_id as i32);
        unsafe {
            cg_ok(SLSSetWindowProperty(
                self.connection,
                self.id,
                CFRetained::<CFString>::as_ptr(&key).as_ptr(),
                CFRetained::<CFNumber>::as_ptr(&value).as_ptr() as *mut CFType,
            ))
            .map_err(CgsWindowError::Property)
        }
    }

    #[inline]
    pub fn order_above(&self, relative: Option<WindowId>) -> Result<(), CgsWindowError> {
        let rel = relative.unwrap_or(0);
        unsafe {
            cg_ok(SLSOrderWindow(
                self.connection,
                self.id,
                1, // kCGSOrderAbove
                rel,
            ))
        }
        .map_err(CgsWindowError::Level)
    }

    #[inline]
    pub fn order_out(&self) -> Result<(), CgsWindowError> {
        unsafe {
            cg_ok(SLSOrderWindow(
                self.connection,
                self.id,
                0, // kCGSOrderOut
                0,
            ))
        }
        .map_err(CgsWindowError::Level)
    }

    #[inline]
    pub fn set_property<T: Type>(
        &self,
        key: CFRetained<CFString>,
        value: CFRetained<T>,
    ) -> Result<(), CgsWindowError> {
        unsafe {
            cg_ok(SLSSetWindowProperty(
                self.connection,
                self.id,
                CFRetained::<CFString>::as_ptr(&key).as_ptr(),
                CFRetained::<T>::as_ptr(&value).as_ptr() as *mut CFType,
            ))
            .map_err(CgsWindowError::Property)
        }
    }

    #[inline]
    pub fn set_resolution(&self, scale: f64) -> Result<(), CgsWindowError> {
        unsafe { cg_ok(SLSSetWindowResolution(self.connection, self.id, scale)) }
            .map_err(CgsWindowError::Resolution)
    }
}

impl Drop for CgsWindow {
    fn drop(&mut self) {
        if !self.owned {
            return;
        }
        unsafe {
            if let Err(err) = cg_ok(SLSReleaseWindow(self.connection, self.id)) {
                tracing::warn!(error=?err, id=self.id, "failed to release CGS window");
            }
        }
    }
}
