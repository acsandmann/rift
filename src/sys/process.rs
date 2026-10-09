use objc2_core_graphics::CGError;

use super::app::pid_t;
use crate::sys::cg_ok;

pub struct ProcessInfo {
    pub is_xpc: bool,
}

impl ProcessInfo {
    pub fn for_pid(pid: pid_t) -> Result<Self, CGError> {
        let psn = ProcessSerialNumber::for_pid(pid)?;

        let mut info = ProcessInfoRec::default();
        info.processInfoLength = size_of::<ProcessInfoRec>() as _;
        cg_ok(unsafe { GetProcessInformation(&psn, &mut info) })?;

        Ok(Self {
            is_xpc: info.processType.to_be_bytes() == *b"XPC!",
        })
    }
}

type FourCharCode = u32;
type OSType = FourCharCode;

#[allow(dead_code)]
#[allow(non_snake_case)]
#[repr(C, packed(2))]
#[derive(Default)]
struct ProcessInfoRec {
    processInfoLength: u32,
    processName: *const u8,
    processNumber: ProcessSerialNumber,
    processType: u32,
    processSignature: OSType,
    processMode: u32,
    processLocation: *const u8,
    processSize: u32,
    processFreeMem: u32,
    processLauncher: ProcessSerialNumber,
    processLaunchDate: u32,
    processActiveTime: u32,
    processAppRef: *const u8,
}
const _: () = if size_of::<ProcessInfoRec>() != 72 {
    panic!("unexpected size")
};

#[repr(C)]
#[derive(Default)]
pub struct ProcessSerialNumber {
    high: u32,
    low: u32,
}

impl ProcessSerialNumber {
    pub(super) fn for_pid(pid: pid_t) -> Result<Self, CGError> {
        let mut psn = ProcessSerialNumber::default();
        cg_ok(unsafe { GetProcessForPID(pid, &mut psn) })?;
        Ok(psn)
    }
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    // Deprecated in macOS 10.9.
    fn GetProcessForPID(pid: pid_t, psn: *mut ProcessSerialNumber) -> CGError;

    // Deprecated in macOS 10.9.
    fn GetProcessInformation(psn: *const ProcessSerialNumber, info: *mut ProcessInfoRec)
    -> CGError;
}
