//! The safe layer over libghostty-vt's C API: one terminal, its render state and the
//! callbacks, owned together by `Vt` (ADR 0120). Each foreign call sits in an `unsafe` block
//! of its own with its `// SAFETY:` comment, and nothing outside this file and `ffi` holds
//! `unsafe`. The library makes no threads and its handles are not thread-safe, so a `Vt` is
//! neither `Send` nor `Sync`: it lives and dies on its terminal's thread.
//!
//! Must not depend on: the policy, the thread or the snapshot's types; `Ghostty` turns what
//! this reads into those.

#![expect(unsafe_code, reason = "the library's C API (ADR 0120)")]

mod callbacks;

use std::ffi::{c_int, c_void};
use std::marker::PhantomData;
use std::ptr::{self, NonNull};

use super::ffi;
use crate::engine::Effect;
use crate::limits;
#[cfg(test)]
use callbacks::bytes;
use callbacks::{
    on_bell, on_clipboard_write, on_notification, on_progress, on_prompt, on_pwd, on_size,
    on_title, on_unknown, on_write_pty, on_xtversion,
};

/// Effects raised but not taken yet are dropped past this many.
const EFFECTS_PENDING_MAX: usize = 1024;

/// What the callbacks write to while the library runs.
pub(super) struct Sink {
    pub effects: Vec<Effect>,
    /// Reply bytes among `effects`.
    reply_bytes: usize,
    /// What the text area size reports (`CSI 14 t`, `CSI 18 t`) answer.
    pub size: ffi::SizeReportSize,
}

impl Sink {
    pub(super) fn push(&mut self, effect: Effect) {
        if self.effects.len() < EFFECTS_PENDING_MAX {
            self.effects.push(effect);
        }
    }

    pub(super) fn reply(&mut self, bytes: &[u8]) {
        if self.reply_bytes + bytes.len() <= limits::REPLIES_PENDING_MAX {
            self.reply_bytes += bytes.len();
            self.push(Effect::Reply(bytes.to_vec()));
        }
    }

    pub fn take(&mut self, out: &mut Vec<Effect>) {
        out.append(&mut self.effects);
        self.reply_bytes = 0;
    }
}

/// The terminal state `Vt::state` reads in one go.
#[derive(Clone, Copy, Default)]
pub(super) struct State {
    pub history: usize,
    pub alternate_screen: bool,
    pub kitty_keyboard: u8,
}

/// The render state's view of the screen, as `Vt::frame` reads it.
pub(super) struct Frame {
    pub columns: u16,
    pub rows: u16,
    pub dirty: c_int,
    pub cursor: ffi::RenderCursor,
}

/// One libghostty-vt terminal with its render state, row iterator and row cells.
pub(super) struct Vt {
    terminal: ffi::Terminal,
    render: ffi::RenderState,
    rows: ffi::RowIterator,
    cells: ffi::RowCells,
    sink: NonNull<Sink>,
    /// Neither `Send` nor `Sync`.
    _thread: PhantomData<*mut ()>,
}

impl Vt {
    /// A terminal of `columns` × `rows`, with oso's settings and callbacks installed, or
    /// `None` when the library cannot allocate one.
    pub fn new(columns: u16, rows: u16, scrollback_lines: usize) -> Option<Vt> {
        let sink = Box::new(Sink {
            effects: Vec::new(),
            reply_bytes: 0,
            size: ffi::SizeReportSize {
                rows,
                columns,
                cell_width: 0,
                cell_height: 0,
            },
        });
        let mut vt = Vt {
            terminal: ptr::null_mut(),
            render: ptr::null_mut(),
            rows: ptr::null_mut(),
            cells: ptr::null_mut(),
            sink: NonNull::from(Box::leak(sink)),
            _thread: PhantomData,
        };
        // SAFETY: a NULL allocator is the library's default; `terminal` is written on success.
        let made =
            unsafe { ffi::ghostty_terminal_new(ptr::null(), &mut vt.terminal, columns, rows) };
        if made != ffi::SUCCESS {
            return None;
        }
        // SAFETY: as above, for the render state.
        let made = unsafe { ffi::ghostty_render_state_new(ptr::null(), &mut vt.render) };
        if made != ffi::SUCCESS {
            return None;
        }
        // SAFETY: as above, for the row iterator.
        let made = unsafe { ffi::ghostty_render_state_row_iterator_new(ptr::null(), &mut vt.rows) };
        if made != ffi::SUCCESS {
            return None;
        }
        // SAFETY: as above, for the row cells.
        let made = unsafe { ffi::ghostty_render_state_row_cells_new(ptr::null(), &mut vt.cells) };
        if made != ffi::SUCCESS {
            return None;
        }
        vt.configure(scrollback_lines).then_some(vt)
    }

    /// oso's settings (ADR 0120's table and step 1's conditions) and its callbacks.
    fn configure(&mut self, scrollback_lines: usize) -> bool {
        let unlimited = usize::MAX;
        let off = false;
        let no_images = 0u64;
        let unknown = limits::UNKNOWN_SEQUENCE_MAX;
        let kitty_apc = limits::KITTY_APC_MAX;
        let clipboard = limits::CLIPBOARD_WRITE_MAX;
        let settings: [(c_int, *const c_void); 23] = [
            (ffi::OPT_USERDATA, self.sink.as_ptr().cast_const().cast()),
            // The byte limit first: NULL would restore a small default (step 1's condition 4).
            (ffi::OPT_SCROLLBACK_MAX_BYTES, (&raw const unlimited).cast()),
            (
                ffi::OPT_SCROLLBACK_MAX_LINES,
                (&raw const scrollback_lines).cast(),
            ),
            (ffi::OPT_TITLE_REPORT, (&raw const off).cast()),
            (ffi::OPT_XT_CHECKSUM_REPORT, (&raw const off).cast()),
            // Kitty graphics stay off until ADR 0120's step 12.
            (
                ffi::OPT_KITTY_IMAGE_STORAGE_LIMIT,
                (&raw const no_images).cast(),
            ),
            (ffi::OPT_KITTY_IMAGE_MEDIUM_FILE, (&raw const off).cast()),
            (ffi::OPT_KITTY_IMAGE_MEDIUM_TEMP_FILE, ptr::null()),
            (
                ffi::OPT_KITTY_IMAGE_MEDIUM_SHARED_MEM,
                (&raw const off).cast(),
            ),
            (ffi::OPT_APC_MAX_BYTES_KITTY, (&raw const kitty_apc).cast()),
            (ffi::OPT_UNKNOWN_MAX_BYTES, (&raw const unknown).cast()),
            (
                ffi::OPT_CLIPBOARD_WRITE_MAX_BYTES,
                (&raw const clipboard).cast(),
            ),
            (
                ffi::OPT_WRITE_PTY,
                on_write_pty as ffi::WritePtyFn as *const c_void,
            ),
            (ffi::OPT_BELL, on_bell as ffi::BellFn as *const c_void),
            (
                ffi::OPT_XTVERSION,
                on_xtversion as ffi::XtversionFn as *const c_void,
            ),
            (
                ffi::OPT_TITLE_CHANGED,
                on_title as ffi::ChangedFn as *const c_void,
            ),
            (
                ffi::OPT_PWD_CHANGED,
                on_pwd as ffi::ChangedFn as *const c_void,
            ),
            (ffi::OPT_SIZE, on_size as ffi::SizeFn as *const c_void),
            (
                ffi::OPT_CLIPBOARD_WRITE,
                on_clipboard_write as ffi::ClipboardWriteFn as *const c_void,
            ),
            (
                ffi::OPT_DESKTOP_NOTIFICATION,
                on_notification as ffi::DesktopNotificationFn as *const c_void,
            ),
            (
                ffi::OPT_PROGRESS_REPORT,
                on_progress as ffi::ProgressReportFn as *const c_void,
            ),
            (
                ffi::OPT_SEMANTIC_PROMPT,
                on_prompt as ffi::SemanticPromptFn as *const c_void,
            ),
            (
                ffi::OPT_UNKNOWN_SEQUENCE,
                on_unknown as ffi::UnknownSequenceFn as *const c_void,
            ),
        ];
        settings.into_iter().all(|(option, value)| {
            // SAFETY: the terminal is live. Each value is what terminal.h gives as the
            // option's input type: the userdata pointer itself, a pointer to a live local of
            // the documented type (`size_t`, `uint64_t`, `bool`), NULL where NULL disables,
            // or a callback of the documented signature (`ffi`'s types, which
            // `manifest_tests` checks). The library copies scalars before it returns.
            unsafe { ffi::ghostty_terminal_set(self.terminal, option, value) == ffi::SUCCESS }
        })
    }

    /// The default colors and the 256-color palette the terminal draws and answers queries
    /// from; a cursor of `None` clears the cursor color.
    pub fn set_colors(
        &mut self,
        foreground: ffi::ColorRgb,
        background: ffi::ColorRgb,
        cursor: Option<ffi::ColorRgb>,
        palette: &[ffi::ColorRgb; 256],
    ) -> bool {
        let cursor_ptr = cursor
            .as_ref()
            .map_or(ptr::null(), |c| ptr::from_ref(c).cast());
        let settings: [(c_int, *const c_void); 4] = [
            (ffi::OPT_COLOR_FOREGROUND, (&raw const foreground).cast()),
            (ffi::OPT_COLOR_BACKGROUND, (&raw const background).cast()),
            (ffi::OPT_COLOR_CURSOR, cursor_ptr),
            (ffi::OPT_COLOR_PALETTE, palette.as_ptr().cast()),
        ];
        settings.into_iter().all(|(option, value)| {
            // SAFETY: the terminal is live; each value points to a live `GhosttyColorRgb`,
            // or to 256 of them for the palette, or is NULL for an unset cursor color, as
            // terminal.h documents. The library copies them.
            unsafe { ffi::ghostty_terminal_set(self.terminal, option, value) == ffi::SUCCESS }
        })
    }

    /// Parses `bytes`; the callbacks run meanwhile.
    pub fn write(&mut self, bytes: &[u8]) {
        // SAFETY: the terminal is live and `bytes` is a readable slice for the call. The
        // callbacks' sink is reached only through the userdata while this runs: `self` is
        // borrowed mutably, so no `&Sink` of ours is alive.
        unsafe { ffi::ghostty_terminal_vt_write(self.terminal, bytes.as_ptr(), bytes.len()) };
    }

    /// Resizes the grid, and what size reports answer.
    pub fn resize(&mut self, columns: u16, rows: u16, cell_width: u32, cell_height: u32) {
        self.sink().size = ffi::SizeReportSize {
            rows,
            columns,
            cell_width,
            cell_height,
        };
        // SAFETY: the terminal is live; callbacks may run, as for `write`.
        let resized = unsafe {
            ffi::ghostty_terminal_resize(self.terminal, columns, rows, cell_width, cell_height)
        };
        debug_assert_eq!(resized, ffi::SUCCESS);
    }

    /// The callbacks' sink, between calls into the library.
    pub fn sink(&mut self) -> &mut Sink {
        // SAFETY: `sink` came from `Box::leak` in `new` and lives until `drop`. The library
        // reaches it only during `write` and `resize`, which take `&mut self`, so while this
        // borrow lives no callback can run.
        unsafe { self.sink.as_mut() }
    }

    /// The scrollback's length, the active screen and the kitty keyboard flags.
    pub fn state(&self) -> State {
        let mut state = State::default();
        let mut screen = ffi::TERMINAL_SCREEN_PRIMARY;
        let reads: [(c_int, *mut c_void); 3] = [
            (ffi::DATA_SCROLLBACK_ROWS, (&raw mut state.history).cast()),
            (ffi::DATA_ACTIVE_SCREEN, (&raw mut screen).cast()),
            (
                ffi::DATA_KITTY_KEYBOARD_FLAGS,
                (&raw mut state.kitty_keyboard).cast(),
            ),
        ];
        for (data, out) in reads {
            // SAFETY: the terminal is live, and each `out` points to a live value of the
            // data's output type in terminal.h: `size_t`, the screen enum (`int`), `uint8_t`.
            unsafe { ffi::ghostty_terminal_get(self.terminal, data, out) };
        }
        state.alternate_screen = screen == ffi::TERMINAL_SCREEN_ALTERNATE;
        state
    }

    /// Whether each of the DEC private modes in `modes` is set.
    pub fn dec_modes<const N: usize>(&self, modes: [u16; N]) -> [bool; N] {
        modes.map(|number| {
            let mut config = ffi::ModeConfig {
                mode: ffi::dec_mode(number),
                value: false,
            };
            // SAFETY: the terminal is live; `DATA_MODE` reads and writes a
            // `GhosttyTerminalModeConfig`, which `config` is.
            let read = unsafe {
                ffi::ghostty_terminal_get(self.terminal, ffi::DATA_MODE, (&raw mut config).cast())
            };
            read == ffi::SUCCESS && config.value
        })
    }

    /// Brings the render state up to the terminal.
    pub fn update(&mut self) {
        // SAFETY: both handles are live and this thread's alone.
        let updated = unsafe { ffi::ghostty_render_state_update(self.render, self.terminal) };
        debug_assert_eq!(updated, ffi::SUCCESS);
    }

    /// Marks every row clean, after a snapshot took them.
    pub fn clean(&mut self) {
        // SAFETY: the render state is live.
        unsafe { ffi::ghostty_render_state_clean(self.render) };
    }

    /// The render state's size, dirtiness and cursor, and the row iterator set to its first
    /// row.
    pub fn frame(&mut self) -> Frame {
        let mut frame = Frame {
            columns: 0,
            rows: 0,
            dirty: ffi::RENDER_DIRTY_FULL,
            cursor: ffi::RenderCursor::sized(),
        };
        let mut iterator = self.rows;
        let reads: [(c_int, *mut c_void); 5] = [
            (ffi::RS_DATA_COLS, (&raw mut frame.columns).cast()),
            (ffi::RS_DATA_ROWS, (&raw mut frame.rows).cast()),
            (ffi::RS_DATA_DIRTY, (&raw mut frame.dirty).cast()),
            (ffi::RS_DATA_CURSOR, (&raw mut frame.cursor).cast()),
            (ffi::RS_DATA_ROW_ITERATOR, (&raw mut iterator).cast()),
        ];
        for (data, out) in reads {
            // SAFETY: the render state is live, and each `out` points to a live value of
            // the data's output type in render.h: `uint16_t` twice, the dirty enum (`int`),
            // a sized `GhosttyRenderStateCursor`, and the iterator handle `new` made, which
            // the library points at the first row.
            unsafe { ffi::ghostty_render_state_get(self.render, data, out) };
        }
        frame
    }

    /// Moves the row iterator to the next row; false past the last.
    pub fn next_row(&mut self) -> bool {
        // SAFETY: the iterator is live and was set by `frame`.
        unsafe { ffi::ghostty_render_state_row_iterator_next(self.rows) }
    }

    /// Whether the current row changed since the last `clean`.
    pub fn row_dirty(&mut self) -> bool {
        let mut dirty = false;
        // SAFETY: the iterator is on a row (`next_row` returned true); `ROW_DATA_DIRTY`
        // writes a `bool`.
        unsafe {
            ffi::ghostty_render_state_row_get(
                self.rows,
                ffi::ROW_DATA_DIRTY,
                (&raw mut dirty).cast(),
            )
        };
        dirty
    }

    /// The current row's packed cells, copied into `raw`, with the row cells handle pointed
    /// at it. Returns whether the row soft-wraps into the next.
    pub fn row(&mut self, raw: &mut Vec<ffi::Cell>) -> bool {
        let mut row: ffi::Row = 0;
        let mut view = ffi::CellsView {
            ptr: ptr::null(),
            len: 0,
        };
        let mut cells = self.cells;
        let reads: [(c_int, *mut c_void); 3] = [
            (ffi::ROW_DATA_RAW, (&raw mut row).cast()),
            (ffi::ROW_DATA_CELLS_RAW, (&raw mut view).cast()),
            (ffi::ROW_DATA_CELLS, (&raw mut cells).cast()),
        ];
        for (data, out) in reads {
            // SAFETY: the iterator is on a row (`next_row` returned true), and each `out`
            // points to a live value of the data's output type in render.h: `GhosttyRow`,
            // `GhosttyCellsView`, and the row cells handle `new` made.
            unsafe { ffi::ghostty_render_state_row_get(self.rows, data, out) };
        }
        raw.clear();
        if !view.ptr.is_null() {
            // SAFETY: the library points `view` at `len` packed cells it owns, valid until the
            // render state changes; they are copied out at once.
            raw.extend_from_slice(unsafe { std::slice::from_raw_parts(view.ptr, view.len) });
        }
        let mut wrapped = false;
        // SAFETY: `row` is a value the library gave; `ROW_DATA_WRAP` writes a `bool`.
        unsafe { ffi::ghostty_row_get(row, ffi::ROW_DATA_WRAP, (&raw mut wrapped).cast()) };
        wrapped
    }

    /// The style of the current row's cell at `column`.
    pub fn style(&mut self, column: u16) -> ffi::Style {
        let mut style = ffi::Style::DEFAULT;
        if self.select(column) {
            // SAFETY: the row cells handle is on the current row with a cell selected;
            // `CELLS_DATA_STYLE` writes a `GhosttyStyle`, which `style` is.
            unsafe {
                ffi::ghostty_render_state_row_cells_get(
                    self.cells,
                    ffi::CELLS_DATA_STYLE,
                    (&raw mut style).cast(),
                )
            };
        }
        style
    }

    /// The UTF-8 text of the current row's cell at `column`, into `buf`.
    pub fn grapheme(&mut self, column: u16, buf: &mut Vec<u8>) {
        buf.clear();
        if !self.select(column) {
            return;
        }
        // A cluster longer than this is cut: no grid draws more of one cell.
        buf.resize(256, 0);
        let mut out = ffi::GBuffer {
            ptr: buf.as_mut_ptr(),
            cap: buf.len(),
            len: 0,
        };
        // SAFETY: a cell is selected, and `out` describes `buf`'s initialised bytes, which
        // the library writes at most `cap` of, reporting how many in `len`.
        let read = unsafe {
            ffi::ghostty_render_state_row_cells_get(
                self.cells,
                ffi::CELLS_DATA_GRAPHEMES_UTF8,
                (&raw mut out).cast(),
            )
        };
        buf.truncate(if read == ffi::SUCCESS {
            out.len.min(256)
        } else {
            0
        });
    }

    fn select(&mut self, column: u16) -> bool {
        // SAFETY: the row cells handle is on the current row, set by `row`.
        unsafe { ffi::ghostty_render_state_row_cells_select(self.cells, column) == ffi::SUCCESS }
    }
}

impl Drop for Vt {
    fn drop(&mut self) {
        if !self.cells.is_null() {
            // SAFETY: made by `new` and freed once, here.
            unsafe { ffi::ghostty_render_state_row_cells_free(self.cells) };
        }
        if !self.rows.is_null() {
            // SAFETY: as above.
            unsafe { ffi::ghostty_render_state_row_iterator_free(self.rows) };
        }
        if !self.render.is_null() {
            // SAFETY: as above.
            unsafe { ffi::ghostty_render_state_free(self.render) };
        }
        if !self.terminal.is_null() {
            // SAFETY: as above; no callback runs after the terminal is freed.
            unsafe { ffi::ghostty_terminal_free(self.terminal) };
        }
        // SAFETY: `sink` came from `Box::leak` in `new`, and the terminal that held it as
        // userdata is gone.
        drop(unsafe { Box::from_raw(self.sink.as_ptr()) });
    }
}

/// The library's version, as it reports it, for `manifest_tests`.
#[cfg(test)]
pub(super) fn version() -> String {
    let mut version = ffi::GString::EMPTY;
    // SAFETY: `BUILD_INFO_VERSION_STRING` writes a `GhosttyString` to static data.
    unsafe { ffi::ghostty_build_info(ffi::BUILD_INFO_VERSION_STRING, (&raw mut version).cast()) };
    String::from_utf8_lossy(bytes(&version)).into_owned()
}

/// The linked library's type manifest (types.h), for `manifest_tests`.
#[cfg(test)]
pub(super) fn type_json() -> &'static str {
    // SAFETY: no precondition; the library returns a pointer to static data.
    let json = unsafe { ffi::ghostty_type_json() };
    // SAFETY: types.h documents a NUL-terminated string valid for the process's lifetime.
    let json = unsafe { std::ffi::CStr::from_ptr(json) };
    json.to_str().unwrap_or_default()
}

/// The library's own reading of a packed cell, for `manifest_tests`' cross-check.
#[cfg(test)]
pub(super) fn cell_get_u32(cell: ffi::Cell, data: c_int) -> u32 {
    let mut value = 0u32;
    // SAFETY: the tests pass only data whose output type fits in a zeroed `u32` (the
    // code point, the enums, the style id, a bool, a palette index), read back as its
    // little-endian low bytes.
    unsafe { ffi::ghostty_cell_get(cell, data, (&raw mut value).cast()) };
    value
}
