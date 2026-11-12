//! libghostty-vt (Ghostty's terminal core, Zig, C API) behind the comparison's `Engine`
//! trait. All unsafe code is in this crate; the public API is safe.
//!
//! Choices worth knowing when reading the comparison's numbers:
//! - Scrollback uses `GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_LINES`. The library prunes whole
//!   pages, so it keeps somewhat more lines than asked for (dozens to about a hundred).
//! - Effects arrive through callbacks during `ghostty_terminal_vt_write`; they only push
//!   onto a `Vec` and never call back into the terminal's write functions.
//! - Snapshots read one render state (`ghostty_render_state_*`), which consumes the
//!   terminal's own damage. `screen_text` updates the same render state without cleaning
//!   it, so the next snapshot still sees every row changed since the last snapshot.
//! - Packed cells are decoded with the bit positions the linked library publishes in its
//!   ABI manifest (`ghostty_type_json`), as types.h recommends for speed, falling back to
//!   `ghostty_cell_get_multi`; runs of identical raw cells reuse the last decoded cell,
//!   and a style is fetched once per style id per row.
//! - Kitty graphics: left at the library default (see `version_info`).

mod ffi;
mod manifest;

use std::ffi::{c_int, c_void};
use std::ptr;

use engine::{Cell, Color, Effect, Engine, Memory, Modes, Snapshot, flags};
use manifest::CellLayout;

const CELL_WIDTH_PX: u32 = 8;
const CELL_HEIGHT_PX: u32 = 16;
const UNKNOWN_MAX_BYTES: usize = 4096;

/// State the callbacks write to. Allocated with `Box::into_raw` so the pointer the
/// library holds as userdata stays valid and unaliased by any Rust reference while
/// `ghostty_terminal_vt_write` runs.
struct CallbackState {
    effects: Vec<Effect>,
    cols: u16,
    rows: u16,
}

pub struct GhosttyEngine {
    terminal: ffi::Terminal,
    render: ffi::RenderState,
    rows_iter: ffi::RowIterator,
    row_cells: ffi::RowCells,
    state: *mut CallbackState,
    /// Reused output buffer for grapheme clusters.
    grapheme_buf: Vec<u8>,
    /// Graphemes of the rows rebuilt by the current snapshot.
    new_graphemes: Vec<(u32, String)>,
    /// Reused copy of the current row's raw cells.
    raw_row: Vec<ffi::Cell>,
    /// Packed-cell bit positions from the library's manifest; None falls back to FFI.
    layout: Option<CellLayout>,
}

fn color_of(c: &ffi::StyleColor) -> Color {
    let b = c.value.to_ne_bytes();
    match c.tag {
        ffi::STYLE_COLOR_PALETTE => Color::Indexed(b[0]),
        ffi::STYLE_COLOR_RGB => Color::Rgb(b[0], b[1], b[2]),
        _ => Color::Default,
    }
}

/// What a cell's style contributes to an `engine::Cell`.
#[derive(Clone, Copy)]
struct StyleBits {
    fg: Color,
    bg: Color,
    flags: u16,
}

impl StyleBits {
    const DEFAULT: StyleBits = StyleBits {
        fg: Color::Default,
        bg: Color::Default,
        flags: 0,
    };

    fn from_style(s: &ffi::Style) -> StyleBits {
        let mut f = 0;
        if s.bold {
            f |= flags::BOLD;
        }
        if s.faint {
            f |= flags::DIM;
        }
        if s.italic {
            f |= flags::ITALIC;
        }
        if s.underline != 0 {
            f |= flags::UNDERLINE;
        }
        if s.inverse {
            f |= flags::INVERSE;
        }
        if s.invisible {
            f |= flags::HIDDEN;
        }
        if s.strikethrough {
            f |= flags::STRIKE;
        }
        StyleBits {
            fg: color_of(&s.fg_color),
            bg: color_of(&s.bg_color),
            flags: f,
        }
    }
}

/// The fields of a raw cell this crate reads, in one `ghostty_cell_get_multi` call.
#[derive(Default)]
struct RawCell {
    codepoint: u32,
    tag: c_int,
    wide: c_int,
    has_styling: bool,
    style_id: u16,
    has_link: bool,
}

const RAW_KEYS: [c_int; 6] = [
    ffi::CELL_DATA_CODEPOINT,
    ffi::CELL_DATA_CONTENT_TAG,
    ffi::CELL_DATA_WIDE,
    ffi::CELL_DATA_HAS_STYLING,
    ffi::CELL_DATA_STYLE_ID,
    ffi::CELL_DATA_HAS_HYPERLINK,
];

fn decode_raw(layout: Option<&CellLayout>, cell: ffi::Cell) -> RawCell {
    match layout {
        Some(l) => {
            let tag = l.tag.get(cell) as c_int;
            // The content bits are a union; only the two text tags hold a codepoint.
            let text =
                tag == ffi::CELL_CONTENT_CODEPOINT || tag == ffi::CELL_CONTENT_CODEPOINT_GRAPHEME;
            RawCell {
                codepoint: if text {
                    l.codepoint.get(cell) as u32
                } else {
                    0
                },
                tag,
                wide: l.wide.get(cell) as c_int,
                has_styling: l.style_id.get(cell) != 0,
                style_id: l.style_id.get(cell) as u16,
                has_link: l.hyperlink.get(cell) != 0,
            }
        }
        None => decode_raw_ffi(cell),
    }
}

/// `decode_raw` through the library's accessor; the fallback, and the reference in tests.
fn decode_raw_ffi(cell: ffi::Cell) -> RawCell {
    let mut r = RawCell::default();
    let mut values: [*mut c_void; 6] = [
        (&raw mut r.codepoint).cast(),
        (&raw mut r.tag).cast(),
        (&raw mut r.wide).cast(),
        (&raw mut r.has_styling).cast(),
        (&raw mut r.style_id).cast(),
        (&raw mut r.has_link).cast(),
    ];
    // SAFETY: `keys` and `values` both have six entries, and each value points at a live
    // field of the type the header documents for its key (uint32_t, the content-tag enum,
    // the wide enum, bool, uint16_t, bool). The cell is passed by value.
    let rc = unsafe {
        ffi::ghostty_cell_get_multi(
            cell,
            RAW_KEYS.len(),
            RAW_KEYS.as_ptr(),
            values.as_mut_ptr(),
            ptr::null_mut(),
        )
    };
    debug_assert_eq!(rc, ffi::SUCCESS);
    r
}

fn bytes<'a>(s: ffi::GString) -> &'a [u8] {
    if s.ptr.is_null() || s.len == 0 {
        &[]
    } else {
        // SAFETY: the library hands out `GhosttyString`s whose `ptr` points at `len`
        // readable bytes for the duration of the callback or until the next mutating call;
        // every caller copies the bytes out before either ends.
        unsafe { std::slice::from_raw_parts(s.ptr, s.len) }
    }
}

fn lossy(s: ffi::GString) -> String {
    String::from_utf8_lossy(bytes(s)).into_owned()
}

/// Recovers the callback state from the userdata pointer.
///
/// # Safety
/// `ud` must be the pointer `GhosttyEngine::new` registered, and no other reference to the
/// state may be live. Both hold inside callbacks: the engine only touches the state outside
/// `ghostty_terminal_vt_write` and `ghostty_terminal_resize`, which are the only calls
/// that run callbacks.
unsafe fn state<'a>(ud: *mut c_void) -> &'a mut CallbackState {
    // SAFETY: see the function's contract.
    unsafe { &mut *ud.cast::<CallbackState>() }
}

fn terminal_string(t: ffi::Terminal, data: c_int) -> String {
    let mut s = ffi::GString::EMPTY;
    // SAFETY: `t` is the live terminal passed to the callback, and `data` is TITLE or PWD,
    // whose output type is `GhosttyString`. The string is copied before returning.
    let rc = unsafe { ffi::ghostty_terminal_get(t, data, (&raw mut s).cast()) };
    if rc == ffi::SUCCESS {
        lossy(s)
    } else {
        String::new()
    }
}

extern "C" fn on_write_pty(_t: ffi::Terminal, ud: *mut c_void, data: *const u8, len: usize) {
    let reply = bytes(ffi::GString { ptr: data, len }).to_vec();
    // SAFETY: called by the library during vt_write with our userdata.
    unsafe { state(ud) }.effects.push(Effect::Reply(reply));
}

extern "C" fn on_bell(_t: ffi::Terminal, ud: *mut c_void) {
    // SAFETY: called by the library during vt_write with our userdata.
    unsafe { state(ud) }.effects.push(Effect::Bell);
}

extern "C" fn on_title_changed(t: ffi::Terminal, ud: *mut c_void) {
    // The library stores the title before calling this (stream_terminal.zig windowTitle),
    // so reading it here gives the new value.
    let title = terminal_string(t, ffi::DATA_TITLE);
    // SAFETY: called by the library during vt_write with our userdata.
    unsafe { state(ud) }.effects.push(Effect::Title(title));
}

extern "C" fn on_pwd_changed(t: ffi::Terminal, ud: *mut c_void) {
    let pwd = terminal_string(t, ffi::DATA_PWD);
    // SAFETY: called by the library during vt_write with our userdata.
    unsafe { state(ud) }
        .effects
        .push(Effect::WorkingDirectory(pwd));
}

extern "C" fn on_size(_t: ffi::Terminal, ud: *mut c_void, out: *mut ffi::SizeReportSize) -> bool {
    // SAFETY: called by the library during vt_write with our userdata.
    let st = unsafe { state(ud) };
    if out.is_null() {
        return false;
    }
    // SAFETY: the library passes a valid out pointer for the duration of the call.
    unsafe {
        out.write(ffi::SizeReportSize {
            rows: st.rows,
            columns: st.cols,
            cell_width: CELL_WIDTH_PX,
            cell_height: CELL_HEIGHT_PX,
        });
    }
    true
}

/// Answers DA1 like Alacritty (`CSI ? 6 c`); DA2 `CSI > 0 ; 0 ; 1 c`.
extern "C" fn on_device_attributes(
    _t: ffi::Terminal,
    _ud: *mut c_void,
    out: *mut ffi::DeviceAttributes,
) -> bool {
    if out.is_null() {
        return false;
    }
    // SAFETY: the library passes a valid, writable out pointer for the duration of the call.
    unsafe {
        out.write(ffi::DeviceAttributes {
            primary: ffi::DeviceAttributesPrimary {
                conformance_level: 6,
                features: [0; 64],
                num_features: 0,
            },
            secondary: ffi::DeviceAttributesSecondary {
                device_type: 0,
                firmware_version: 0,
                rom_cartridge: 1,
            },
            tertiary: ffi::DeviceAttributesTertiary { unit_id: 0 },
        });
    }
    true
}

extern "C" fn on_clipboard_write(
    _t: ffi::Terminal,
    ud: *mut c_void,
    w: *const ffi::ClipboardWrite,
) {
    if w.is_null() {
        return;
    }
    // SAFETY: the request is valid for the duration of the callback.
    let w = unsafe { &*w };
    let contents: &[ffi::ClipboardContent] = if w.contents.is_null() || w.contents_len == 0 {
        &[]
    } else {
        // SAFETY: `contents` points at `contents_len` entries for the callback's duration.
        unsafe { std::slice::from_raw_parts(w.contents, w.contents_len) }
    };
    // Prefer a text representation; OSC 52 always gives exactly one, "text/plain".
    let data = contents
        .iter()
        .find(|c| bytes(c.mime).starts_with(b"text/"))
        .or(contents.first())
        .map(|c| bytes(c.data).to_vec())
        .unwrap_or_default();
    // SAFETY: called by the library during vt_write with our userdata.
    unsafe { state(ud) }
        .effects
        .push(Effect::ClipboardWrite(data));
    if let Some(reply) = w.reply {
        let r = ffi::ClipboardWriteReply {
            size: size_of::<ffi::ClipboardWriteReply>(),
            result: ffi::CLIPBOARD_WRITE_RESULT_DENIED,
            remember: false,
        };
        // SAFETY: the header requires replying through `reply` with this request while the
        // callback runs; the reply struct is borrowed only for the call.
        unsafe { reply(w, &r) };
    }
}

extern "C" fn on_clipboard_read(_t: ffi::Terminal, ud: *mut c_void, r: *const ffi::ClipboardRead) {
    // SAFETY: called by the library during vt_write with our userdata.
    unsafe { state(ud) }.effects.push(Effect::ClipboardRead);
    if r.is_null() {
        return;
    }
    // SAFETY: the request is valid for the duration of the callback.
    let req = unsafe { &*r };
    if let Some(reply) = req.reply {
        let deny = ffi::ClipboardReadReply {
            size: size_of::<ffi::ClipboardReadReply>(),
            result: ffi::CLIPBOARD_READ_RESULT_DENIED,
            contents: ptr::null(),
            contents_len: 0,
            available: ptr::null(),
            available_len: 0,
            remember: false,
        };
        // SAFETY: as for the write reply: answered during the callback, reply borrowed only
        // for the call, and it carries no data.
        unsafe { reply(r, &deny) };
    }
}

extern "C" fn on_notification(
    _t: ffi::Terminal,
    ud: *mut c_void,
    n: *const ffi::DesktopNotification,
) {
    if n.is_null() {
        return;
    }
    // SAFETY: the notification is valid for the duration of the callback.
    let n = unsafe { &*n };
    let effect = Effect::Notification {
        title: lossy(n.title),
        body: lossy(n.body),
    };
    // SAFETY: called by the library during vt_write with our userdata.
    unsafe { state(ud) }.effects.push(effect);
}

extern "C" fn on_progress(_t: ffi::Terminal, ud: *mut c_void, p: *const ffi::ProgressReport) {
    if p.is_null() {
        return;
    }
    // SAFETY: the report is valid for the duration of the callback.
    let p = unsafe { &*p };
    let effect = Effect::Progress {
        state: u8::try_from(p.state).unwrap_or(u8::MAX),
        percent: u8::try_from(p.progress).ok(),
    };
    // SAFETY: called by the library during vt_write with our userdata.
    unsafe { state(ud) }.effects.push(effect);
}

/// Maps the decoded OSC 133 event back to its mark letter. The library does not pass the
/// raw parameters, so `params` is rebuilt from what it does report: `k=r|c|s` for a
/// non-primary prompt, `cmdline=<decoded>` for C, the exit code (and `;err=<text>`) for D.
extern "C" fn on_semantic_prompt(
    _t: ffi::Terminal,
    ud: *mut c_void,
    e: *const ffi::SemanticPrompt,
) {
    if e.is_null() {
        return;
    }
    // SAFETY: the event is valid for the duration of the callback.
    let e = unsafe { &*e };
    let (kind, params) = match e.kind {
        ffi::SEMANTIC_PROMPT_PROMPT_START => {
            let params = match e.prompt_kind {
                ffi::SEMANTIC_PROMPT_PROMPT_RIGHT => "k=r",
                ffi::SEMANTIC_PROMPT_PROMPT_CONTINUATION => "k=c",
                ffi::SEMANTIC_PROMPT_PROMPT_SECONDARY => "k=s",
                _ => "",
            };
            ('A', params.to_string())
        }
        ffi::SEMANTIC_PROMPT_INPUT_START => ('B', String::new()),
        ffi::SEMANTIC_PROMPT_OUTPUT_START => {
            let cmd = lossy(e.command);
            (
                'C',
                if cmd.is_empty() {
                    cmd
                } else {
                    format!("cmdline={cmd}")
                },
            )
        }
        ffi::SEMANTIC_PROMPT_COMMAND_END => {
            let mut p = if e.has_exit_code {
                e.exit_code.to_string()
            } else {
                String::new()
            };
            let err = lossy(e.error);
            if !err.is_empty() {
                p.push_str(";err=");
                p.push_str(&err);
            }
            ('D', p)
        }
        _ => return,
    };
    // SAFETY: called by the library during vt_write with our userdata.
    unsafe { state(ud) }
        .effects
        .push(Effect::PromptMark { kind, params });
}

extern "C" fn on_unknown_sequence(
    _t: ffi::Terminal,
    ud: *mut c_void,
    s: *const ffi::UnknownSequence,
) {
    if s.is_null() {
        return;
    }
    // SAFETY: the sequence is valid for the duration of the callback.
    let s = unsafe { &*s };
    let effect = match s.tag {
        ffi::UNKNOWN_SEQUENCE_OSC => {
            // SAFETY: the tag says the OSC arm is the active one.
            let osc = unsafe { s.value.osc };
            Effect::UnknownOsc(bytes(osc.content).to_vec())
        }
        ffi::UNKNOWN_SEQUENCE_APC => {
            // SAFETY: the tag says the APC arm is the active one.
            let apc = unsafe { s.value.apc };
            Effect::Apc(bytes(apc.content).to_vec())
        }
        _ => return,
    };
    // SAFETY: called by the library during vt_write with our userdata.
    unsafe { state(ud) }.effects.push(effect);
}

impl GhosttyEngine {
    fn set(&self, option: c_int, value: *const c_void) {
        // SAFETY: `self.terminal` is live; each call site passes the value type the header
        // documents for `option` (a function pointer, the userdata pointer, or a pointer
        // to a value the library copies before returning).
        let rc = unsafe { ffi::ghostty_terminal_set(self.terminal, option, value) };
        assert_eq!(
            rc,
            ffi::SUCCESS,
            "ghostty_terminal_set({option}) failed: {rc}"
        );
    }

    fn get<T>(&self, data: c_int, out: &mut T) -> bool {
        // SAFETY: `self.terminal` is live and every call site passes `T` matching the
        // output type the header documents for `data`.
        unsafe {
            ffi::ghostty_terminal_get(self.terminal, data, (out as *mut T).cast()) == ffi::SUCCESS
        }
    }

    fn mode(&self, number: u16) -> bool {
        let mut m = ffi::ModeConfig {
            mode: ffi::dec_mode(number),
            value: false,
        };
        self.get(ffi::DATA_MODE, &mut m) && m.value
    }

    fn update_render(&mut self) {
        // SAFETY: both handles are live and owned by `self`; no row iterator or cells data
        // from an earlier update is used after this call without being re-populated.
        let rc = unsafe { ffi::ghostty_render_state_update(self.render, self.terminal) };
        assert_eq!(rc, ffi::SUCCESS, "ghostty_render_state_update failed: {rc}");
    }

    fn render_get<T>(&self, data: c_int, out: &mut T) -> bool {
        // SAFETY: `self.render` is live and every call site passes `T` matching the output
        // type documented for `data` (for ROW_ITERATOR, the iterator handle it populates).
        unsafe {
            ffi::ghostty_render_state_get(self.render, data, (out as *mut T).cast()) == ffi::SUCCESS
        }
    }

    fn row_get<T>(&self, data: c_int, out: &mut T) -> bool {
        // SAFETY: the iterator is live and positioned on a row (callers only call this after
        // `ghostty_render_state_row_iterator_next` returned true); `T` matches `data`.
        unsafe {
            ffi::ghostty_render_state_row_get(self.rows_iter, data, (out as *mut T).cast())
                == ffi::SUCCESS
        }
    }

    /// Points `self.rows_iter` at the rows of the last update.
    fn start_rows(&mut self) {
        let mut it = self.rows_iter;
        let ok = self.render_get(ffi::RS_DATA_ROW_ITERATOR, &mut it);
        debug_assert!(ok);
    }

    fn next_row(&mut self) -> bool {
        // SAFETY: the iterator is live and was populated by `start_rows` after the last
        // render-state update.
        unsafe { ffi::ghostty_render_state_row_iterator_next(self.rows_iter) }
    }

    /// The current row's raw cells. Valid until the next render-state update.
    fn row_raw_cells(&self) -> &[ffi::Cell] {
        let mut view = ffi::CellsView {
            ptr: ptr::null(),
            len: 0,
        };
        if !self.row_get(ffi::ROW_DATA_CELLS_RAW, &mut view) || view.ptr.is_null() {
            return &[];
        }
        // SAFETY: the library documents the view as `len` contiguous cells owned by the
        // render state, valid until it is next updated; `&self` keeps `update_render`
        // (which needs `&mut self`) from running while the slice lives.
        unsafe { std::slice::from_raw_parts(view.ptr, view.len) }
    }

    /// Populates `self.row_cells` for the current row so cells can be selected.
    fn load_row_cells(&self) {
        let mut cells = self.row_cells;
        let ok = self.row_get(ffi::ROW_DATA_CELLS, &mut cells);
        debug_assert!(ok);
    }

    fn cell_style(&self, x: u16) -> StyleBits {
        let mut style = ffi::Style::sized();
        // SAFETY: `row_cells` is live and was populated for the current row; `x` is below
        // the row's width; STYLE writes a `GhosttyStyle` whose `size` we set.
        let ok = unsafe {
            ffi::ghostty_render_state_row_cells_select(self.row_cells, x) == ffi::SUCCESS
                && ffi::ghostty_render_state_row_cells_get(
                    self.row_cells,
                    ffi::CELLS_DATA_STYLE,
                    (&raw mut style).cast(),
                ) == ffi::SUCCESS
        };
        if ok {
            StyleBits::from_style(&style)
        } else {
            StyleBits::DEFAULT
        }
    }

    /// The full grapheme cluster of cell `x` of the current row (cells loaded).
    fn cell_grapheme(&mut self, x: u16) -> String {
        loop {
            let mut buf = ffi::GBuffer {
                ptr: self.grapheme_buf.as_mut_ptr(),
                cap: self.grapheme_buf.capacity(),
                len: 0,
            };
            // SAFETY: `row_cells` is live and populated for the current row; `buf` describes
            // our Vec's spare capacity, which the library writes at most `cap` bytes into.
            let rc = unsafe {
                if ffi::ghostty_render_state_row_cells_select(self.row_cells, x) != ffi::SUCCESS {
                    return String::new();
                }
                ffi::ghostty_render_state_row_cells_get(
                    self.row_cells,
                    ffi::CELLS_DATA_GRAPHEMES_UTF8,
                    (&raw mut buf).cast(),
                )
            };
            match rc {
                ffi::SUCCESS => {
                    let len = buf.len.min(self.grapheme_buf.capacity());
                    // SAFETY: the library wrote `len` initialized bytes at the start of the
                    // buffer, within its capacity.
                    unsafe { self.grapheme_buf.set_len(len) };
                    let s = String::from_utf8_lossy(&self.grapheme_buf).into_owned();
                    self.grapheme_buf.clear();
                    return s;
                }
                ffi::OUT_OF_SPACE if buf.len > self.grapheme_buf.capacity() => {
                    self.grapheme_buf.reserve(buf.len);
                }
                _ => return String::new(),
            }
        }
    }

    fn cell_bg(layout: Option<&CellLayout>, raw: ffi::Cell, tag: c_int) -> Color {
        if let Some(l) = layout {
            return if tag == ffi::CELL_CONTENT_BG_COLOR_PALETTE {
                Color::Indexed(l.palette.get(raw) as u8)
            } else {
                Color::Rgb(l.r.get(raw) as u8, l.g.get(raw) as u8, l.b.get(raw) as u8)
            };
        }
        if tag == ffi::CELL_CONTENT_BG_COLOR_PALETTE {
            let mut idx = 0u8;
            // SAFETY: COLOR_PALETTE writes a `GhosttyColorPaletteIndex` (uint8_t).
            unsafe {
                ffi::ghostty_cell_get(raw, ffi::CELL_DATA_COLOR_PALETTE, (&raw mut idx).cast())
            };
            Color::Indexed(idx)
        } else {
            let mut rgb = ffi::ColorRgb::default();
            // SAFETY: COLOR_RGB writes a `GhosttyColorRgb`.
            unsafe { ffi::ghostty_cell_get(raw, ffi::CELL_DATA_COLOR_RGB, (&raw mut rgb).cast()) };
            Color::Rgb(rgb.r, rgb.g, rgb.b)
        }
    }

    /// Fills `dst` (one row) from the current row. Grapheme cells are pushed to
    /// `self.new_graphemes` with `base` added to their column.
    fn build_row(&mut self, dst: &mut [Cell], base: u32) {
        self.load_row_cells();
        // Copy the raw cells out of the borrow so the helpers below can take `&mut self`.
        // Rows are short; this is one memcpy into a reused buffer.
        let mut raw_row = std::mem::take(&mut self.raw_row);
        raw_row.clear();
        raw_row.extend_from_slice(self.row_raw_cells());

        let mut last_raw: Option<ffi::Cell> = None;
        let mut last_cell = Cell::default();
        let mut style_cache: Option<(u16, StyleBits)> = None;

        for (x, (out, &raw)) in dst.iter_mut().zip(raw_row.iter()).enumerate() {
            if last_raw == Some(raw) {
                *out = last_cell;
                continue;
            }
            let r = decode_raw(self.layout.as_ref(), raw);
            let x16 = x as u16;
            let style = if r.has_styling {
                match style_cache {
                    Some((id, bits)) if id == r.style_id => bits,
                    _ => {
                        let bits = self.cell_style(x16);
                        style_cache = Some((r.style_id, bits));
                        bits
                    }
                }
            } else {
                StyleBits::DEFAULT
            };
            let mut cell = Cell {
                ch: ' ',
                fg: style.fg,
                bg: style.bg,
                flags: style.flags,
            };
            match r.tag {
                ffi::CELL_CONTENT_CODEPOINT | ffi::CELL_CONTENT_CODEPOINT_GRAPHEME => {
                    if r.codepoint != 0 {
                        cell.ch =
                            char::from_u32(r.codepoint).unwrap_or(char::REPLACEMENT_CHARACTER);
                    }
                }
                tag => cell.bg = Self::cell_bg(self.layout.as_ref(), raw, tag),
            }
            match r.wide {
                ffi::CELL_WIDE_WIDE => cell.flags |= flags::WIDE,
                ffi::CELL_WIDE_SPACER_TAIL => cell.flags |= flags::WIDE_SPACER,
                _ => {}
            }
            if r.has_link {
                cell.flags |= flags::LINK;
            }
            if r.tag == ffi::CELL_CONTENT_CODEPOINT_GRAPHEME {
                cell.flags |= flags::GRAPHEME;
                let g = self.cell_grapheme(x16);
                self.new_graphemes.push((base + x as u32, g));
                // Two grapheme cells with the same raw value can hold different clusters.
                last_raw = None;
            } else {
                last_raw = Some(raw);
                last_cell = cell;
            }
            *out = cell;
        }
        self.raw_row = raw_row;
    }

    /// The current row as text: trailing spaces trimmed, wide spacers skipped.
    fn row_text(&mut self) -> String {
        self.load_row_cells();
        let mut raw_row = std::mem::take(&mut self.raw_row);
        raw_row.clear();
        raw_row.extend_from_slice(self.row_raw_cells());
        let mut line = String::with_capacity(raw_row.len());
        for (x, &raw) in raw_row.iter().enumerate() {
            let r = decode_raw(self.layout.as_ref(), raw);
            if r.wide == ffi::CELL_WIDE_SPACER_TAIL {
                continue;
            }
            match r.tag {
                ffi::CELL_CONTENT_CODEPOINT_GRAPHEME => {
                    let g = self.cell_grapheme(x as u16);
                    line.push_str(&g);
                }
                ffi::CELL_CONTENT_CODEPOINT if r.codepoint != 0 => {
                    line.push(char::from_u32(r.codepoint).unwrap_or(char::REPLACEMENT_CHARACTER));
                }
                _ => line.push(' '),
            }
        }
        self.raw_row = raw_row;
        let trimmed = line.trim_end_matches(' ').len();
        line.truncate(trimmed);
        line
    }

    fn st(&mut self) -> &mut CallbackState {
        // SAFETY: `state` came from `Box::into_raw` in `new` and is freed only in Drop. No
        // callback runs while this borrow lives: callbacks only run inside
        // `ghostty_terminal_vt_write` / `ghostty_terminal_resize`, which never overlap
        // with a call to this method.
        unsafe { &mut *self.state }
    }
}

/// libghostty-vt's build: version, optimize mode, SIMD and Kitty graphics support, and
/// the Kitty image storage limit a new terminal starts with.
pub fn version_info() -> String {
    let mut version = ffi::GString::EMPTY;
    let mut optimize: c_int = -1;
    let mut simd = false;
    let mut kitty = false;
    // SAFETY: each key's output type is the one build_info.h documents (GhosttyString,
    // GhosttyOptimizeMode, bool, bool). The version string is static library data.
    unsafe {
        ffi::ghostty_build_info(ffi::BUILD_INFO_VERSION_STRING, (&raw mut version).cast());
        ffi::ghostty_build_info(ffi::BUILD_INFO_OPTIMIZE, (&raw mut optimize).cast());
        ffi::ghostty_build_info(ffi::BUILD_INFO_SIMD, (&raw mut simd).cast());
        ffi::ghostty_build_info(ffi::BUILD_INFO_KITTY_GRAPHICS, (&raw mut kitty).cast());
    }
    let optimize = match optimize {
        0 => "Debug",
        1 => "ReleaseSafe",
        2 => "ReleaseSmall",
        3 => "ReleaseFast",
        _ => "unknown",
    };
    let engine = GhosttyEngine::new(80, 24, 0);
    let mut limit = 0u64;
    let limit = if engine.get(ffi::DATA_KITTY_IMAGE_STORAGE_LIMIT, &mut limit) {
        format!("{limit} bytes")
    } else {
        "n/a".into()
    };
    let (mut file, mut shm) = (false, false);
    let mut temp = ffi::GString::EMPTY;
    engine.get(ffi::DATA_KITTY_IMAGE_MEDIUM_FILE, &mut file);
    engine.get(ffi::DATA_KITTY_IMAGE_MEDIUM_SHARED_MEM, &mut shm);
    engine.get(ffi::DATA_KITTY_IMAGE_MEDIUM_TEMP_FILE, &mut temp);
    format!(
        "libghostty-vt {} (source {}, {optimize}, simd {simd}, kitty graphics {kitty}, \
         image limit {limit}, file medium {file}, temp-file medium {:?}, shm medium {shm})",
        lossy(version),
        env!("GHOSTTY_VT_SOURCE_VERSION"),
        lossy(temp),
    )
}

impl Engine for GhosttyEngine {
    fn new(cols: u16, rows: u16, scrollback: usize) -> Self {
        let mut terminal: ffi::Terminal = ptr::null_mut();
        let mut render: ffi::RenderState = ptr::null_mut();
        let mut rows_iter: ffi::RowIterator = ptr::null_mut();
        let mut row_cells: ffi::RowCells = ptr::null_mut();
        // SAFETY: NULL selects the default allocator; each out pointer is a live local of
        // the handle type the function writes.
        unsafe {
            assert_eq!(
                ffi::ghostty_terminal_new(ptr::null(), &mut terminal, cols, rows),
                ffi::SUCCESS
            );
            assert_eq!(
                ffi::ghostty_render_state_new(ptr::null(), &mut render),
                ffi::SUCCESS
            );
            assert_eq!(
                ffi::ghostty_render_state_row_iterator_new(ptr::null(), &mut rows_iter),
                ffi::SUCCESS
            );
            assert_eq!(
                ffi::ghostty_render_state_row_cells_new(ptr::null(), &mut row_cells),
                ffi::SUCCESS
            );
        }
        let state = Box::into_raw(Box::new(CallbackState {
            effects: Vec::new(),
            cols,
            rows,
        }));
        let engine = GhosttyEngine {
            terminal,
            render,
            rows_iter,
            row_cells,
            state,
            grapheme_buf: Vec::with_capacity(64),
            new_graphemes: Vec::new(),
            raw_row: Vec::new(),
            layout: CellLayout::parse(),
        };

        // ghostty_terminal_set takes pointer-typed values (callbacks, userdata) directly
        // and everything else through a pointer it reads during the call.
        engine.set(ffi::OPT_USERDATA, state.cast_const().cast());
        let lines: usize = scrollback;
        engine.set(ffi::OPT_SCROLLBACK_MAX_LINES, (&raw const lines).cast());
        let no = false;
        engine.set(ffi::OPT_TITLE_REPORT, (&raw const no).cast());
        engine.set(
            ffi::OPT_WRITE_PTY,
            on_write_pty as ffi::WritePtyFn as *const c_void,
        );
        engine.set(ffi::OPT_BELL, on_bell as ffi::BellFn as *const c_void);
        engine.set(
            ffi::OPT_TITLE_CHANGED,
            on_title_changed as ffi::TitleChangedFn as *const c_void,
        );
        engine.set(
            ffi::OPT_PWD_CHANGED,
            on_pwd_changed as ffi::PwdChangedFn as *const c_void,
        );
        engine.set(ffi::OPT_SIZE, on_size as ffi::SizeFn as *const c_void);
        engine.set(
            ffi::OPT_DEVICE_ATTRIBUTES,
            on_device_attributes as ffi::DeviceAttributesFn as *const c_void,
        );
        engine.set(
            ffi::OPT_CLIPBOARD_WRITE,
            on_clipboard_write as ffi::ClipboardWriteFn as *const c_void,
        );
        engine.set(
            ffi::OPT_CLIPBOARD_READ,
            on_clipboard_read as ffi::ClipboardReadFn as *const c_void,
        );
        engine.set(
            ffi::OPT_DESKTOP_NOTIFICATION,
            on_notification as ffi::DesktopNotificationFn as *const c_void,
        );
        engine.set(
            ffi::OPT_PROGRESS_REPORT,
            on_progress as ffi::ProgressReportFn as *const c_void,
        );
        engine.set(
            ffi::OPT_SEMANTIC_PROMPT,
            on_semantic_prompt as ffi::SemanticPromptFn as *const c_void,
        );
        engine.set(
            ffi::OPT_UNKNOWN_SEQUENCE,
            on_unknown_sequence as ffi::UnknownSequenceFn as *const c_void,
        );
        let max = UNKNOWN_MAX_BYTES;
        engine.set(ffi::OPT_UNKNOWN_MAX_BYTES, (&raw const max).cast());

        // SAFETY: the terminal is live; this sets the pixel size used by size reports.
        let rc = unsafe {
            ffi::ghostty_terminal_resize(terminal, cols, rows, CELL_WIDTH_PX, CELL_HEIGHT_PX)
        };
        assert_eq!(rc, ffi::SUCCESS);
        engine
    }

    fn name(&self) -> &'static str {
        "libghostty-vt"
    }

    fn feed(&mut self, bytes: &[u8]) {
        // SAFETY: the terminal is live and `bytes` is readable for its length. Callbacks
        // run during this call and use `self.state` through the userdata pointer; nothing
        // else borrows the state meanwhile.
        unsafe { ffi::ghostty_terminal_vt_write(self.terminal, bytes.as_ptr(), bytes.len()) };
    }

    fn resize(&mut self, cols: u16, rows: u16) {
        let st = self.st();
        st.cols = cols;
        st.rows = rows;
        // SAFETY: the terminal is live. Resize can run callbacks (render hold, in-band size
        // report through write_pty); no borrow of the state is live here.
        let rc = unsafe {
            ffi::ghostty_terminal_resize(self.terminal, cols, rows, CELL_WIDTH_PX, CELL_HEIGHT_PX)
        };
        debug_assert_eq!(rc, ffi::SUCCESS);
    }

    fn snapshot(&mut self, out: &mut Snapshot) {
        self.update_render();
        let (mut cols, mut rows) = (0u16, 0u16);
        self.render_get(ffi::RS_DATA_COLS, &mut cols);
        self.render_get(ffi::RS_DATA_ROWS, &mut rows);
        let mut dirty: c_int = ffi::RENDER_DIRTY_FULL;
        self.render_get(ffi::RS_DATA_DIRTY, &mut dirty);

        let mut full = dirty == ffi::RENDER_DIRTY_FULL;
        if out.cols != cols || out.rows != rows {
            out.resize(cols, rows);
            full = true;
        }

        if full || dirty == ffi::RENDER_DIRTY_PARTIAL {
            self.new_graphemes.clear();
            self.start_rows();
            let width = usize::from(cols);
            let mut y = 0usize;
            while y < usize::from(rows) && self.next_row() {
                let mut rebuild = full;
                if !rebuild {
                    self.row_get(ffi::ROW_DATA_DIRTY, &mut rebuild);
                }
                out.dirty[y] = rebuild;
                if rebuild {
                    let start = y * width;
                    self.build_row(&mut out.cells[start..start + width], start as u32);
                }
                y += 1;
            }
            let any = out.dirty.iter().any(|&d| d);
            if any {
                if full {
                    out.graphemes.clear();
                } else {
                    let dirty_rows = &out.dirty;
                    out.graphemes
                        .retain(|(i, _)| !dirty_rows[*i as usize / width]);
                }
                out.graphemes.append(&mut self.new_graphemes);
            }
        } else {
            out.dirty.iter_mut().for_each(|d| *d = false);
        }

        let mut cursor = ffi::RenderCursor::sized();
        if self.render_get(ffi::RS_DATA_CURSOR, &mut cursor) && cursor.viewport_has_value {
            out.cursor_row = cursor.viewport_y;
            out.cursor_col = cursor.viewport_x;
            out.cursor_visible = cursor.visible;
        } else {
            out.cursor_visible = false;
        }

        // SAFETY: the render state is live; this only resets its own dirty flags.
        unsafe { ffi::ghostty_render_state_clean(self.render) };
    }

    fn screen_text(&mut self) -> Vec<String> {
        // Updating without cleaning keeps the render state's dirty flags for the next
        // snapshot (update only ever sets them).
        self.update_render();
        let mut rows = 0u16;
        self.render_get(ffi::RS_DATA_ROWS, &mut rows);
        self.start_rows();
        let mut lines = Vec::with_capacity(usize::from(rows));
        while lines.len() < usize::from(rows) && self.next_row() {
            lines.push(self.row_text());
        }
        lines.resize(usize::from(rows), String::new());
        lines
    }

    fn history_len(&mut self) -> usize {
        let mut n = 0usize;
        self.get(ffi::DATA_SCROLLBACK_ROWS, &mut n);
        n
    }

    fn modes(&mut self) -> Modes {
        let mut screen: c_int = ffi::TERMINAL_SCREEN_PRIMARY;
        self.get(ffi::DATA_ACTIVE_SCREEN, &mut screen);
        let mut cursor_visible = false;
        self.get(ffi::DATA_CURSOR_VISIBLE, &mut cursor_visible);
        let mut kitty = 0u8;
        self.get(ffi::DATA_KITTY_KEYBOARD_FLAGS, &mut kitty);
        let mouse_tracking = if self.mode(1003) {
            1003
        } else if self.mode(1002) {
            1002
        } else if self.mode(1000) {
            1000
        } else {
            0
        };
        Modes {
            alt_screen: screen == ffi::TERMINAL_SCREEN_ALTERNATE,
            app_cursor: self.mode(1),
            bracketed_paste: self.mode(2004),
            mouse_tracking,
            sgr_mouse: self.mode(1006),
            focus_events: self.mode(1004),
            cursor_visible,
            kitty_keyboard_flags: kitty,
        }
    }

    fn drain_effects(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.st().effects)
    }

    fn memory(&mut self) -> Memory {
        let mut usage = ffi::MemoryUsage {
            size: size_of::<ffi::MemoryUsage>(),
            ..Default::default()
        };
        let grid_bytes = self
            .get(ffi::DATA_MEMORY_USAGE, &mut usage)
            .then(|| usage.primary_resident_bytes + usage.alternate_resident_bytes);
        Memory { grid_bytes }
    }
}

impl Drop for GhosttyEngine {
    fn drop(&mut self) {
        // SAFETY: every handle was created in `new`, is freed exactly once here, and the
        // terminal (the only thing that calls back into `state`) goes before the state.
        unsafe {
            ffi::ghostty_render_state_row_cells_free(self.row_cells);
            ffi::ghostty_render_state_row_iterator_free(self.rows_iter);
            ffi::ghostty_render_state_free(self.render);
            ffi::ghostty_terminal_free(self.terminal);
            drop(Box::from_raw(self.state));
        }
    }
}

#[cfg(test)]
mod tests;
