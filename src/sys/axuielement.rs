use std::error::Error as StdError;
use std::ffi::c_void;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::ops::Deref;
use std::ptr::{self, NonNull};

use objc2_application_services::{AXError, AXUIElement as RawAXUIElement, AXValue, AXValueType};
use objc2_core_foundation::{
    CFArray, CFBoolean, CFData, CFRetained, CFString, CFType, CGPoint, CGRect, CGSize, ConcreteType,
};

use super::skylight::{CGSGetWindowBounds, G_CONNECTION};
use crate::actor::app::WindowId;
use crate::sys::app::pid_t;
use crate::sys::skylight::_AXUIElementCreateWithRemoteToken;

pub const AX_WINDOW_ROLE: &str = "AXWindow";
pub const AX_STANDARD_WINDOW_SUBROLE: &str = "AXStandardWindow";
pub const AX_TAB_GROUP_ROLE: &str = "AXTabGroup";

#[derive(Clone)]
pub struct AXUIElement {
    inner: CFRetained<RawAXUIElement>,
}

#[derive(Debug, Clone)]
pub enum Error {
    Ax(AXError),
    NotFound,
}

pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Ax(err) => write!(f, "AX error {err:?}"),
            Error::NotFound => write!(f, "value not found"),
        }
    }
}

impl StdError for Error {}

impl From<AXError> for Error {
    fn from(value: AXError) -> Self { Self::Ax(value) }
}

#[derive(Debug, PartialEq)]
pub(crate) struct WindowAttributes {
    pub(crate) frame: CGRect,
    pub(crate) role: String,
    pub(crate) subrole: String,
    pub(crate) minimized: bool,
    pub(crate) title: String,
}

const WINDOW_ATTRIBUTES: [&str; 5] = ["AXFrame", "AXRole", "AXSubrole", "AXMinimized", "AXTitle"];

thread_local! {
    // Only immutable attribute names are reused; window values are always fresh.
    static WINDOW_ATTRIBUTE_NAMES: CFRetained<CFArray<CFString>> =
        CFArray::from_retained_objects(&WINDOW_ATTRIBUTES.map(CFString::from_static_str));
}

impl AXUIElement {
    fn new(inner: CFRetained<RawAXUIElement>) -> Self { Self { inner } }

    #[inline]
    pub fn application(pid: pid_t) -> Self {
        // SAFETY: The returned object follows the Create rule and therefore
        // owns +1 retain count.
        let inner = unsafe { RawAXUIElement::new_application(pid) };
        Self::new(inner)
    }

    #[inline]
    pub fn system_wide() -> Self {
        // SAFETY: The returned object follows the Create rule and therefore
        // owns +1 retain count.
        let inner = unsafe { RawAXUIElement::new_system_wide() };
        Self::new(inner)
    }

    // TODO: im not sure this works...
    #[inline]
    pub fn from_window_id(wid: WindowId) -> Self {
        const BUFSIZE: usize = 0x14;
        const MAGIC: u32 = 0x636f636f;

        let mut data = [0u8; BUFSIZE];

        let pid_bytes = (wid.pid as u32).to_ne_bytes();
        data[0x0..0x0 + pid_bytes.len()].copy_from_slice(&pid_bytes);

        let magic_bytes = MAGIC.to_ne_bytes();
        data[0x8..0x8 + magic_bytes.len()].copy_from_slice(&magic_bytes);

        let element_id = wid.idx.get() as u64;
        let element_bytes = element_id.to_ne_bytes();
        data[0xc..0xc + element_bytes.len()].copy_from_slice(&element_bytes);

        debug_assert_eq!(data.len(), BUFSIZE);
        let data = CFData::from_bytes(&data);

        let inner = unsafe {
            _AXUIElementCreateWithRemoteToken(CFRetained::<CFData>::as_ptr(&data).as_ptr())
        };
        Self::new(unsafe {
            CFRetained::from_raw(NonNull::new(inner).expect("non-null AXUIElement pointer"))
        })
    }

    #[inline]
    pub fn retained(&self) -> CFRetained<RawAXUIElement> { self.inner.clone() }

    #[allow(non_snake_case)]
    #[inline]
    pub fn as_concrete_TypeRef(&self) -> &RawAXUIElement { self.deref() }

    #[inline]
    pub fn raw_ptr(&self) -> NonNull<RawAXUIElement> { CFRetained::as_ptr(&self.inner) }

    #[inline]
    pub unsafe fn from_get_rule(ptr: *const RawAXUIElement) -> Self {
        let ptr = NonNull::new(ptr.cast_mut()).expect("attempted to create a NULL object");
        let retained = unsafe { CFRetained::retain(ptr) };
        Self::new(retained)
    }

    #[inline]
    pub unsafe fn from_create_rule(ptr: *const RawAXUIElement) -> Self {
        let ptr = NonNull::new(ptr.cast_mut()).expect("attempted to create a NULL object");
        let retained = unsafe { CFRetained::from_raw(ptr) };
        Self::new(retained)
    }

    fn copy_attribute(&self, name: &'static str) -> Result<Option<CFRetained<CFType>>> {
        let attr = CFString::from_static_str(name);
        let mut value: *const CFType = ptr::null();
        let status = unsafe {
            self.inner.copy_attribute_value(
                attr.as_ref(),
                NonNull::new((&mut value) as *mut *const CFType)
                    .expect("pointer to local is never null"),
            )
        };
        match status {
            AXError::Success => {
                if value.is_null() {
                    Ok(None)
                } else {
                    // SAFETY: The function follows the Copy rule and returns
                    // a value the caller owns.
                    let retained = unsafe {
                        CFRetained::from_raw(
                            NonNull::new(value as *mut CFType).expect("non-null value pointer"),
                        )
                    };
                    Ok(Some(retained))
                }
            }
            AXError::NoValue => Ok(None),
            err => Err(Error::Ax(err)),
        }
    }

    fn copy_required_attribute(&self, name: &'static str) -> Result<CFRetained<CFType>> {
        self.copy_attribute(name)?.ok_or(Error::NotFound)
    }

    fn downcast<T: ConcreteType>(&self, value: CFRetained<CFType>) -> Result<CFRetained<T>> {
        value.downcast::<T>().map_err(|_| Error::Ax(AXError::Failure))
    }

    pub fn bool_attribute(&self, name: &'static str) -> Result<bool> {
        let value = self.copy_required_attribute(name)?;
        let boolean = self.downcast::<CFBoolean>(value)?;
        Ok(boolean.value())
    }

    pub fn is_settable(&self, name: &'static str) -> Result<bool> {
        let mut is_settable = false;
        let status = unsafe {
            self.inner.is_attribute_settable(
                CFString::from_static_str(name).as_ref(),
                NonNull::new_unchecked((&mut is_settable as *mut bool).cast::<u8>()),
            )
        };
        match status {
            AXError::Success => Ok(is_settable),
            err => Err(Error::Ax(err)),
        }
    }

    pub fn frame(&self) -> Result<CGRect> {
        let value = self.copy_required_attribute("AXFrame")?;
        let ax_value = self.downcast::<AXValue>(value)?;
        rect_from_axvalue(&ax_value)
    }

    /// One request-local metadata read. Settable state is deliberately separate.
    pub(crate) fn window_attributes(&self) -> Result<WindowAttributes> {
        window_attributes_with_fallback(self.copy_window_attributes(), || {
            self.window_attributes_individually()
        })
    }

    fn window_attributes_individually(&self) -> Result<WindowAttributes> {
        Ok(WindowAttributes {
            frame: self.frame()?,
            role: self.role()?,
            subrole: self.subrole()?,
            minimized: self.minimized().unwrap_or_default(),
            title: self.title().unwrap_or_default(),
        })
    }

    fn copy_window_attributes(&self) -> Result<Option<WindowAttributes>> {
        use objc2_application_services::AXCopyMultipleAttributeOptions;
        let mut values = ptr::null();
        // SAFETY: names and the output pointer live through the call. Options 0
        // keeps errors in their individual slots instead of stopping early.
        let status = WINDOW_ATTRIBUTE_NAMES.with(|names| unsafe {
            self.inner.copy_multiple_attribute_values(
                names.as_opaque(),
                AXCopyMultipleAttributeOptions(0),
                NonNull::from(&mut values),
            )
        });
        // Adopt any returned array immediately, including on an error response.
        let values =
            NonNull::new(values.cast_mut()).map(|values| unsafe { CFRetained::from_raw(values) });
        finish_bulk_window_attributes(status, values)
    }

    pub fn fframe(&self, wid: WindowId) -> Result<CGRect> {
        let mut frame = CGRect::default();
        let result = unsafe { CGSGetWindowBounds(*G_CONNECTION, wid.idx.get(), &mut frame) };
        if result == 0 {
            Ok(frame)
        } else {
            Err(Error::Ax(AXError(result)))
        }
    }

    pub fn role(&self) -> Result<String> {
        let value = self.copy_required_attribute("AXRole")?;
        let string = self.downcast::<CFString>(value)?;
        Ok(string.to_string())
    }

    pub fn subrole(&self) -> Result<String> {
        let value = self.copy_required_attribute("AXSubrole")?;
        let string = self.downcast::<CFString>(value)?;
        Ok(string.to_string())
    }

    pub fn minimized(&self) -> Result<bool> { self.bool_attribute("AXMinimized") }

    pub fn fullscreen(&self) -> Result<bool> { self.bool_attribute("AXFullscreen") }

    pub fn title(&self) -> Result<String> {
        let value = self.copy_required_attribute("AXTitle")?;
        let string = self.downcast::<CFString>(value)?;
        Ok(string.to_string())
    }

    pub fn frontmost(&self) -> Result<bool> { self.bool_attribute("AXFrontmost") }

    pub fn main_window(&self) -> Result<AXUIElement> {
        let value = self.copy_required_attribute("AXMainWindow")?;
        let element = self.downcast::<RawAXUIElement>(value)?;
        Ok(AXUIElement::new(element))
    }

    /// Whether this element is the "main" window (AXMain).
    ///
    /// This is primarily used by developer tooling and may not be supported by all elements.
    pub fn main(&self) -> Result<bool> { self.bool_attribute("AXMain") }

    pub fn windows(&self) -> Result<Vec<AXUIElement>> {
        let Some(value) = self.copy_attribute("AXWindows")? else {
            return Ok(Vec::new());
        };
        let array = self.downcast::<CFArray>(value)?;
        let array = unsafe { CFRetained::cast_unchecked::<CFArray<CFType>>(array) };
        let mut out = Vec::with_capacity(array.len());
        for entry in array.iter() {
            let elem = self.downcast::<RawAXUIElement>(entry)?;
            out.push(AXUIElement::new(elem));
        }
        Ok(out)
    }

    pub fn parent(&self) -> Result<Option<AXUIElement>> {
        let Some(value) = self.copy_attribute("AXParent")? else {
            return Ok(None);
        };
        let element = self.downcast::<RawAXUIElement>(value)?;
        Ok(Some(AXUIElement::new(element)))
    }

    pub fn children(&self) -> Result<Vec<AXUIElement>> {
        let Some(value) = self.copy_attribute("AXChildren")? else {
            return Ok(Vec::new());
        };
        let array = self.downcast::<CFArray>(value)?;
        let array = unsafe { CFRetained::cast_unchecked::<CFArray<CFType>>(array) };
        let mut out = Vec::with_capacity(array.len());
        for entry in array.iter() {
            let elem = self.downcast::<RawAXUIElement>(entry)?;
            out.push(AXUIElement::new(elem));
        }
        Ok(out)
    }

    /// True when this window has an AXTabGroup child whose AXTabs list has ≥ 2
    /// entries. macOS only exposes the tab group once a second tab is added.
    pub fn is_native_tabbed(&self) -> bool {
        let Ok(children) = self.children() else {
            return false;
        };
        for child in &children {
            let Ok(role) = child.role() else { continue };
            if role != AX_TAB_GROUP_ROLE {
                continue;
            }
            let Ok(Some(tabs)) = child.copy_attribute("AXTabs") else {
                return false;
            };
            let Ok(tabs) = self.downcast::<CFArray>(tabs) else {
                return false;
            };
            return tabs.len() >= 2;
        }
        false
    }

    pub fn attribute(&self, name: &'static str) -> Result<Option<CFRetained<CFType>>> {
        self.copy_attribute(name)
    }

    pub fn set_position(&self, mut point: CGPoint) -> Result<()> {
        let attr = CFString::from_static_str("AXPosition");
        let value = make_axvalue(AXValueType::CGPoint, &mut point)?;
        self.set_attribute_value(attr.as_ref(), value.as_ref())
    }

    pub fn set_size(&self, mut size: CGSize) -> Result<()> {
        let attr = CFString::from_static_str("AXSize");
        let value = make_axvalue(AXValueType::CGSize, &mut size)?;
        self.set_attribute_value(attr.as_ref(), value.as_ref())
    }

    pub fn raise(&self) -> Result<()> {
        let action = CFString::from_static_str("AXRaise");
        let status = unsafe { self.inner.perform_action(action.as_ref()) };
        if status == AXError::Success {
            Ok(())
        } else {
            Err(Error::Ax(status))
        }
    }

    fn set_attribute_value(&self, name: &CFString, value: &CFType) -> Result<()> {
        let status = unsafe { self.inner.set_attribute_value(name, value) };
        if status == AXError::Success {
            Ok(())
        } else {
            Err(Error::Ax(status))
        }
    }

    pub fn set_bool_attribute(&self, name: &'static str, value: bool) -> Result<()> {
        let cf_bool = CFBoolean::new(value);
        let attr = CFString::from_static_str(name);
        self.set_attribute_value(attr.as_ref(), cf_bool.as_ref())
    }

    pub fn can_move(&self) -> Result<bool> { self.is_settable("AXPosition") }

    pub fn can_resize(&self) -> Result<bool> { self.is_settable("AXSize") }

    #[allow(unused)]
    pub fn element_at_position(point: CGPoint) -> Option<Self> {
        let mut out_ptr: *const RawAXUIElement = std::ptr::null();
        let status = unsafe {
            RawAXUIElement::copy_element_at_position(
                &*RawAXUIElement::new_system_wide(),
                point.x as f32,
                point.y as f32,
                std::ptr::NonNull::new_unchecked(&mut out_ptr),
            )
        };
        if status == AXError::Success && !out_ptr.is_null() {
            let retained = unsafe {
                CFRetained::from_raw(
                    std::ptr::NonNull::new(out_ptr as *mut RawAXUIElement)
                        .expect("non-null AXUIElement pointer"),
                )
            };
            return Some(Self::new(retained));
        }

        None
    }

    pub fn process_id(&self) -> Result<pid_t> {
        let pid = unsafe { NonNull::new_unchecked(&mut 0i32) };
        if unsafe { self.inner.pid(pid) } == AXError::Success {
            Ok(unsafe { *pid.as_ref() })
        } else {
            Err(Error::Ax(unsafe { self.inner.pid(pid) }))
        }
    }
}

impl Deref for AXUIElement {
    type Target = RawAXUIElement;

    fn deref(&self) -> &Self::Target { &self.inner }
}

impl PartialEq for AXUIElement {
    fn eq(&self, other: &Self) -> bool {
        // AX APIs may return a fresh retained wrapper for the same remote
        // accessibility object. The generated Core Foundation type implements
        // equality with CFEqual, which compares that remote identity; comparing
        // allocation addresses makes AXWindows and AXMainWindow disagree after
        // wake even when they refer to the same window.
        self.deref().eq(other.deref())
    }
}

impl Eq for AXUIElement {}

impl Hash for AXUIElement {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // Keep Hash consistent with the CFEqual-based PartialEq implementation.
        self.deref().hash(state);
    }
}

impl fmt::Debug for AXUIElement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { self.deref().fmt(f) }
}

fn finish_bulk_window_attributes(
    status: AXError,
    values: Option<CFRetained<CFArray>>,
) -> Result<Option<WindowAttributes>> {
    match status {
        AXError::Success => {
            let Some(values) = values else {
                return Ok(None);
            };
            if values.len() != WINDOW_ATTRIBUTES.len() {
                return Ok(None);
            }
            // SAFETY: this API returns an array of CFType values.
            let values = unsafe { CFRetained::cast_unchecked::<CFArray<CFType>>(values) };
            decode_window_attributes(values.iter()).map(Some)
        }
        AXError::NotImplemented | AXError::AttributeUnsupported | AXError::IllegalArgument => {
            Ok(None)
        }
        err => Err(Error::Ax(err)),
    }
}

fn window_attributes_with_fallback(
    bulk: Result<Option<WindowAttributes>>,
    fallback: impl FnOnce() -> Result<WindowAttributes>,
) -> Result<WindowAttributes> {
    match bulk? {
        Some(attributes) => Ok(attributes),
        None => fallback(),
    }
}

fn bulk_attribute(value: CFRetained<CFType>) -> Result<CFRetained<CFType>> {
    use objc2_core_foundation::CFNull;
    if value.downcast_ref::<CFNull>().is_some() {
        return Err(Error::NotFound);
    }
    if let Some(ax_value) = value.downcast_ref::<AXValue>() {
        // SAFETY: AXValue is a checked concrete type; the error output has the
        // exact layout required for kAXValueAXErrorType.
        if unsafe { ax_value.r#type() } == AXValueType::AXError {
            let mut error = AXError::Failure;
            let decoded =
                unsafe { ax_value.value(AXValueType::AXError, NonNull::from(&mut error).cast()) };
            return Err(if !decoded {
                Error::Ax(AXError::Failure)
            } else if error == AXError::NoValue {
                Error::NotFound
            } else {
                Error::Ax(error)
            });
        }
    }
    Ok(value)
}

fn decode_window_attributes(
    values: impl IntoIterator<Item = CFRetained<CFType>>,
) -> Result<WindowAttributes> {
    let mut values = values.into_iter();
    let mut next = || values.next().ok_or(Error::NotFound).and_then(bulk_attribute);
    // Decode every slot independently before applying required/optional policy.
    let frame = next()
        .and_then(|v| v.downcast::<AXValue>().map_err(|_| Error::Ax(AXError::Failure)))
        .and_then(|v| rect_from_axvalue(&v));
    let role = next().and_then(decode_string);
    let subrole = next().and_then(decode_string);
    let minimized = next()
        .and_then(|v| v.downcast::<CFBoolean>().map_err(|_| Error::Ax(AXError::Failure)))
        .map(|v| v.value());
    let title = next().and_then(decode_string);
    Ok(WindowAttributes {
        frame: frame?,
        role: role?,
        subrole: subrole?,
        minimized: minimized.unwrap_or_default(),
        title: title.unwrap_or_default(),
    })
}

fn decode_string(value: CFRetained<CFType>) -> Result<String> {
    value
        .downcast::<CFString>()
        .map(|v| v.to_string())
        .map_err(|_| Error::Ax(AXError::Failure))
}

fn rect_from_axvalue(value: &AXValue) -> Result<CGRect> {
    let mut rect = CGRect::default();
    let success = unsafe {
        value.value(
            AXValueType::CGRect,
            NonNull::new((&mut rect as *mut CGRect).cast::<c_void>()).expect("rect pointer"),
        )
    };
    if success {
        Ok(rect)
    } else {
        Err(Error::Ax(AXError::Failure))
    }
}

fn make_axvalue<T>(ty: AXValueType, value: &mut T) -> Result<CFRetained<AXValue>> {
    let ptr = NonNull::new((value as *mut T).cast::<c_void>()).expect("value pointer");
    unsafe { AXValue::new(ty, ptr) }.ok_or(Error::Ax(AXError::Failure))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[test]
    fn separately_retained_elements_share_logical_identity() {
        let pid = std::process::id() as pid_t;
        let first = AXUIElement::application(pid);
        let second = AXUIElement::application(pid);

        let mut by_element = HashMap::new();
        by_element.insert(first, 1);

        assert_eq!(by_element.get(&second), Some(&1));
    }
}
