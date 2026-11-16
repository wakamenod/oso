//! The ids of the requests pasted into a session.
//!
//! Must not depend on: winit or the window; these are the ids' own tests.

use super::request_id;

#[test]
fn an_id_is_eight_hex_digits_and_they_differ() {
    let (one, two) = (request_id().unwrap(), request_id().unwrap());
    assert_eq!(one.len(), 8);
    assert!(one.chars().all(|c| c.is_ascii_hexdigit()), "{one}");
    assert_ne!(one, two);
}
