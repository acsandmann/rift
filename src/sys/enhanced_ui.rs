use objc2_application_services::AXError;
use tracing::debug;

use crate::sys::axuielement::{AXUIElement, Error as AxError};

const ATTRIBUTE: &str = "AXEnhancedUserInterface";

#[derive(Debug, Default)]
pub struct EnhancedUi {
    depth: usize,
    suppression: Suppression,
    unsupported: bool,
}

#[derive(Debug, Default, PartialEq, Eq)]
enum Suppression {
    #[default]
    Inactive,
    Disabled,
    RestorePending,
}

impl EnhancedUi {
    /// Apps can publish EUI when they open a window. Recheck support on the
    /// next write without disturbing any active suppression.
    pub fn invalidate_support_cache(&mut self) { self.unsupported = false; }

    /// Check once per write batch while support or suppression is uncertain.
    pub fn acquire(&mut self, app: &AXUIElement) {
        self.depth += 1;
        if self.unsupported || self.suppression == Suppression::Disabled {
            return;
        }

        match app.bool_attribute(ATTRIBUTE) {
            Ok(true) => match set_enabled(app, false) {
                Ok(()) => self.suppression = Suppression::Disabled,
                Err(AxError::Ax(AXError::AttributeUnsupported)) => self.unsupported = true,
                Err(AxError::NotFound) => {}
                Err(err) => {
                    // An AX setter can apply its change despite an error.
                    self.suppression = Suppression::RestorePending;
                    debug!(?err, "Could not confirm Enhanced UI was disabled");
                }
            },
            Ok(false) if self.suppression == Suppression::RestorePending => {
                self.suppression = Suppression::Disabled;
            }
            Ok(false) => {}
            Err(AxError::Ax(AXError::AttributeUnsupported)) => self.unsupported = true,
            Err(AxError::NotFound) => {}
            Err(err) => debug!(?err, "Could not read Enhanced UI"),
        }
    }

    /// Keep the current batch's suppression alive across animation samples.
    pub fn retain(&mut self) {
        debug_assert!(self.depth > 0);
        self.depth += 1;
    }

    pub fn release(&mut self, app: &AXUIElement) {
        if self.depth == 0 {
            return;
        }

        self.depth -= 1;
        if self.depth == 0 {
            self.try_restore(app);
        }
    }

    pub fn restore_if_needed(&mut self, app: &AXUIElement) {
        self.depth = 0;
        self.try_restore(app);
    }

    fn try_restore(&mut self, app: &AXUIElement) {
        if self.suppression == Suppression::Inactive {
            return;
        }

        match set_enabled(app, true) {
            Ok(()) => {
                self.suppression = Suppression::Inactive;
                self.unsupported = false;
            }
            Err(AxError::Ax(AXError::AttributeUnsupported)) => {
                self.suppression = Suppression::Inactive;
                self.unsupported = true;
            }
            Err(AxError::NotFound) => self.suppression = Suppression::Inactive,
            Err(err) => {
                self.suppression = Suppression::RestorePending;
                debug!(?err, "Could not confirm Enhanced UI was restored");
            }
        }
    }
}

fn set_enabled(app: &AXUIElement, enabled: bool) -> Result<(), AxError> {
    match app.set_bool_attribute(ATTRIBUTE, enabled) {
        // Firefox can apply the change while returning NotImplemented.
        Err(AxError::Ax(AXError::NotImplemented))
            if app.bool_attribute(ATTRIBUTE).is_ok_and(|actual| actual == enabled) =>
        {
            Ok(())
        }
        result => result,
    }
}
