//! The other forms of a romaji string's kana and letters (ADR 0116): katakana, half-width
//! katakana and full-width letters.
//!
//! Must not depend on: any module of this crate.

/// Full-width katakana with a half-width form of one character, and that form, in order.
const FULL: &str = "ァアィイゥウェエォオカキクケコサシスセソタチッツテトナニヌネノハヒフヘホマミムメモャヤュユョヨラリルレロワヲンー";
const HALF: &str = "ｧｱｨｲｩｳｪｴｫｵｶｷｸｹｺｻｼｽｾｿﾀﾁｯﾂﾃﾄﾅﾆﾇﾈﾉﾊﾋﾌﾍﾎﾏﾐﾑﾒﾓｬﾔｭﾕｮﾖﾗﾘﾙﾚﾛﾜｦﾝｰ";

/// The half-width voiced and semi-voiced sound marks.
const VOICED: char = 'ﾞ';
const SEMI_VOICED: char = 'ﾟ';

/// `c`'s katakana: hiragana moves to its katakana, and anything else stays.
pub(super) fn katakana(c: char) -> char {
    match c {
        'ぁ'..='ゖ' => char::from_u32(u32::from(c) + 0x60).unwrap_or(c),
        _ => c,
    }
}

/// Pushes `c`, a katakana, in half-width onto `out`: one character, or two with a sound
/// mark (`ガ` is `ｶﾞ`). A katakana with no half-width form takes its nearest (`ヮ` is `ﾜ`),
/// and anything else stays as it is.
pub(super) fn push_half_width(c: char, out: &mut String) {
    let near = match c {
        'ヮ' => 'ワ',
        'ヰ' => 'イ',
        'ヱ' => 'エ',
        'ヵ' => 'カ',
        'ヶ' => 'ケ',
        c => c,
    };
    if let Some(half) = half(near) {
        out.push(half);
        return;
    }
    if c == 'ヴ' {
        out.push('ｳ');
        out.push(VOICED);
        return;
    }
    // `ガ` follows `カ`, and `パ` follows `ハ` and `バ`.
    let code = u32::from(c);
    let before = |by: u32| code.checked_sub(by).and_then(char::from_u32).and_then(half);
    let marked = match c {
        'ガ'..='ド' | 'バ' | 'ビ' | 'ブ' | 'ベ' | 'ボ' => {
            before(1).map(|base| (base, VOICED))
        }
        'パ' | 'ピ' | 'プ' | 'ペ' | 'ポ' => before(2).map(|base| (base, SEMI_VOICED)),
        _ => None,
    };
    match marked {
        Some((base, mark)) => {
            out.push(base);
            out.push(mark);
        }
        None => out.push(c),
    }
}

/// The half-width form of a full-width katakana that has one of a single character.
fn half(c: char) -> Option<char> {
    let at = FULL.chars().position(|full| full == c)?;
    HALF.chars().nth(at)
}

/// `c`'s full-width form: an ASCII letter, `-` or `'` moves to the one in the full-width
/// block, and anything else stays.
pub(super) fn full_width(c: char) -> char {
    match c {
        'a'..='z' | '\'' | '-' => char::from_u32(u32::from(c) - 0x20 + 0xFF00).unwrap_or(c),
        _ => c,
    }
}
