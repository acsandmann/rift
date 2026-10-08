use std::ffi::{c_int, c_void};
use std::sync::LazyLock;

use objc2_core_foundation::{CFString, CFType, CGPoint, CGRect};
use objc2_core_graphics::{CGColorSpace, CGContext, CGError};
#[allow(non_camel_case_types)]
pub type cid_t = i32;
pub static G_CONNECTION: LazyLock<cid_t> = LazyLock::new(|| unsafe { SLSMainConnectionID() });
#[link(name = "SkyLight", kind = "framework")]
unsafe extern "C" {
    pub(super) fn SLSMainConnectionID() -> cid_t;
    pub fn SLSNewWindowWithOpaqueShapeAndContext(
        cid: cid_t,
        r#type: c_int,
        region: *mut CFType,
        opaque_region: *mut CFType,
        options: c_int,
        tags: *mut u64,
        x: f32,
        y: f32,
        tag_count: c_int,
        out_wid: *mut u32,
        context: *mut c_void,
    ) -> CGError;
    pub fn SLSReleaseWindow(cid: cid_t, wid: u32) -> CGError;
    pub fn SLSSetWindowResolution(cid: cid_t, wid: u32, resolution: f64) -> CGError;
    pub fn SLSSetWindowAlpha(cid: cid_t, wid: u32, alpha: f32) -> CGError;
    pub fn SLSSetWindowBackgroundBlurRadiusStyle(
        cid: cid_t,
        wid: u32,
        radius: c_int,
        style: c_int,
    ) -> CGError;
    pub fn SLSSetWindowBackgroundBlurRadius(cid: cid_t, wid: u32, radius: c_int) -> CGError;
    pub fn SLSSetWindowLevel(cid: cid_t, wid: u32, level: c_int) -> CGError;
    pub fn SLSSetWindowSubLevel(cid: cid_t, wid: u32, sub_level: c_int) -> CGError;
    pub fn SLSSetWindowOpacity(cid: cid_t, wid: u32, opaque: bool) -> CGError;
    pub fn SLSSetWindowShape(
        cid: cid_t,
        wid: u32,
        x_offset: f32,
        y_offset: f32,
        shape: *mut CFType,
    ) -> CGError;
    pub fn SLSOrderWindow(cid: cid_t, wid: u32, order: c_int, relative_to: u32) -> CGError;
    pub fn SLSSetWindowTags(cid: cid_t, wid: u32, tags: *mut u64, tag_count: c_int) -> CGError;
    pub fn SLSClearWindowTags(cid: cid_t, wid: u32, tags: *mut u64, tag_count: c_int) -> CGError;
    pub fn CGSNewRegionWithRect(rect: *const CGRect, region: *mut *mut CFType) -> CGError;
    pub fn CGSNewRegionWithRectList(
        rects: *const CGRect,
        rect_count: c_int,
        region: *mut *mut CFType,
    ) -> CGError;
    pub fn CGRegionCreateEmptyRegion() -> *mut CFType;
    pub fn SLWindowContextCreate(cid: cid_t, wid: u32, options: *mut CFType) -> *mut CGContext;

    pub fn SLSAddSurface(cid: cid_t, wid: u32, out_sid: *mut u32) -> CGError;
    pub fn SLSRemoveSurface(cid: cid_t, wid: u32, sid: u32) -> CGError;
    pub fn SLSBindSurface(
        cid: cid_t,
        wid: u32,
        sid: u32,
        options: c_int,
        unknown: c_int,
        context_id: u32,
    ) -> CGError;
    pub fn SLSSetSurfaceBounds(cid: cid_t, wid: u32, sid: u32, bounds: CGRect) -> CGError;
    pub fn SLSSetSurfaceResolution(cid: cid_t, wid: u32, sid: u32, resolution: f64) -> CGError;
    pub fn SLSSetSurfaceOpacity(cid: cid_t, wid: u32, sid: u32, opaque: bool) -> CGError;
    pub fn SLSSetSurfaceColorSpace(cid: cid_t, wid: u32, sid: u32, color_space: *mut CGColorSpace)
    -> CGError;
    pub fn SLSOrderSurface(cid: cid_t, wid: u32, sid: u32, order: c_int, relative_to: u32) -> CGError;
    pub fn SLSTransactionCreate(cid: cid_t) -> *mut CFType;
    pub fn SLSTransactionSetWindowShape(
        transaction: *mut CFType,
        wid: u32,
        x_offset: f32,
        y_offset: f32,
        shape: *mut CFType,
    );
    pub fn SLSTransactionMoveWindowWithGroup(transaction: *mut CFType, wid: u32, point: CGPoint);
    pub fn SLSTransactionSetSurfaceBounds(transaction: *mut CFType, wid: u32, sid: u32, bounds: CGRect);
    pub fn SLSTransactionCommit(transaction: *mut CFType, asynchronous: u32);
    pub fn SLSSetWindowProperty(cid: cid_t, wid: u32, property: *mut CFString, value: *mut CFType)
    -> CGError;
    pub fn SLSFlushWindowContentRegion(cid: cid_t, wid: u32, dirty: *mut c_void) -> CGError;
}
