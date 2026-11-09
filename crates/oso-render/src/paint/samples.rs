//! Sample frames of every face in each theme and candidate, drawn off screen with the OS's
//! font, for the author to pick colours from (ADR 0089). Run with `OSO_THEME_SAMPLES=<dir>
//! cargo test -p oso-render -- --ignored theme_samples`: it writes each frame's RGBA pixels
//! and a table of the contrast of each face's pairs.
//!
//! Must not depend on: `capture` or the control socket.

use std::fmt::Write as _;
use std::path::Path;

use oso_core::buffer::Buffer;
use oso_core::display_map::DisplayPoint;
use oso_core::frame::CellRect;
use oso_core::window::Scroll;
use oso_platform::text::{self, FontRequest};

use crate::pane::{
    Bands, CursorStyle, Face, Layer, Pane, PaneContent, PaneId, Panel, Span, TextView,
};
use crate::theme::candidates::candidates;
use crate::theme::{Theme, themes};

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
    theme
}
";

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

fn text_pane<'a>(
    buffer: &Buffer,
    id: oso_core::buffer::BufferId,
    fills: &'a [(Face, &'a [CellRect])],
) -> Pane<'a> {
    Pane {
        id: PaneId(0),
        region: cells(0, 0, TEXT_ROWS, COLUMNS),
        layer: Layer::TILED,
        background: Some(Face::Text),
        panel: None,
        bands: Bands::NONE,
        content: PaneContent::Text(TextView {
            display: buffer.display(),
            buffer: id,
            revision: 1,
            scroll: Scroll::default(),
            cursor: DisplayPoint::new(3, 10),
            cursor_style: CursorStyle::Box,
            preedit: None,
            fills,
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
    let id = oso_core::editor::Editor::default().window().buffer();
    let found = matches();
    let current = [found[2]];
    let region = [cells(2, 16, 1, 10)];
    let fills = [
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
        text_pane(&buffer, id, &fills),
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

/// The buffer with a question on the panel at the frame's top edge.
fn question_frame(theme: Theme) -> Option<(u32, u32, Vec<u8>)> {
    let buffer = Buffer::from_text("sample.rs", TEXT);
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
    let panes = [
        text_pane(&buffer, id, &[]),
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
    for theme in themes().into_iter().chain(candidates()) {
        let name = theme.name();
        let (Some(completion), Some(question)) = (completion_frame(theme), question_frame(theme))
        else {
            log::warn!("no text backend on this OS: no samples");
            return;
        };
        write(dir, &format!("{name}-completion"), completion);
        write(dir, &format!("{name}-question"), question);
        writeln!(
            table,
            "## {name}\n\n{}",
            crate::theme::tests::ratios(&theme)
        )
        .unwrap();
    }
    std::fs::write(dir.join("ratios.md"), table).unwrap();
}
