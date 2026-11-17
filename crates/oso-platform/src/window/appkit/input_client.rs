//! What the view's `NSTextInputClient` methods turn AppKit's values into and out of: an input
//! method's string as text, `NSNotFound`, and a place in the view as one on the screen
//! (ADR 0070).
//!
//! Must not depend on: the run loop's hooks, the delegates or the view's state.

use objc2::runtime::AnyObject;
use objc2_app_kit::NSView;
use objc2_foundation::{
    NSAttributedString, NSNotFound, NSPoint, NSRect, NSSize, NSString, NSUInteger,
};

use crate::window::CursorArea;

/// `area`, in physical pixels of `view`, whose backing `scale` it is, in points on the screen.
pub(super) fn on_screen(view: &NSView, scale: f64, area: CursorArea) -> NSRect {
    let rect = NSRect::new(
        NSPoint::new(f64::from(area.x) / scale, f64::from(area.y) / scale),
        NSSize::new(
            f64::from(area.width) / scale,
            f64::from(area.height) / scale,
        ),
    );
    let in_window = view.convertRect_toView(rect, None);
    view.window()
        .map_or(in_window, |window| window.convertRectToScreen(in_window))
}

/// The text of what an input method gives, which is a string or an attributed one.
pub(super) fn text_of(string: &AnyObject) -> String {
    if let Some(attributed) = string.downcast_ref::<NSAttributedString>() {
        attributed.string().to_string()
    } else if let Some(plain) = string.downcast_ref::<NSString>() {
        plain.to_string()
    } else {
        String::new()
    }
}

/// AppKit's `NSNotFound`, as an unsigned location.
#[expect(clippy::cast_sign_loss, reason = "NSNotFound is NSIntegerMax")]
pub(super) fn not_found() -> NSUInteger {
    NSNotFound as NSUInteger
}
