//! Where each property of a buffer's indentation setting comes from (ADR 0115): the
//! EditorConfig files that apply to its file, over the configuration's `[modes.<mode>]`, over
//! the compiled-in default.
//!
//! Must not depend on: `buffer` or `editor`; the editor keeps what each buffer was given.

use super::{Settings, Style};
use crate::display_map::TabWidth;
use crate::editorconfig::{IndentSize, IndentStyle, Properties};

/// Where a property of an indentation setting came from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Origin {
    /// An `.editorconfig` that applies to the file.
    EditorConfig,
    /// The configuration's `[modes.<mode>]`.
    Configuration,
    /// The compiled-in default.
    #[default]
    Default,
    /// The tab width the file was visited with, which a configuration or a name changed since
    /// would set otherwise: it applies at the next visit.
    Visited,
}

/// Where each property of an indentation setting came from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Origins {
    pub indent_size: Origin,
    pub style: Origin,
    pub tab_width: Origin,
}

/// The setting `properties`, from EditorConfig, give over `configured`, whose properties came
/// from `given`, with where each came from. `indent_size = tab` is the tab width it gives.
pub fn with_editorconfig(
    configured: Settings,
    given: Origins,
    properties: &Properties,
) -> (Settings, Origins) {
    let mut origins = given;
    let style = match properties.indent_style() {
        Some((style, _)) => {
            origins.style = Origin::EditorConfig;
            match style {
                IndentStyle::Space => Style::Spaces,
                IndentStyle::Tab => Style::Tabs,
            }
        }
        None => configured.style(),
    };
    let tab_width = match properties.tab_width() {
        Some((width, _)) => {
            origins.tab_width = Origin::EditorConfig;
            TabWidth::new(u32::from(width))
        }
        None => configured.tab_width(),
    };
    let indent_size = match properties.indent_size() {
        Some((size, _)) => {
            origins.indent_size = Origin::EditorConfig;
            match size {
                IndentSize::Columns(columns) => u32::from(columns),
                IndentSize::Tab => tab_width.get(),
            }
        }
        None => u32::try_from(configured.indent_size()).unwrap_or(4),
    };
    match Settings::new(indent_size, style, tab_width.get()) {
        Some(settings) => (settings, origins),
        None => (configured, given),
    }
}

#[cfg(test)]
mod tests;
