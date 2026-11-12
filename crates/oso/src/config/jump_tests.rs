//! The tests of `[jump]` in [`parse`](super::parse): its keys and its timeout, and what is
//! reported of bad ones (ADR 0116).
//!
//! Must not depend on: `app`, `dispatch` or the file system; these run on strings alone.

use std::time::Duration;

use oso_core::jump::Keys;

use super::settings::Jump;
use super::tests::{Fixture, messages};

#[test]
fn jump_keys_are_distinct_printable_keys_none_an_actions() {
    let f = Fixture::new();
    for keys in ["asdfghjl", "ab", "aoeu;,."] {
        let loaded = f.parse(&format!("[jump]\nkeys = {keys:?}\n"));
        assert_eq!(messages(&loaded), Vec::<String>::new());
        assert_eq!(loaded.settings.jump.keys, Some(Keys::new(keys).unwrap()));
    }
    let long = format!("{:?}", "a".repeat(33));
    for (keys, message) in [
        (r#""a""#, "takes 2 to 32 characters, not 1"),
        (r#""""#, "takes 2 to 32 characters, not 0"),
        (&long, "takes 2 to 32 characters, not 33"),
        (r#""asda""#, "gives 'a' twice"),
        (r#""asK""#, "cannot take 'K', an action's key"),
        (r#""as d""#, "takes printable ASCII characters, not ' '"),
        (r#""asé""#, "takes printable ASCII characters, not 'é'"),
        ("1", "takes a string of keys, not a number"),
        (r#"["a", "s"]"#, "takes a string of keys, not a list"),
    ] {
        let loaded = f.parse(&format!("[jump]\nkeys = {keys}\n"));
        assert_eq!(messages(&loaded), [format!("2:8: [jump] keys {message}")]);
        assert_eq!(loaded.settings.jump.keys, None, "{keys}");
    }
}

#[test]
fn a_jump_timeout_is_seconds_from_a_tenth_to_five() {
    let f = Fixture::new();
    for (timeout, millis) in [
        ("1", 1000),
        ("0.3", 300),
        ("0.1", 100),
        ("5", 5000),
        ("5.0", 5000),
    ] {
        let loaded = f.parse(&format!("[jump]\ntimeout = {timeout}\n"));
        assert_eq!(messages(&loaded), Vec::<String>::new(), "{timeout}");
        assert_eq!(
            loaded.settings.jump.timeout,
            Some(Duration::from_millis(millis)),
            "{timeout}"
        );
    }
    for (timeout, not) in [
        ("0.05", "0.05"),
        ("7", "7"),
        ("0", "0"),
        ("-1", "-1"),
        ("nan", "nan"),
        ("inf", "inf"),
        ("1e300", "1e300"),
        ("99999999999999999999", "99999999999999999999"),
        ("0x10", "0x10"),
        (r#""0.5""#, "a string"),
        ("true", "true or false"),
    ] {
        let loaded = f.parse(&format!("[jump]\ntimeout = {timeout}\n"));
        assert_eq!(
            messages(&loaded),
            [format!(
                "2:11: [jump] timeout takes a number of seconds from 0.1 to 5, not {not}"
            )],
            "{timeout}"
        );
        assert_eq!(loaded.settings.jump.timeout, None, "{timeout}");
    }
    let loaded = f.parse("jump = 1\n");
    assert_eq!(messages(&loaded), ["1:8: [jump] must be a table"]);
    assert_eq!(loaded.settings.jump, Jump::default());
}
