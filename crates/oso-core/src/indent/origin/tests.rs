//! EditorConfig over the configuration over the default, property by property.
//!
//! Must not depend on: `buffer` or `editor`; they run on settings alone.

use super::{Origin, Origins, with_editorconfig};
use crate::editorconfig::{Budget, File, Properties};
use crate::indent::{Settings, Style};

/// What `text`, an `.editorconfig`, gives a file `a.rs` beside it.
fn properties(text: &str) -> Properties {
    let mut properties = Properties::default();
    properties.apply(&File::parse(text, &mut Budget::default()), 0, "a.rs");
    properties
}

/// Two spaces from the configuration, with the default tab width.
fn configured() -> (Settings, Origins) {
    let origins = Origins {
        indent_size: Origin::Configuration,
        style: Origin::Configuration,
        tab_width: Origin::Default,
    };
    (Settings::new(2, Style::Spaces, 8).unwrap(), origins)
}

#[test]
fn nothing_from_editorconfig_keeps_the_configuration() {
    let (settings, origins) = configured();
    assert_eq!(
        with_editorconfig(settings, origins, &Properties::default()),
        (settings, origins)
    );
}

#[test]
fn each_property_editorconfig_gives_wins_on_its_own() {
    let (settings, origins) = configured();
    let all = properties("[*]\nindent_style = tab\ntab_width = 4\nindent_size = 3\n");
    let (got, from) = with_editorconfig(settings, origins, &all);
    assert_eq!(got, Settings::new(3, Style::Tabs, 4).unwrap());
    assert_eq!(
        from,
        Origins {
            indent_size: Origin::EditorConfig,
            style: Origin::EditorConfig,
            tab_width: Origin::EditorConfig,
        }
    );
    let tabs = properties("[*]\nindent_style = tab\n");
    let (got, from) = with_editorconfig(settings, origins, &tabs);
    assert_eq!(
        got,
        Settings::new(8, Style::Tabs, 8).unwrap(),
        "indent_style = tab makes a level one tab of the configured width"
    );
    assert_eq!(
        (from.indent_size, from.style, from.tab_width),
        (Origin::EditorConfig, Origin::EditorConfig, Origin::Default)
    );
}

#[test]
fn a_numeric_indent_size_sets_the_tab_width_too() {
    let (settings, origins) = configured();
    let size = properties("[*.rs]\nindent_size = 4\n");
    let (got, from) = with_editorconfig(settings, origins, &size);
    assert_eq!(got, Settings::new(4, Style::Spaces, 4).unwrap());
    assert_eq!(from.tab_width, Origin::EditorConfig);
    assert_eq!(from.style, Origin::Configuration);
}

#[test]
fn an_invalid_value_leaves_the_configurations() {
    let (settings, origins) = configured();
    let invalid = properties("[*]\nindent_size = 40\nindent_style = wide\n");
    assert_eq!(
        with_editorconfig(settings, origins, &invalid),
        (settings, origins)
    );
}
