//! Small macOS calls GPUI doesn't expose: keyboard/mouse state, the frontmost
//! app, moving a window, and click-through windows.

use gpui_kit::Window;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{class, msg_send};
use objc2_core_foundation::CGPoint;
use objc2_foundation::NSString;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventSourceFlagsState(state: i32) -> u64;
    fn CGEventSourceButtonState(state: i32, button: u32) -> bool;
    fn CGEventSourceCounterForEventType(state: i32, event_type: u32) -> u32;
}

const HID_STATE: i32 = 1;
const KEY_DOWN: u32 = 10;
pub const SHIFT: u64 = 0x0002_0000;
pub const CONTROL: u64 = 0x0004_0000;
pub const OPTION: u64 = 0x0008_0000;
pub const COMMAND: u64 = 0x0010_0000;

/// Modifier keys currently held (CGEventFlags).
pub fn modifiers() -> u64 {
    unsafe { CGEventSourceFlagsState(HID_STATE) }
}

pub fn left_button_down() -> bool {
    unsafe { CGEventSourceButtonState(HID_STATE, 0) }
}

/// Key presses so far this session; a change means the user typed something.
pub fn key_down_count() -> u32 {
    unsafe { CGEventSourceCounterForEventType(HID_STATE, KEY_DOWN) }
}

/// Bundle identifier of the app in front.
pub fn frontmost_bundle_id() -> Option<String> {
    unsafe {
        let ws: Option<Retained<AnyObject>> = msg_send![class!(NSWorkspace), sharedWorkspace];
        let app: Option<Retained<AnyObject>> = msg_send![&*ws?, frontmostApplication];
        let id: Option<Retained<NSString>> = msg_send![&*app?, bundleIdentifier];
        Some(id?.to_string())
    }
}

fn ns_window(window: &Window) -> Option<*mut AnyObject> {
    let handle = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::AppKit(h) = handle.as_raw() else { return None };
    let view = h.ns_view.as_ptr() as *mut AnyObject;
    let win: *mut AnyObject = unsafe { msg_send![view, window] };
    (!win.is_null()).then_some(win)
}

/// Let clicks pass through the window to whatever is below it.
pub fn set_click_through(window: &Window) {
    if let Some(w) = ns_window(window) {
        unsafe {
            let _: () = msg_send![w, setIgnoresMouseEvents: true];
        }
    }
}

/// Move a window so its top-left corner is at `(x, y)` in global points
/// (origin at the top-left of the main display, like CoreGraphics).
pub fn move_window(window: &Window, x: f64, y: f64) {
    let Some(w) = ns_window(window) else { return };
    let main_h = core_graphics::display::CGDisplay::main().bounds().size.height;
    let h = f64::from(window.bounds().size.height);
    // Cocoa screen coordinates start at the bottom-left of the main display.
    let origin = CGPoint { x, y: main_h - y - h };
    unsafe {
        let _: () = msg_send![w, setFrameOrigin: origin];
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_input_state_and_front_app() {
        let id = super::frontmost_bundle_id();
        assert!(id.as_deref().is_some_and(|s| !s.is_empty()), "{id:?}");
        let _ = (super::modifiers(), super::left_button_down(), super::key_down_count());
    }
}
