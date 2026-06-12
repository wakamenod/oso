/// Cursor data model (data layer).
///
/// Pixel coordinates (screen_x / screen_y) are computed by the renderer,
/// so this struct holds only the logical position within the text buffer.
#[derive(Default)]
pub struct Cursor {
    /// Byte offset of the cursor in the text buffer (initial value: 0).
    pub position: usize,
}
