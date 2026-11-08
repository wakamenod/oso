//! Buffers the editor's tests share.
//!
//! Must not depend on: the host's window, threads or I/O.

use std::path::PathBuf;

use crate::buffer::Buffer;

/// A file that may hold secrets, judged by its name.
pub(super) fn secret_buffer() -> Buffer {
    let path = PathBuf::from("/tmp/.env");
    let mut buffer = Buffer::visiting(path.clone());
    buffer.judge_secret(Some(&[path]));
    buffer
}
