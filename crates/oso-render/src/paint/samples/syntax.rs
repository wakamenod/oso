//! A sample frame of highlighted Rust and Markdown in each theme, as the author judged its
//! syntax colours by (ADR 0111). The highlights are marked by hand, as the queries take them,
//! since the renderer has no parser. The cursor's row has the current line's band, and a region
//! and isearch's matches show the colour giving way.
//!
//! Must not depend on: `capture` or the control socket.

use std::path::Path;

use oso_core::buffer::Buffer;
use oso_core::display_map::DisplayPoint;
use oso_core::frame::CellRect;
use oso_core::syntax::{Highlight, Highlights};
use oso_core::window::{Screen, ViewSize, ViewTop};

use super::{COLUMNS, ROWS, cells, draw, write};
use crate::pane::{Bands, CursorStyle, Face, Layer, Pane, PaneContent, PaneId, TextView};
use crate::theme::Theme;

/// Rust and Markdown, each highlighted part as `⟨class:text⟩`.
const MARKED: &str = "⟨c:// Loads a theme by name, or gives none.⟩
⟨m:#[derive(Debug, Clone)]⟩
⟨k:pub⟩ ⟨k:struct⟩ ⟨t:Theme⟩ {
    ⟨p:name⟩: &⟨k:'static⟩ ⟨t:str⟩,
    ⟨p:size⟩: ⟨t:u32⟩,
}

⟨k:impl⟩ ⟨t:Theme⟩ {
    ⟨c:/// The theme called `name`, if there is one.⟩
    ⟨k:pub⟩ ⟨k:fn⟩ ⟨f:load⟩(name: &⟨t:str⟩) -> ⟨t:Option⟩<⟨t:Self⟩> {
        ⟨k:let⟩ size = ⟨n:13⟩ * ⟨n:2⟩;
        ⟨m:log::info!⟩(⟨s:\"loading {name}\\n\"⟩);
        ⟨k:if⟩ name.⟨f:is_empty⟩() || size > ⟨n:MAX_SIZE⟩ {
            ⟨k:return⟩ ⟨t:None⟩;
        }
        ⟨t:Some⟩(⟨t:Self⟩ { ⟨p:name⟩: ⟨s:\"dark\"⟩, size })
    }
}

⟨u:#⟩ ⟨h:Themes⟩

Each theme is ⟨u:*a palette*⟩ of ⟨u:**named colours**⟩, as ⟨l:[ADR 0089](0089.md)⟩ says.
⟨u:-⟩ ⟨s:`dark`⟩ is the default, and ⟨s:`dusk`⟩ the M2 steps' grey.
⟨u:>⟩ A face maps to a colour, never to a hue.
";

/// The highlight a mark's letter names.
fn class(letter: char) -> Highlight {
    match letter {
        'c' => Highlight::Comment,
        's' => Highlight::String,
        'k' => Highlight::Keyword,
        't' => Highlight::Type,
        'f' => Highlight::Function,
        'n' => Highlight::Constant,
        'p' => Highlight::Property,
        'm' => Highlight::Macro,
        'h' => Highlight::Heading,
        'l' => Highlight::Link,
        _ => Highlight::Markup,
    }
}

/// [`MARKED`]'s text and its highlights.
fn sample() -> (String, Highlights) {
    let mut text = String::new();
    let mut runs = Vec::new();
    let mut rest = MARKED;
    while let Some(open) = rest.find('⟨') {
        text.push_str(&rest[..open]);
        let mark = &rest[open + '⟨'.len_utf8()..];
        let close = mark.find('⟩').unwrap();
        let letter = mark.chars().next().unwrap();
        let start = text.len();
        text.push_str(&mark[2..close]);
        runs.push((start..text.len(), class(letter)));
        rest = &mark[close + '⟩'.len_utf8()..];
    }
    text.push_str(rest);
    let mut highlights = Highlights::new(text.len());
    for (range, highlight) in runs {
        highlights.replace(range.clone(), [(range.len(), Some(highlight))]);
    }
    (text, highlights)
}

/// Where `word` is on each line of `text`, as frame cells. Every line is ASCII before it.
fn found(text: &str, word: &str) -> Vec<CellRect> {
    let mut found = Vec::new();
    for (row, line) in text.lines().enumerate() {
        let mut from = 0;
        while let Some(at) = line[from..].find(word) {
            found.push(cells(row, from + at, 1, word.len()));
            from += at + word.len();
        }
    }
    found
}

/// The sample in `theme`, with the cursor on `load`'s row.
fn frame(theme: Theme) -> Option<(u32, u32, Vec<u8>)> {
    let (text, highlights) = sample();
    let buffer = Buffer::from_text("sample.rs", &text);
    let mut screen = Screen::default();
    let size = ViewSize::new(ROWS, COLUMNS);
    screen.show_lines(&buffer.display(), ViewTop::default(), size);
    let mut matches = found(&text, "name");
    let current = [matches.remove(2)];
    let band = [cells(9, 0, 1, COLUMNS)];
    let region = [cells(10, 8, 1, 21), cells(11, 0, 1, 30)];
    let fills: [(Face, &[CellRect]); 4] = [
        (Face::CurrentLine, &band),
        (Face::SearchMatch, &matches),
        (Face::SearchCurrent, &current),
        (Face::Region, &region),
    ];
    let pane = Pane {
        id: PaneId(0),
        region: cells(0, 0, ROWS, COLUMNS),
        layer: Layer::TILED,
        background: Some(Face::Text),
        panel: None,
        bands: Bands::NONE,
        content: PaneContent::Text(TextView {
            display: buffer.display(),
            buffer: oso_core::editor::Editor::default().window().buffer(),
            revision: 1,
            screen: &screen,
            cursor: DisplayPoint::new(9, 11),
            cursor_style: CursorStyle::Box,
            others: &[],
            preedit: None,
            fills: &fills,
            labels: &[],
            whitespace: false,
            flash: None,
            highlights: Some(&highlights),
        }),
    };
    draw(&[pane], theme)
}

/// Writes `theme`'s sample as `<theme>-syntax.rgba`.
pub(super) fn write_sample(dir: &Path, theme: Theme) {
    if let Some(frame) = frame(theme) {
        write(dir, &format!("{}-syntax", theme.name()), frame);
    }
}
