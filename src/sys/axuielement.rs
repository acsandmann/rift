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

    pub fn fast_frame(&self, wid: WindowId) -> Result<CGRect> {
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

    pub fn close(&self) -> Result<()> {
        if let Some(value) = self.copy_attribute("AXCloseButton")? {
            let button = self.downcast::<RawAXUIElement>(value)?;
            let action = CFString::from_static_str("AXPress");
            let status = unsafe { button.perform_action(action.as_ref()) };
            if status == AXError::Success {
                return Ok(());
            } else {
                return Err(Error::Ax(status));
            }
        }

        Err(Error::NotFound)
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

    pub fn can_move(&self) -> Result<bool> { self.bool_attribute("AXPosition") }

    pub fn can_resize(&self) -> Result<bool> { self.bool_attribute("AXSize") }
}

impl Deref for AXUIElement {
    type Target = RawAXUIElement;

    fn deref(&self) -> &Self::Target { &self.inner }
}

impl PartialEq for AXUIElement {
    fn eq(&self, other: &Self) -> bool { self.raw_ptr() == other.raw_ptr() }
}

impl Eq for AXUIElement {}

impl Hash for AXUIElement {
    fn hash<H: Hasher>(&self, state: &mut H) { self.raw_ptr().hash(state); }
}

impl fmt::Debug for AXUIElement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { self.deref().fmt(f) }
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
