//! Sample frames of every face in each theme, drawn off screen with the OS's
//! font, for the author to pick colours from (ADR 0089), split windows' modelines among
//! them (ADR 0112). Run with `OSO_THEME_SAMPLES=<dir>
//! cargo test -p oso-render -- --ignored theme_samples`: it writes each frame's RGBA pixels
//! and a table of the contrast of each face's pairs.
//!
//! Must not depend on: `capture` or the control socket.

use std::fmt::Write as _;
use std::path::Path;

use oso_core::buffer::Buffer;
use oso_core::display_map::DisplayPoint;
use oso_core::frame::CellRect;
use oso_core::window::{Screen, ViewSize, ViewTop};
use oso_platform::text::{self, FontRequest};

use crate::pane::{
    Bands, CursorStyle, Face, FlashFill, Layer, Pane, PaneContent, PaneId, Panel, Span, TextView,
};
use crate::theme::{Theme, themes};

mod syntax;

const COLUMNS: usize = 80;
const ROWS: usize = 24;
/// The text area's rows; the modeline and the echo area take the last two.
const TEXT_ROWS: usize = ROWS - 2;

const TEXT: &str = "// A sample of every face, in each theme
fn load_theme(name: &str) -> Theme {
    let theme = find(name).unwrap_or_default();
    log!(\"loaded {theme}\r\");
    let tag = \"a\u{202e}\u{2066}b\";
    let bell = \"\u{7}\";
\tlet wide = \"\u{3000}\";   
    theme
}
";

/// The gutter's columns in the frame that shows line numbers (ADR 0097).
const GUTTER: usize = 4;

/// Two other cursors, on the rows below the primary's (ADR 0091).
const OTHERS: [CellRect; 2] = [cells(4, 10, 1, 1), cells(5, 10, 1, 1)];

const fn cells(row: usize, column: usize, rows: usize, columns: usize) -> CellRect {
    CellRect {
        row,
        column,
        rows,
        columns,
    }
}

/// Where "theme" is on each line of [`TEXT`], as frame cells. Every line is ASCII before it.
fn matches() -> Vec<CellRect> {
    let mut found = Vec::new();
    for (row, line) in TEXT.lines().enumerate() {
        let mut from = 0;
        while let Some(at) = line[from..].find("theme") {
            found.push(cells(row, from + at, 1, 5));
            from += at + 5;
        }
    }
    found
}

/// The rows the sample's text pane shows of `buffer`, from its top.
fn sample_screen(buffer: &Buffer) -> Screen {
    let mut screen = Screen::default();
    let size = ViewSize::new(TEXT_ROWS, COLUMNS);
    screen.show_lines(&buffer.display(), ViewTop::default(), size);
    screen
}

/// The text pane from `column`, after a gutter of that many columns, with the cells of `fills`,
/// the other cursors and `flash` given in the frame's columns.
fn text_pane<'a>(
    (buffer, screen): (&Buffer, &'a Screen),
    id: oso_core::buffer::BufferId,
    (column, others): (usize, &'a [CellRect]),
    fills: &'a [(Face, &'a [CellRect])],
    flash: Option<FlashFill>,
) -> Pane<'a> {
    Pane {
        id: PaneId(0),
        region: cells(0, column, TEXT_ROWS, COLUMNS - column),
        layer: Layer::TILED,
        background: Some(Face::Text),
        panel: None,
        bands: Bands::NONE,
        content: PaneContent::Text(TextView {
            display: buffer.display(),
            buffer: id,
            revision: 1,
            screen,
            cursor: DisplayPoint::new(3, 10),
            cursor_style: CursorStyle::Box,
            others,
            preedit: None,
            fills,
            labels: &[],
            whitespace: true,
            flash,
            highlights: None,
        }),
    }
}

fn lines<'a>(id: u32, region: CellRect, layer: Layer, lines: &'a [&'a [Span<'a>]]) -> Pane<'a> {
    // The modeline's row is filled in its face, as the app fills it.
    let background = (layer == Layer::TILED).then_some(if region.row == TEXT_ROWS {
        Face::Modeline
    } else {
        Face::Text
    });
    Pane {
        id: PaneId(id),
        region,
        layer,
        background,
        panel: None,
        bands: Bands::NONE,
        content: PaneContent::Lines(lines),
    }
}

fn span(text: &str, face: Face) -> Span<'_> {
    Span { text, face }
}

/// Draws `panes` in `theme` with the OS's font at twice its size, as a Retina display does.
fn draw(panes: &[Pane<'_>], theme: Theme) -> Option<(u32, u32, Vec<u8>)> {
    let request = FontRequest {
        families: &["SF Mono", "Menlo"],
        size_pt: 13.0,
    };
    let mut system = text::load(&request, 2.0).ok()?;
    let metrics = system.metrics();
    // The frame's margin, 8 pixels on each side.
    let width = (COLUMNS as f32 * metrics.width).ceil() as u32 + 16;
    let height = (ROWS as f32 * metrics.height).ceil() as u32 + 16;
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let pixels = super::tests::render(panes, &mut *system, theme, (width, height, format));
    Some((width, height, pixels))
}

/// The buffer with the region, isearch's matches and a completion's float over it.
fn completion_frame(theme: Theme) -> Option<(u32, u32, Vec<u8>)> {
    let buffer = Buffer::from_text("sample.rs", TEXT);
    let screen = sample_screen(&buffer);
    let id = oso_core::editor::Editor::default().window().buffer();
    let found = matches();
    let current = [found[2]];
    let region = [cells(2, 16, 1, 10)];
    // The cursor's row gets the band, and the line after the bell its trailing fill.
    let band = [cells(3, 0, 1, COLUMNS)];
    let trailing = [cells(6, 24, 1, 3)];
    let fills = [
        (Face::CurrentLine, &band[..]),
        (Face::TrailingWhitespace, &trailing[..]),
        (Face::SearchMatch, &found[..]),
        (Face::SearchCurrent, &current[..]),
        (Face::Region, &region[..]),
    ];
    let modeline: [&[Span<'_>]; 1] = [&[
        span(" sample.rs   4:10   ", Face::Modeline),
        span("[changed on disk]", Face::Warning),
    ]];
    let echo: [&[Span<'_>]; 1] = [&[span("Mark set", Face::Text)]];
    let answer: [&[Span<'_>]; 1] = [&[
        span("4/12 ", Face::CompletionPrompt),
        span("M-x ", Face::CompletionPrompt),
        span("the", Face::Text),
    ]];
    // As main's float lays a candidate out: its binding in parentheses at one column, and what
    // is said of it at the next.
    let candidates: [&[Span<'_>]; 4] = [
        &[
            span("load-", Face::Text),
            span("the", Face::CompletionMatch),
            span("me                  ", Face::Text),
            span("Pick a theme by its name", Face::CompletionAnnotation),
        ],
        &[
            span("toggle-", Face::Text),
            span("the", Face::CompletionMatch),
            span("me      ", Face::Text),
            span("(<f5>)", Face::CompletionBinding),
            span("    ", Face::Text),
            span("Switch between the pair", Face::CompletionAnnotation),
        ],
        &[
            span("describe-", Face::Text),
            span("the", Face::CompletionMatch),
            span("me              ", Face::Text),
            span("Show a theme's faces", Face::CompletionAnnotation),
        ],
        &[
            span("customize-", Face::Text),
            span("the", Face::CompletionMatch),
            span("mes  ", Face::Text),
            span("(C-c t)", Face::CompletionBinding),
            span("   ", Face::Text),
            span("Not a command yet", Face::CompletionAnnotation),
        ],
    ];
    let outer = cells(10, 10, 9, 60);
    let panes = [
        text_pane((&buffer, &screen), id, (0, &OTHERS), &fills, None),
        lines(1, cells(TEXT_ROWS, 0, 1, COLUMNS), Layer::TILED, &modeline),
        lines(2, cells(TEXT_ROWS + 1, 0, 1, COLUMNS), Layer::TILED, &echo),
        Pane {
            id: PaneId(3),
            region: cells(0, 0, TEXT_ROWS, COLUMNS),
            layer: Layer::FLOAT,
            background: None,
            panel: Some(Panel {
                cells: outer,
                field: None,
                field_room: cells(11, 12, 1, 0),
                keys: &[],
                selected: Some(cells(14, 11, 1, 58)),
                border: true,
            }),
            bands: Bands::NONE,
            content: PaneContent::Lines(&[]),
        },
        lines(4, cells(11, 12, 1, 56), Layer::FLOAT_CONTENT, &answer),
        lines(5, cells(13, 12, 4, 56), Layer::FLOAT_CONTENT, &candidates),
    ];
    draw(&panes, theme)
}

/// The buffer with a question on the panel at the frame's top edge, line numbers in a gutter,
/// and the cursor flash at full strength over a band, below the panel, over text and
/// whitespace's marks. The band's line has the current number.
fn question_frame(theme: Theme) -> Option<(u32, u32, Vec<u8>)> {
    let buffer = Buffer::from_text("sample.rs", TEXT);
    let screen = sample_screen(&buffer);
    let id = oso_core::editor::Editor::default().window().buffer();
    let modeline: [&[Span<'_>]; 1] = [&[span(" sample.rs   4:10   ", Face::Modeline)]];
    let echo: [&[Span<'_>]; 1] = [&[span("", Face::Text)]];
    let label: [&[Span<'_>]; 1] = [&[span(
        "sample.rs changed on disk; revert it?",
        Face::Question,
    )]];
    let keys: [&[Span<'_>]; 1] = [&[
        span(" y ", Face::Key),
        span(" revert  ", Face::Prompt),
        span(" n ", Face::Key),
        span(" keep", Face::Prompt),
    ]];
    let chips = [cells(2, 6, 1, 3), cells(2, 18, 1, 3)];
    let band = [cells(6, GUTTER, 1, COLUMNS - GUTTER)];
    let fills = [(Face::CurrentLine, &band[..])];
    let flash = Some(FlashFill {
        cells: cells(6, GUTTER + 2, 1, COLUMNS - GUTTER - 2),
        strength: 1.0,
    });
    let others = OTHERS.map(|cell| cells(cell.row, cell.column + GUTTER, 1, 1));
    let numbers: Vec<String> = (1..=TEXT.lines().count() + 1)
        .map(|n| format!("{n:>3} "))
        .collect();
    let spans: Vec<[Span<'_>; 1]> = numbers
        .iter()
        .enumerate()
        .map(|(row, text)| {
            let face = if row == 6 {
                Face::LineNumberCurrent
            } else {
                Face::LineNumber
            };
            [span(text, face)]
        })
        .collect();
    let gutter: Vec<&[Span<'_>]> = spans.iter().map(|row| &row[..]).collect();
    let panes = [
        text_pane((&buffer, &screen), id, (GUTTER, &others), &fills, flash),
        lines(6, cells(0, 0, TEXT_ROWS, GUTTER), Layer::TILED, &gutter),
        lines(1, cells(TEXT_ROWS, 0, 1, COLUMNS), Layer::TILED, &modeline),
        lines(2, cells(TEXT_ROWS + 1, 0, 1, COLUMNS), Layer::TILED, &echo),
        Pane {
            id: PaneId(3),
            region: cells(0, 0, TEXT_ROWS, COLUMNS),
            layer: Layer::FLOAT,
            background: None,
            panel: Some(Panel {
                cells: cells(0, 4, 4, 72),
                field: None,
                field_room: cells(1, 6, 1, 0),
                keys: &chips,
                selected: None,
                border: false,
            }),
            bands: Bands::NONE,
            content: PaneContent::Lines(&[]),
        },
        lines(4, cells(1, 6, 1, 68), Layer::FLOAT_CONTENT, &label),
        lines(5, cells(2, 6, 1, 68), Layer::FLOAT_CONTENT, &keys),
    ];
    draw(&panes, theme)
}

fn write(dir: &Path, name: &str, (width, height, pixels): (u32, u32, Vec<u8>)) {
    let mut file = format!("{width} {height}\n").into_bytes();
    file.extend_from_slice(&pixels);
    std::fs::write(dir.join(format!("{name}.rgba")), file).unwrap();
}

#[test]
#[ignore = "draws with the OS's font on a real GPU, and writes files"]
fn theme_samples() {
    let Some(dir) = std::env::var_os("OSO_THEME_SAMPLES") else {
        return;
    };
    let dir = Path::new(&dir);
    std::fs::create_dir_all(dir).unwrap();
    let mut table = String::new();
    for theme in themes() {
        let name = theme.name();
        let (Some(completion), Some(question)) = (completion_frame(theme), question_frame(theme))
        else {
            log::warn!("no text backend on this OS: no samples");
            return;
        };
        write(dir, &format!("{name}-completion"), completion);
        write(dir, &format!("{name}-question"), question);
        if let Some(split) = split_frame(theme) {
            write(dir, &format!("{name}-split"), split);
        }
        writeln!(
            table,
            "## {name}\n\n{}",
            crate::theme::tests::ratios(&theme)
        )
        .unwrap();
        syntax::write_sample(dir, theme);
    }
    std::fs::write(dir.join("ratios.md"), table).unwrap();
}

/// A frame of two windows side by side, the left one selected with `Agent` and a warning on
/// its modeline, a divider between them, in `theme` (ADR 0112).
fn split_frame(theme: Theme) -> Option<(u32, u32, Vec<u8>)> {
    let buffer = Buffer::from_text("sample.rs", TEXT);
    let half = COLUMNS / 2;
    let size = ViewSize::new(TEXT_ROWS, half - 1);
    let mut screen = Screen::default();
    screen.show_lines(&buffer.display(), ViewTop::default(), size);
    let id = oso_core::editor::Editor::default().window().buffer();
    let band = [cells(3, 0, 1, half - 1)];
    let fills = [(Face::CurrentLine, &band[..])];
    let view = |style: CursorStyle, fills| {
        PaneContent::Text(TextView {
            display: buffer.display(),
            buffer: id,
            revision: 1,
            screen: &screen,
            cursor: DisplayPoint::new(3, 10),
            cursor_style: style,
            others: &[],
            preedit: None,
            fills,
            labels: &[],
            whitespace: true,
            flash: None,
            highlights: None,
        })
    };
    let active: [&[Span<'_>]; 1] = [&[
        span("Agent  ", Face::Prompt),
        span("[changed on disk] ", Face::Warning),
        span("-- sample.rs  ", Face::Modeline),
        span("4:10  Rust  UTF-8 LF", Face::Modeline),
    ]];
    let inactive: [&[Span<'_>]; 1] = [&[
        span("-- sample.rs  ", Face::ModelineInactive),
        span("4:10  Rust  UTF-8 LF", Face::ModelineInactive),
    ]];
    let echo: [&[Span<'_>]; 1] = [&[span("Mark set", Face::Text)]];
    let pane = tiled_pane;
    let panes = [
        pane(
            0,
            cells(0, 0, TEXT_ROWS, half - 1),
            Some(Face::Text),
            view(CursorStyle::Box, &fills[..]),
        ),
        pane(
            1,
            cells(0, half, TEXT_ROWS, half),
            Some(Face::Text),
            view(CursorStyle::Hollow, &[]),
        ),
        pane(
            2,
            cells(0, half - 1, TEXT_ROWS + 1, 1),
            Some(Face::Text),
            PaneContent::Divider,
        ),
        pane(
            3,
            cells(TEXT_ROWS, 0, 1, half - 1),
            Some(Face::Modeline),
            PaneContent::Lines(&active),
        ),
        pane(
            4,
            cells(TEXT_ROWS, half, 1, half),
            Some(Face::ModelineInactive),
            PaneContent::Lines(&inactive),
        ),
        pane(
            5,
            cells(TEXT_ROWS + 1, 0, 1, COLUMNS),
            None,
            PaneContent::Lines(&echo),
        ),
    ];
    draw(&panes, theme)
}

/// A tiled pane of `content`, filled in `background`.
fn tiled_pane<'a>(
    id: u32,
    region: CellRect,
    background: Option<Face>,
    content: PaneContent<'a>,
) -> Pane<'a> {
    Pane {
        id: PaneId(id),
        region,
        layer: Layer::TILED,
        background,
        panel: None,
        bands: Bands::NONE,
        content,
    }
}
