//! Every bound oso keeps on a language server, whatever it sends (ADR 0138, Q12): the
//! framing's, the JSON's, what an answer may carry, what may wait, and how often reports
//! wake the UI. Each is here, in one place, with the step of ADR 0138 that enforces it;
//! the part that enforces one reads it from here.
//!
//! Must not depend on: anything else in the crate.

use std::time::Duration;

/// The most lines in a message's header, the blank line that ends it included (step 2,
/// `framing`).
pub const HEADER_LINES_MAX: usize = 32;
/// The most bytes in one header line, its CRLF included (step 2, `framing`).
pub const HEADER_LINE_BYTES_MAX: usize = 1 << 10;
/// The most bytes in a whole header, its blank line included (step 2, `framing`).
pub const HEADER_BYTES_MAX: usize = 4 << 10;
/// The most digits a `Content-Length` may have; more is a broken header (step 2,
/// `framing`). With 19, the value cannot overflow before it is compared.
pub const CONTENT_LENGTH_DIGITS_MAX: usize = 19;
/// The largest `Content-Length` a header may declare: a message longer than the flood
/// bound is a broken header, so one endless message cannot hide from that bound (step 2,
/// `framing`).
pub const DECLARED_BYTES_MAX: u64 = FLOOD_BYTES_MAX as u64;

/// The largest message oso parses. A longer one is read through and dropped (step 2,
/// `framing`).
pub const MESSAGE_BYTES_MAX: usize = 16 << 20;
/// The most bytes read from the server at once, and the size of the pieces a message over
/// [`MESSAGE_BYTES_MAX`] is read through in (step 2, `framing`).
pub const READ_PIECE: usize = 64 << 10;

/// `serde_json`'s own nesting limit, which stays on: a message's arrays and objects nest
/// fewer than this many deep, 127 at most (step 2, `json`).
pub const JSON_DEPTH_LIMIT: usize = 128;
/// The most JSON values a message may hold, counted before it is parsed; a message with
/// more is dropped unparsed, as one over [`MESSAGE_BYTES_MAX`] is (step 2, `json`). Parsed,
/// a million values take some 40–80 MiB.
pub const JSON_VALUES_MAX: usize = 1_000_000;

/// The longest URI read: 4 × Linux's `PATH_MAX`, room for `%XX` escapes (step 2, `uri`).
pub const URI_BYTES_MAX: usize = 16 << 10;

/// The most items kept of a completion answer; the rest are dropped (step 9).
pub const COMPLETION_ITEMS_MAX: usize = 5000;
/// The most bytes kept of a completion's label (step 9).
pub const LABEL_BYTES_MAX: usize = 1 << 10;
/// The most bytes kept of a completion's detail (step 9).
pub const DETAIL_BYTES_MAX: usize = 4 << 10;
/// The most bytes kept of a completion's `sortText` (step 9); its `filterText` keeps as many
/// as a label.
pub const SORT_TEXT_BYTES_MAX: usize = 256;
/// The most bytes of text a completion inserts; an item with more is dropped (step 9).
pub const INSERT_TEXT_BYTES_MAX: usize = 16 << 10;
/// The most trigger characters of a server's kept (step 9b).
pub const TRIGGERS_MAX: usize = 16;
/// The most lines an edit besides a completion's own may replace; a list with one past it is
/// dropped whole, as the person sees no more than which lines changed (step 9b).
pub const EXTRA_EDIT_LINES_MAX: u32 = 8;
/// The most bytes kept of a diagnostic's message (step 6).
pub const DIAGNOSTIC_MESSAGE_BYTES_MAX: usize = 8 << 10;
/// The most bytes kept of hover or documentation text (steps 9 and 10).
pub const HOVER_BYTES_MAX: usize = 64 << 10;
/// The most pieces of a hover's list of `MarkedString`s read (step 10).
pub const HOVER_PIECES_MAX: usize = 256;
/// The most bytes of the language a hover's fenced block names (step 10).
pub const HOVER_LANGUAGE_BYTES_MAX: usize = 32;

/// The most bytes kept of a diagnostic's source and of its code, each (step 6).
pub const DIAGNOSTIC_LABEL_BYTES_MAX: usize = 256;
/// The most diagnostics kept for one document, the most severe first (step 6).
pub const DIAGNOSTICS_MAX: usize = 2000;
/// The most documents with diagnostics; past it, the oldest unopened one goes first
/// (step 6).
pub const DIAGNOSED_DOCUMENTS_MAX: usize = 10_000;
/// The most bytes of diagnostics a server's session keeps in all, their strings and a fixed
/// cost for each entry; past it, the oldest unopened document's go first (step 6). Without
/// it, the bounds above would allow some 160 GiB.
pub const DIAGNOSTICS_BYTES_MAX: usize = 64 << 20;
/// The most rows the list of a project's diagnostics shows, `M-g d`; past it, the count says
/// `+` (step 6b).
pub const DIAGNOSTIC_ROWS_MAX: usize = 100_000;
/// The most locations kept of one answer (step 8).
pub const LOCATIONS_MAX: usize = 1000;
/// The most edits in a completion's `additionalTextEdits` (step 9).
pub const ADDITIONAL_EDITS_MAX: usize = 16;
/// The most bytes of text those edits may insert in all (step 9).
pub const ADDITIONAL_EDITS_BYTES_MAX: usize = 64 << 10;

/// The most bytes queued for the writer. Sync cannot drop a message, so past this the
/// server is stopped (step 3).
pub const WRITER_QUEUE_BYTES_MAX: usize = 64 << 20;
/// The texts kept of each document, one for each version given, to map a late report forward
/// (step 6).
pub const KEPT_VERSIONS_MAX: usize = 16;

/// How long `initialize` may take (step 3).
pub const INITIALIZE_WAIT: Duration = Duration::from_secs(60);
/// How long a definition or references request may take (step 8).
pub const DEFINITION_WAIT: Duration = Duration::from_secs(10);
/// How long a completion or hover request may take (steps 9 and 10).
pub const COMPLETION_WAIT: Duration = Duration::from_secs(3);
/// How long `shutdown` may take (step 3).
pub const SHUTDOWN_WAIT: Duration = Duration::from_secs(2);

/// Diagnostics and progress wake the UI at most once in this time, coalesced to the latest
/// of each document. An answer to a request wakes it at once (steps 3 and 6).
pub const REPORT_INTERVAL: Duration = Duration::from_millis(50);
/// `window/showMessage` reaches the echo area at most once in this time; the rest go only
/// to `*LSP*` (step 3).
pub const SHOW_MESSAGE_INTERVAL: Duration = Duration::from_secs(1);

/// The window in which a server's messages are counted (step 3).
pub const FLOOD_WINDOW: Duration = Duration::from_secs(10);
/// The most messages from a server in [`FLOOD_WINDOW`]; past it the server is stopped
/// (step 3).
pub const FLOOD_MESSAGES_MAX: usize = 10_000;
/// The most bytes from a server in [`FLOOD_WINDOW`]; past it the server is stopped
/// (step 3).
pub const FLOOD_BYTES_MAX: usize = 256 << 20;

/// The most bytes kept of a server's standard error, the last ones, for `*LSP*` (step 3).
pub const STDERR_KEPT_BYTES: usize = 256 << 10;
/// The largest document opened with a server (step 4).
pub const DOCUMENT_BYTES_MAX: usize = 8 << 20;

/// The most bytes of a method's name; a message with a longer one is not JSON-RPC as oso
/// reads it, and stops the server (step 3, `protocol`).
pub const METHOD_BYTES_MAX: usize = 256;
/// The most bytes of a string id a server gives its request; a longer one gets no answer
/// (step 3, `protocol`).
pub const ID_BYTES_MAX: usize = 256;
/// The most bytes kept of a server's message for the person: `window/showMessage`,
/// `window/logMessage`, an error's message, a line of its standard error (step 3).
pub const SHOWN_BYTES_MAX: usize = 1 << 10;
/// The most items a `workspace/configuration` request may ask for; a longer one gets an
/// error (step 3, `protocol`).
pub const CONFIGURATION_ITEMS_MAX: usize = 256;
/// The most progress tokens followed at once; a server that begins more has the rest
/// ignored (step 3, `protocol`).
pub const PROGRESS_TOKENS_MAX: usize = 64;
/// The most bytes the reader hands the session thread before it waits for them to be taken,
/// two of the largest messages: a slow session holds up the server's writes rather than
/// growing memory (step 3, `server`).
pub const READER_QUEUE_BYTES_MAX: usize = 2 * MESSAGE_BYTES_MAX;

/// The waits before each start after a crash: the first, the second, and every later one
/// (step 3, `server`).
pub const RESTART_DELAYS: [Duration; 3] = [
    Duration::from_secs(1),
    Duration::from_secs(5),
    Duration::from_secs(30),
];
/// This many crashes within [`CRASH_WINDOW`] stop a server for the session (step 3).
pub const CRASHES_MAX: usize = 3;
/// The time in which [`CRASHES_MAX`] crashes stop a server (step 3).
pub const CRASH_WINDOW: Duration = Duration::from_secs(180);
/// How long a server may take to end after `exit`, and then after `SIGTERM`, before the next
/// step (step 3).
pub const EXIT_WAIT: Duration = Duration::from_secs(2);
/// How long what a server left in its group has, after the server ended and its group got
/// `SIGTERM`, before the group gets `SIGKILL` (step 3, `server`).
pub const GROUP_GRACE: Duration = Duration::from_millis(250);
/// How often the session thread looks whether its server has ended, also when its output
/// stays open, as when something it started holds the pipe (step 3).
pub const LIVENESS_INTERVAL: Duration = Duration::from_millis(500);

/// The most bytes `*LSP*` keeps, the last whole rows (step 3, the tip crate, which passes it
/// to the editor).
pub const LOG_BYTES_MAX: usize = 1 << 20;
/// The most bytes of `*LSP*`'s lines a server's outbox holds until the UI takes them, the last
/// ones (step 3, `server`).
pub const REPORT_LOG_BYTES_MAX: usize = 1 << 20;
/// The most messages for the person an outbox holds until the UI takes them; the rest go
/// (step 3, `server`).
pub const REPORT_SHOWN_MAX: usize = 16;
/// The most bytes of a keyword a server sends, such as a progress's `kind` or a position
/// encoding's name; a longer one is no keyword oso knows (step 3, `protocol`).
pub const KEYWORD_BYTES_MAX: usize = 16;
