//! A text field saves its value when editing ends. AppKit ends it on Return or
//! Tab only, so a click elsewhere in the window, or the window losing key
//! status, ends it here as well.

use std::ptr::NonNull;

use block2::RcBlock;
use objc2::Message;
use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyObject, NSObjectProtocol, ProtocolObject};
use objc2_app_kit::{
    NSControl, NSEvent, NSEventMask, NSTextView, NSWindow, NSWindowDidResignKeyNotification,
};
use objc2_core_foundation::CGPoint;
use objc2_foundation::{NSNotification, NSNotificationCenter};

/// Ends a settings window's field editing on a click outside the field and when
/// the window resigns key, until dropped.
pub(super) struct EndEditing {
    monitor: Option<Retained<AnyObject>>,
    observer: Retained<ProtocolObject<dyn NSObjectProtocol>>,
}

impl EndEditing {
    pub(super) fn new(window: &NSWindow) -> Self {
        let clicked = Weak::new(window);
        let handler = RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
            let event_ref = unsafe { event.as_ref() };
            if let Some(window) = clicked.load()
                && event_ref.windowNumber() == window.windowNumber()
                && ends_editing(&window, event_ref.locationInWindow())
            {
                window.makeFirstResponder(None);
            }
            event.as_ptr()
        });
        let monitor = unsafe {
            NSEvent::addLocalMonitorForEventsMatchingMask_handler(
                NSEventMask::LeftMouseDown,
                &handler,
            )
        };

        let resigned = Weak::new(window);
        let block = RcBlock::new(move |_: NonNull<NSNotification>| {
            if let Some(window) = resigned.load()
                && editing_control(&window).is_some()
            {
                window.makeFirstResponder(None);
            }
        });
        let observer = unsafe {
            NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
                Some(NSWindowDidResignKeyNotification),
                Some(window),
                None,
                &block,
            )
        };
        Self { monitor, observer }
    }
}

impl Drop for EndEditing {
    fn drop(&mut self) {
        if let Some(monitor) = self.monitor.take() {
            unsafe { NSEvent::removeMonitor(&monitor) };
        }
        unsafe {
            NSNotificationCenter::defaultCenter().removeObserver(self.observer.as_ref());
        }
    }
}

/// Whether a click at `at`, in window coordinates, ends the editing in
/// `window`: a field is being edited and the click lands outside it.
pub(super) fn ends_editing(window: &NSWindow, at: CGPoint) -> bool {
    let Some(control) = editing_control(window) else {
        return false;
    };
    // The frame view, which also holds the toolbar over the content, has no
    // superview: its hit test takes window coordinates.
    let hit = window
        .contentView()
        .and_then(|content| unsafe { content.superview() })
        .and_then(|frame| frame.hitTest(at));
    !hit.is_some_and(|view| view.isDescendantOf(&control))
}

/// The control whose text is being edited in `window`.
fn editing_control(window: &NSWindow) -> Option<Retained<NSControl>> {
    let editor = window.firstResponder()?.downcast::<NSTextView>().ok()?;
    if !editor.isFieldEditor() {
        return None;
    }
    let delegate = editor.delegate()?;
    let control = AsRef::<AnyObject>::as_ref(&*delegate).downcast_ref::<NSControl>()?;
    Some(control.retain())
}
