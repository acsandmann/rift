use std::{fmt, ptr};

use core_foundation::base::{CFRelease, CFTypeRef, TCFType};
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;
use core_graphics::base::{CGError, kCGErrorSuccess};
use objc2_core_foundation::CGRect;

use super::skylight::{
    CGRegionCreateEmptyRegion, CGSNewRegionWithRect, G_CONNECTION, SLSClearWindowTags,
    SLSNewWindowWithOpaqueShapeAndContext, SLSOrderWindow, SLSReleaseWindow, SLSSetWindowAlpha,
    SLSSetWindowBackgroundBlurRadiusStyle, SLSSetWindowLevel, SLSSetWindowOpacity,
    SLSSetWindowProperty, SLSSetWindowResolution, SLSSetWindowShape, SLSSetWindowSubLevel,
    SLSSetWindowTags, cid_t,
};

type WindowId = u32;
const TAG_BITSET_LEN: i32 = 64;
const DEFAULT_SUBLEVEL: i32 = 0;

#[repr(transparent)]
struct CFRegion(CFTypeRef);

impl CFRegion {
    fn from_rect(rect: &CGRect) -> Result<Self, CGError> {
        let mut region: CFTypeRef = ptr::null();
        let err = unsafe { CGSNewRegionWithRect(rect, &mut region) };
        if err == kCGErrorSuccess {
            Ok(Self(region))
        } else {
            Err(err)
        }
    }

    /// Empty region (Create-rule; must be released).
    fn empty() -> Self { Self(unsafe { CGRegionCreateEmptyRegion() }) }

    #[inline]
    fn as_cf(&self) -> CFTypeRef { self.0 }
}

impl Drop for CFRegion {
    fn drop(&mut self) { unsafe { CFRelease(self.0) } }
}

#[inline]
fn cg_ok(err: CGError) -> Result<(), CGError> {
    if err == kCGErrorSuccess {
        Ok(())
    } else {
        Err(err)
    }
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
            Region(e) => write!(f, "CGS region error: {e}"),
            Window(e) => write!(f, "CGS window create error: {e}"),
            Resolution(e) => write!(f, "CGS window resolution error: {e}"),
            Alpha(e) => write!(f, "CGS window alpha/opacity error: {e}"),
            Blur(e) => write!(f, "CGS window blur error: {e}"),
            Level(e) => write!(f, "CGS window level/order error: {e}"),
            Shape(e) => write!(f, "CGS window shape error: {e}"),
            Tags(e) => write!(f, "CGS window tags error: {e}"),
            Release(e) => write!(f, "CGS window release error: {e}"),
            Property(e) => write!(f, "CGS window property error: {e}"),
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
                frame_region.as_cf(),
                empty_region.as_cf(),
                13,
                &mut tags,
                0.0,
                0.0,
                TAG_BITSET_LEN,
                &mut wid,
                ptr::null_mut(),
            ))
            .map_err(CgsWindowError::Window)?;

            // doesnt work often but nice if it can
            if let Err(res_err) = cg_ok(SLSSetWindowResolution(connection, wid, 1.0)) {
                tracing::warn!(error=?res_err, "SLSSetWindowResolution failed; continuing");
            }

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
    pub fn set_blur(&self, radius: i32, style: i32) -> Result<(), CgsWindowError> {
        unsafe {
            cg_ok(SLSSetWindowBackgroundBlurRadiusStyle(
                self.connection,
                self.id,
                radius,
                style,
            ))
        }
        .map_err(CgsWindowError::Blur)
    }

    pub fn set_level(&self, level: i32) -> Result<(), CgsWindowError> {
        unsafe { cg_ok(SLSSetWindowLevel(self.connection, self.id, level)) }
            .map_err(CgsWindowError::Level)?;
        unsafe { cg_ok(SLSSetWindowSubLevel(self.connection, self.id, DEFAULT_SUBLEVEL)) }
            .map_err(CgsWindowError::Level)
    }

    pub fn set_shape(&self, frame: CGRect) -> Result<(), CgsWindowError> {
        unsafe {
            let region = CFRegion::from_rect(&frame).map_err(CgsWindowError::Region)?;
            cg_ok(SLSSetWindowShape(
                self.connection,
                self.id,
                0.0,
                0.0,
                region.as_cf(),
            ))
            .map_err(CgsWindowError::Shape)
        }
    }

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

    pub fn bind_to_context(&self, context_id: u32) -> Result<(), CgsWindowError> {
        let key = CFString::from_static_string("CAContextID");
        let value = CFNumber::from(context_id as i32);
        unsafe {
            cg_ok(SLSSetWindowProperty(
                self.connection,
                self.id,
                key.as_concrete_TypeRef(),
                value.as_CFTypeRef(),
            ))
            .map_err(CgsWindowError::Property)
        }
    }

    /// Orders this window above `relative` (or above 0 for global top).
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
