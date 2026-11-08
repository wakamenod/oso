//! The macOS clipboard: the general `NSPasteboard` (ADR 0074).
//!
//! Must not depend on: the window layer; this needs no window and runs on its own thread.
//!
//! One pasteboard item carries the text as `public.utf8-plain-text`, and for text that
//! may be secret, the markers of nspasteboard.org that clipboard managers skip:
//! `org.nspasteboard.ConcealedType` and `org.nspasteboard.TransientType`. Text read is
//! concealed if the pasteboard carries the first, as a password manager's does.
//!
//! Apple documents no rule for the threads `NSPasteboard` may run on; it is used only
//! from the clipboard's thread here, never from two at once. Each call runs in an
//! autorelease pool of its own, as that thread has none.

use objc2::rc::autoreleasepool;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{NSPasteboard, NSPasteboardItem};
use objc2_foundation::{NSArray, NSString};

use super::{Backend, Found};

/// The type of plain text, the value of `NSPasteboardTypeString`.
const PLAIN_TEXT: &str = "public.utf8-plain-text";

/// What clipboard managers that follow nspasteboard.org leave out of their history.
const CONCEALED: &str = "org.nspasteboard.ConcealedType";
const TRANSIENT: &str = "org.nspasteboard.TransientType";

/// The general pasteboard.
pub(super) struct Pasteboard;

impl Backend for Pasteboard {
    fn change_count(&mut self) -> Option<i64> {
        autoreleasepool(|_| {
            NSPasteboard::generalPasteboard()
                .changeCount()
                .try_into()
                .ok()
        })
    }

    fn write(&mut self, text: &str, concealed: bool) -> Option<i64> {
        autoreleasepool(|_| {
            let pasteboard = NSPasteboard::generalPasteboard();
            pasteboard.clearContents();
            let item = NSPasteboardItem::new();
            item.setString_forType(&NSString::from_str(text), &NSString::from_str(PLAIN_TEXT));
            if concealed {
                let empty = NSString::from_str("");
                for marker in [CONCEALED, TRANSIENT] {
                    item.setString_forType(&empty, &NSString::from_str(marker));
                }
            }
            let items = NSArray::from_retained_slice(&[ProtocolObject::from_retained(item)]);
            pasteboard.writeObjects(&items);
            pasteboard.changeCount().try_into().ok()
        })
    }

    fn read(&mut self, max: usize) -> Found {
        autoreleasepool(|_| {
            let pasteboard = NSPasteboard::generalPasteboard();
            let marker = NSString::from_str(CONCEALED);
            let concealed = pasteboard
                .types()
                .is_some_and(|types| types.containsObject(&marker));
            // This can wait on the app that copied the text, if it hands it over lazily, and
            // copies it whole: the pasteboard has no cheap way to ask its size first.
            let Some(text) = pasteboard.stringForType(&NSString::from_str(PLAIN_TEXT)) else {
                return Found::Nothing;
            };
            // A UTF-16 unit is at least one UTF-8 byte, so the cheap count goes first.
            if text.length() > max || text.len() > max {
                return Found::TooLarge;
            }
            Found::Text {
                text: text.to_string(),
                concealed,
            }
        })
    }
}
