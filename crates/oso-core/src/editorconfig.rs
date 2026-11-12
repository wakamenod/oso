//! EditorConfig (ADR 0115): reading a `.editorconfig` file's text, and the indentation it gives
//! a file, as the specification (0.17.2) and editorconfig-core-test have it.
//!
//! Must not depend on: `indent`, `buffer`, `editor` or file I/O; it works on text and paths
//! alone. The host finds the files, from the file's directory up, and reads them.
//!
//! A file is INI: a preamble whose `root = true` stops the search above it, then sections, each
//! a glob ([`glob`]) and the pairs it sets. Keys and the values of the properties read here are
//! taken in any case, and `unset` takes a property's value away. Only `indent_style`,
//! `indent_size` and `tab_width` are kept; the others are parsed and passed over. Files are
//! applied farthest first, so a nearer one wins, and in a file a later section wins.
//!
//! Each file is bounded: at most [`MAX_FILE`] bytes, [`MAX_SECTIONS`] sections, a glob of
//! [`MAX_GLOB`] bytes with braces [`MAX_NESTING`] deep, and the `regex` crate's size limit. A
//! section past one is skipped, and the file says so in its [`Problem`]s. A section that sets
//! none of the three properties is never compiled. One visit compiles at most
//! [`VISIT_SECTIONS`] sections and [`VISIT_GLOBS`] bytes of globs across all its files, which
//! share a [`Budget`]; the sections past it are skipped too.

mod glob;

use std::fmt;
use std::path::{Path, PathBuf};

use glob::{Glob, Refused};
pub use glob::{MAX_GLOB, MAX_NESTING, MAX_RANGE_GLOB};

/// The most bytes of a `.editorconfig` read; a larger one is not read at all.
pub const MAX_FILE: usize = 64 << 10;

/// The most sections of a file taken; the rest are skipped.
pub const MAX_SECTIONS: usize = 1000;

/// The most sections one visit compiles, across all the files of its walk.
pub const VISIT_SECTIONS: usize = 128;

/// The most bytes of globs one visit compiles, across all the files of its walk.
pub const VISIT_GLOBS: usize = 2 << 10;

/// The largest `indent_size` and `tab_width` taken, as the indentation setting's own bound.
const MAX_WIDTH: u8 = 32;

/// `indent_style`: what indentation is made of.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IndentStyle {
    Space,
    /// As many tabs as fit, then spaces.
    Tab,
}

/// `indent_size`: the columns of a level, or `tab`, the width of a tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IndentSize {
    Columns(u8),
    Tab,
}

/// What a pair sets a property to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Setting<T> {
    Value(T),
    /// A value the property does not take: it hides the files' farther values, and gives
    /// nothing itself.
    Invalid,
    /// `unset`: the property has no value, whatever came before.
    Unset,
}

/// One of the pairs kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pair {
    Style(Setting<IndentStyle>),
    Size(Setting<IndentSize>),
    TabWidth(Setting<u8>),
}

/// A section: its glob, and the pairs it sets, in order.
#[derive(Debug)]
struct Section {
    glob: Glob,
    pairs: Vec<Pair>,
}

/// Why part of a file was not taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Problem {
    /// The file is larger than [`MAX_FILE`], and was not read.
    TooLarge,
    /// The section at this line, 1-based, and those after it are past [`MAX_SECTIONS`].
    TooManySections { line: usize },
    /// The section at this line has a glob longer than [`MAX_GLOB`].
    GlobTooLong { line: usize },
    /// The section at this line has braces nested deeper than [`MAX_NESTING`].
    NestedTooDeep { line: usize },
    /// The section at this line has a glob whose expression is too large.
    TooComplex { line: usize },
    /// The section at this line has a glob with a number range longer than
    /// [`MAX_RANGE_GLOB`].
    RangeGlobTooLong { line: usize },
    /// The section at this line, and those after it, are past what one visit compiles.
    OverBudget { line: usize },
}

/// What one visit may still compile, shared by the files of its walk, nearest first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Budget {
    sections: usize,
    glob_bytes: usize,
    /// Whether a section past it was skipped, which is said once a visit.
    said: bool,
}

impl Default for Budget {
    /// [`VISIT_SECTIONS`] sections and [`VISIT_GLOBS`] bytes of globs.
    fn default() -> Self {
        Self {
            sections: VISIT_SECTIONS,
            glob_bytes: VISIT_GLOBS,
            said: false,
        }
    }
}

impl Budget {
    /// Whether it is spent: no later section is compiled, so no later file gives anything.
    pub fn is_spent(&self) -> bool {
        self.sections == 0
    }

    /// Takes a section of `glob_bytes` from the budget, if it fits; once one does not, none
    /// does.
    fn take(&mut self, glob_bytes: usize) -> bool {
        if self.sections == 0 || glob_bytes > self.glob_bytes {
            self.sections = 0;
            self.glob_bytes = 0;
            return false;
        }
        self.sections -= 1;
        self.glob_bytes -= glob_bytes;
        true
    }
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge => write!(
                f,
                "it is larger than {} KiB, and is not read",
                MAX_FILE >> 10
            ),
            Self::TooManySections { line } => write!(
                f,
                "line {line}: more than {MAX_SECTIONS} sections; this one and the rest are skipped"
            ),
            Self::GlobTooLong { line } => write!(
                f,
                "line {line}: a section name longer than {} KiB is skipped",
                MAX_GLOB >> 10
            ),
            Self::NestedTooDeep { line } => write!(
                f,
                "line {line}: a section with braces nested more than {MAX_NESTING} deep is skipped"
            ),
            Self::RangeGlobTooLong { line } => write!(
                f,
                "line {line}: a section name with a number range longer than {MAX_RANGE_GLOB} \
                 bytes is skipped"
            ),
            Self::OverBudget { line } => write!(
                f,
                "line {line}: a visit compiles at most {VISIT_SECTIONS} sections and {} KiB of \
                 globs; this section and the rest are skipped",
                VISIT_GLOBS >> 10
            ),
            Self::TooComplex { line } => {
                write!(
                    f,
                    "line {line}: a section name too complex to match is skipped"
                )
            }
        }
    }
}

/// A `.editorconfig` file, parsed.
#[derive(Debug, Default)]
pub struct File {
    root: bool,
    sections: Vec<Section>,
    problems: Vec<Problem>,
}

impl File {
    /// The file whose text is `text`, its sections compiled from `budget`. A line that is no
    /// comment, section or pair is passed over, as the specification leaves it.
    pub fn parse(text: &str, budget: &mut Budget) -> Self {
        let mut file = Self::default();
        if text.len() > MAX_FILE {
            file.problems.push(Problem::TooLarge);
            return file;
        }
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        // The section being read: its line, its glob and its pairs.
        let mut section: Option<(usize, &str, Vec<Pair>)> = None;
        let mut sections = 0;
        for (index, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.starts_with(['#', ';']) {
                continue;
            }
            if let Some(name) = line.strip_prefix('[').and_then(section_name) {
                file.finish(section.take(), budget);
                sections += 1;
                if sections > MAX_SECTIONS {
                    file.problems
                        .push(Problem::TooManySections { line: index + 1 });
                    return file;
                }
                section = Some((index + 1, name, Vec::new()));
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let (key, value) = (key.trim().to_ascii_lowercase(), value.trim());
            match &mut section {
                None if key == "root" => file.root = value.eq_ignore_ascii_case("true"),
                None => {}
                Some((_, _, pairs)) => pairs.extend(pair(&key, value)),
            }
        }
        file.finish(section, budget);
        file
    }

    /// Whether its preamble says `root = true`: no file above it counts.
    pub fn is_root(&self) -> bool {
        self.root
    }

    /// What of it was not taken, in the order of its lines.
    pub fn problems(&self) -> &[Problem] {
        &self.problems
    }

    /// Takes in the section read, if it sets anything kept and `budget` has room for it.
    fn finish(&mut self, section: Option<(usize, &str, Vec<Pair>)>, budget: &mut Budget) {
        let Some((line, name, pairs)) = section else {
            return;
        };
        if pairs.is_empty() {
            return;
        }
        if name.len() > MAX_GLOB {
            self.problems.push(Problem::GlobTooLong { line });
            return;
        }
        if !budget.take(name.len()) {
            // Said once, at the first section past it.
            if !std::mem::replace(&mut budget.said, true) {
                self.problems.push(Problem::OverBudget { line });
            }
            return;
        }
        match Glob::new(name) {
            Ok(glob) => self.sections.push(Section { glob, pairs }),
            Err(refused) => self.problems.push(match refused {
                Refused::TooLong => Problem::GlobTooLong { line },
                Refused::NestedTooDeep => Problem::NestedTooDeep { line },
                Refused::TooComplex => Problem::TooComplex { line },
                Refused::RangeTooLong => Problem::RangeGlobTooLong { line },
            }),
        }
    }
}

/// The name of a section whose header is `[` then `rest`: up to the last `]`, which ends the
/// line or comes before a comment, a `;` or `#` after a space, as editorconfig-core-c's reader
/// takes it.
fn section_name(rest: &str) -> Option<&str> {
    if let Some(name) = rest.strip_suffix(']') {
        return Some(name);
    }
    let comment = rest
        .char_indices()
        .zip(rest.chars().skip(1))
        .find(|((_, before), mark)| before.is_whitespace() && matches!(mark, ';' | '#'))
        .map(|((at, _), _)| at)?;
    rest[..comment].trim_end().strip_suffix(']')
}

/// The pair `key = value`, if it is one of those kept. `value` is taken in any case.
fn pair(key: &str, value: &str) -> Option<Pair> {
    let value = value.to_ascii_lowercase();
    Some(match key {
        "indent_style" => Pair::Style(setting(&value, |value| match value {
            "space" => Some(IndentStyle::Space),
            "tab" => Some(IndentStyle::Tab),
            _ => None,
        })),
        "indent_size" => Pair::Size(setting(&value, |value| match value {
            "tab" => Some(IndentSize::Tab),
            value => width(value).map(IndentSize::Columns),
        })),
        "tab_width" => Pair::TabWidth(setting(&value, width)),
        _ => return None,
    })
}

/// What `value` sets a property to, `parse` reading the values it takes.
fn setting<T>(value: &str, parse: impl Fn(&str) -> Option<T>) -> Setting<T> {
    match value {
        "unset" => Setting::Unset,
        value => parse(value).map_or(Setting::Invalid, Setting::Value),
    }
}

/// A width from 1 to [`MAX_WIDTH`], written in digits.
fn width(value: &str) -> Option<u8> {
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    value
        .parse::<u8>()
        .ok()
        .filter(|width| (1..=MAX_WIDTH).contains(width))
}

/// A property's value as the files give it, and which file gave it: `None` for a value it
/// does not take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Given<T> {
    value: Option<T>,
    source: usize,
}

/// The indentation properties that apply to a file, from the files applied so far. Each
/// value comes with the `source` its file was applied with.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Properties {
    style: Option<Given<IndentStyle>>,
    size: Option<Given<IndentSize>>,
    tab_width: Option<Given<u8>>,
}

impl Properties {
    /// Applies what `file` sets for `path`, the file's path from that file's directory, its
    /// parts joined by `/`, over what the files before it set. Files go farthest first.
    fn apply(&mut self, file: &File, source: usize, path: &str) {
        // Matched with a `/` before it, made once for all the sections.
        let path = format!("/{path}");
        for section in &file.sections {
            if !section.glob.matches(&path) {
                continue;
            }
            for pair in &section.pairs {
                match *pair {
                    Pair::Style(setting) => self.style = given(setting, source),
                    Pair::Size(setting) => self.size = given(setting, source),
                    Pair::TabWidth(setting) => self.tab_width = given(setting, source),
                }
            }
        }
    }

    /// `indent_style`, and the source of the file that set it.
    pub fn indent_style(&self) -> Option<(IndentStyle, usize)> {
        let style = self.style?;
        Some((style.value?, style.source))
    }

    /// `indent_size`, and the source of the file that set it. With none set, `indent_style =
    /// tab` makes it `tab`, as EditorConfig's cores do.
    pub fn indent_size(&self) -> Option<(IndentSize, usize)> {
        match self.size {
            Some(size) => Some((size.value?, size.source)),
            None => match self.indent_style()? {
                (IndentStyle::Tab, source) => Some((IndentSize::Tab, source)),
                (IndentStyle::Space, _) => None,
            },
        }
    }

    /// `tab_width`, and the source of the file that set it. With none set, a number given as
    /// `indent_size` is the tab width too, as the specification has it.
    pub fn tab_width(&self) -> Option<(u8, usize)> {
        match self.tab_width {
            Some(width) => Some((width.value?, width.source)),
            None => match self.size? {
                Given {
                    value: Some(IndentSize::Columns(columns)),
                    source,
                } => Some((columns, source)),
                _ => None,
            },
        }
    }
}

/// What a property is after a pair set it to `setting`, in the file applied as `source`.
fn given<T>(setting: Setting<T>, source: usize) -> Option<Given<T>> {
    match setting {
        Setting::Value(value) => Some(Given {
            value: Some(value),
            source,
        }),
        Setting::Invalid => Some(Given {
            value: None,
            source,
        }),
        Setting::Unset => None,
    }
}

/// What the `.editorconfig` files found for a file give it: the properties, each with its
/// source, which is the index of its file, nearest first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Applied {
    properties: Properties,
    files: Vec<PathBuf>,
}

impl Applied {
    /// What the `.editorconfig` files a walk up from the file at `location` found give it:
    /// each file by its path, nearest first, as a walk finds them. They apply farthest first,
    /// so a nearer one wins, each with its globs read from its own directory.
    pub fn from_walk(location: &Path, files: Vec<(PathBuf, File)>) -> Self {
        let mut properties = Properties::default();
        for (source, (path, file)) in files.iter().enumerate().rev() {
            let dir = path.parent().unwrap_or(path);
            properties.apply(file, source, &relative(location, dir));
        }
        Self {
            properties,
            files: files.into_iter().map(|(path, _)| path).collect(),
        }
    }

    /// The properties, each with the source of the file that gave it.
    pub fn properties(&self) -> &Properties {
        &self.properties
    }

    /// The path of the file `source`, as the properties name it.
    pub fn file(&self, source: usize) -> Option<&Path> {
        self.files.get(source).map(PathBuf::as_path)
    }
}

/// The path of `location` from `dir`, its parts joined by `/`, as the globs read it.
fn relative(location: &Path, dir: &Path) -> String {
    let parts = location.strip_prefix(dir).unwrap_or(location).iter();
    let parts: Vec<_> = parts.map(|part| part.to_string_lossy()).collect();
    parts.join("/")
}

#[cfg(test)]
mod tests;
