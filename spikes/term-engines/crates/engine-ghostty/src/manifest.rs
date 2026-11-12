//! Reads the linked library's ABI manifest (`ghostty_type_json`, types.h).
//!
//! The header says the packed `GhosttyCell` layout is not a stable ABI and that callers
//! who need speed should decode it with the bit positions from this manifest rather than
//! hardcoding them. `CellLayout` does that; when the manifest does not look as expected,
//! the engine falls back to `ghostty_cell_get_multi`. The manifest is compact JSON from
//! the library itself; this reads only the few keys it needs, with plain string search.

use std::ffi::CStr;

use crate::ffi;

pub fn json() -> &'static str {
    // SAFETY: the header documents the result as a NUL-terminated string valid for the
    // lifetime of the process.
    let s = unsafe { CStr::from_ptr(ffi::ghostty_type_json()) };
    s.to_str().unwrap_or("")
}

/// The JSON object of one type in the manifest, up to the next type's key.
pub fn type_section(name: &str) -> Option<&'static str> {
    let json = json();
    let key = format!("\"{name}\":{{");
    let start = json.find(&key)? + key.len();
    let rest = &json[start..];
    let end = rest.find(",\"Ghostty").unwrap_or(rest.len());
    Some(&rest[..end])
}

/// The integer after `"key":` (the first occurrence in `section` after `from`).
pub fn number_after(section: &str, key: &str) -> Option<u64> {
    let pat = format!("\"{key}\":");
    let start = section.find(&pat)? + pat.len();
    let digits: String = section[start..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

/// `(lsb, width)` of a packed field `"name":{"lsb":L,"width":W`.
fn bits(section: &str, name: &str) -> Option<Bits> {
    let pat = format!("\"{name}\":{{\"lsb\":");
    let start = section.find(&pat)? + pat.len() - "\"lsb\":".len();
    let field = &section[start..];
    let lsb = number_after(field, "lsb")?;
    let width = number_after(field, "width")?;
    Some(Bits {
        lsb: u32::try_from(lsb).ok()?,
        width: u32::try_from(width).ok()?,
    })
}

#[derive(Clone, Copy, Debug)]
pub struct Bits {
    pub lsb: u32,
    pub width: u32,
}

impl Bits {
    #[inline]
    pub fn get(self, v: u64) -> u64 {
        (v >> self.lsb) & ((1u64 << self.width) - 1)
    }

    fn offset(self, by: u32) -> Bits {
        Bits {
            lsb: self.lsb + by,
            width: self.width,
        }
    }
}

/// Bit positions of the packed `GhosttyCell` fields this crate reads.
#[derive(Clone, Copy, Debug)]
pub struct CellLayout {
    pub tag: Bits,
    pub codepoint: Bits,
    pub palette: Bits,
    pub r: Bits,
    pub g: Bits,
    pub b: Bits,
    pub style_id: Bits,
    pub wide: Bits,
    pub hyperlink: Bits,
}

impl CellLayout {
    pub fn parse() -> Option<CellLayout> {
        let json = json();
        if !json.contains("\"endian\":\"little\"") {
            return None;
        }
        let cell = type_section("GhosttyCell")?;
        if !cell.starts_with("\"kind\":\"packed\",\"size\":8,") {
            return None;
        }
        let content = bits(cell, "content")?;
        let layout = CellLayout {
            tag: bits(cell, "content_tag")?,
            codepoint: bits(cell, "codepoint")?.offset(content.lsb),
            palette: bits(cell, "index")?.offset(content.lsb),
            r: bits(cell, "r")?.offset(content.lsb),
            g: bits(cell, "g")?.offset(content.lsb),
            b: bits(cell, "b")?.offset(content.lsb),
            style_id: bits(cell, "style_id")?,
            wide: bits(cell, "wide")?,
            hyperlink: bits(cell, "hyperlink")?,
        };
        let all = [
            layout.tag,
            layout.codepoint,
            layout.palette,
            layout.r,
            layout.g,
            layout.b,
            layout.style_id,
            layout.wide,
            layout.hyperlink,
        ];
        let sane = all.iter().all(|b| b.width > 0 && b.lsb + b.width <= 64)
            && layout.codepoint.width <= 21
            && layout.style_id.width == 16
            && layout.palette.width == 8;
        sane.then_some(layout)
    }
}
