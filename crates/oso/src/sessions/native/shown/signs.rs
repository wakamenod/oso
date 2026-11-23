//! Which characters a reply's line may start with, or a link's text hold, without looking like
//! something of oso's (ADR 0165, "The look: Markdown"): allowlists, as no list of look-alikes
//! is whole.
//!
//! Must not depend on: anything but the characters.

/// Whether `line` starts, past its spaces and a tree's `│`s, with a character that could be
/// taken for the mark a row of oso's starts with (`render` has them: `▸`, `›`, `●`, `∴`, `◌`,
/// `·`, `…`, `✗`, `⚠`, `──`, and `〉`): any that is not ASCII, a letter of [`word`], or one of a few
/// signs that look like none of them; or one that a mark of its own cluster changes.
pub(super) fn could_be_mark(line: &str) -> bool {
    let mut chars = past_spaces(line).chars();
    let Some(first) = chars.next() else {
        return false;
    };
    let second = chars.next();
    // A dash's look-alikes stand alone in a word, not in a row.
    let dash = |c: char| matches!(c, 'ー' | '一');
    if second.is_some_and(|c| (dash(first) && dash(c)) || !(stands(c) || c.is_whitespace())) {
        return true;
    }
    !(first.is_ascii() || word(first) || picture(first) || sign(first))
}

/// Whether `c` is a sign that looks like no row's mark: quotation marks, Japanese brackets and
/// stops, full-width ASCII but `－` and `＞`, half-width kana but its long vowel mark, circled
/// numbers, a tree's lines but `─`, and a check box.
fn sign(c: char) -> bool {
    let quote =
        matches!(c, '\u{2018}'..='\u{201d}' | '\u{3001}' | '\u{3002}' | '\u{300c}'..='\u{3011}');
    let full =
        matches!(c, '\u{ff01}'..='\u{ff0c}' | '\u{ff0e}'..='\u{ff1d}' | '\u{ff1f}'..='\u{ff5e}');
    let half = matches!(c, '\u{ff66}'..='\u{ff6f}' | '\u{ff71}'..='\u{ff9f}');
    let tree = "┌┐└┘├┤┬┴┼".contains(c);
    quote || full || half || tree || matches!(c, '\u{2460}'..='\u{2473}' | '\u{2705}')
}

/// Whether `c` is a picture in colour that looks like no sign: past the discs, triangles and
/// arrows among them.
fn picture(c: char) -> bool {
    match c {
        '\u{1f3fb}'..='\u{1f3ff}' => false,
        '\u{1f31f}'..='\u{1f517}' | '\u{1f519}'..='\u{1f52f}' => true,
        '\u{1f5fb}'..='\u{1f64f}' | '\u{1f680}'..='\u{1f6c5}' => true,
        '\u{1f90c}'..='\u{1f93a}' | '\u{1f93c}'..='\u{1f945}' => true,
        '\u{1f947}'..='\u{1f9ff}' => true,
        _ => false,
    }
}

/// Whether `c` stands on its own: it joins neither the character before it nor the one after,
/// so that what is hidden next to it changes nothing of how it shows. No list of what joins is
/// whole, so this is one of what does not.
pub(super) fn stands(c: char) -> bool {
    match c {
        ' '..='~' | '\u{a1}'..='\u{ac}' | '\u{ae}'..='\u{2ff}' => true,
        '\u{2010}'..='\u{2027}' | '\u{2030}'..='\u{205e}' | '\u{20a0}'..='\u{20bf}' => true,
        '\u{2190}'..='\u{23ff}' | '\u{2460}'..='\u{27bf}' | '\u{2900}'..='\u{2bff}' => true,
        '\u{3000}'..='\u{3029}' | '\u{3030}'..='\u{3098}' => true,
        '\u{309b}'..='\u{30ff}' | '\u{ff01}'..='\u{ff9d}' => true,
        _ => wide(c) || picture(c),
    }
}

/// `text` past its spaces and a tree's `│`s at its start.
pub(super) fn past_spaces(text: &str) -> &str {
    text.trim_start_matches(|c: char| c.is_whitespace() || c == '│')
}

/// Whether `c` is a letter or a digit of the scripts whose letters look like no sign: Latin,
/// Greek, Cyrillic, kana, the ideographs and Hangul, but those that look like a dot or a
/// stroke.
pub(super) fn word(c: char) -> bool {
    c.is_ascii_alphanumeric() || wide(c)
}

/// Whether `c` is a letter of [`word`] that is not ASCII.
pub(super) fn wide(c: char) -> bool {
    match c {
        '丶' | '丿' | '丨' | '亅' | '乀' | '乁' | '乙' | '乚' => false,
        '\u{c0}'..='\u{1bf}' | '\u{1c4}'..='\u{24f}' => c.is_alphabetic(),
        '\u{386}'..='\u{52f}' | '\u{3041}'..='\u{3096}' => c.is_alphabetic(),
        '\u{3005}' | '\u{30a1}'..='\u{30fa}' | '\u{30fc}' => true,
        '\u{3400}'..='\u{4dbf}' | '\u{4e00}'..='\u{9fff}' | '\u{ac00}'..='\u{d7a3}' => true,
        _ => false,
    }
}

/// Whether `c` may stand in a link's text whose target is hidden: no address can be written
/// in them.
pub(super) fn plain(c: char) -> bool {
    matches!(c, ' ' | '*' | '`') || (word(c) && c != 'ノ')
}

/// Whether `before` and `after`, the rest of the line on each side of a link, leave its target
/// hidden: nothing next to it could go on its text, and nothing on the line could read as an
/// address, or as the target oso puts in parentheses.
pub(super) fn around_link(before: &str, after: &str) -> bool {
    let before = before.trim_end_matches('*');
    let after = after.trim_start_matches('*');
    let next_to = before
        .chars()
        .next_back()
        .is_none_or(|c| matches!(c, ' ' | '\t' | '、' | '。' | '「' | '（') || kana(c));
    let follows = after.chars().next().is_none_or(|c| {
        matches!(
            c,
            ' ' | '.' | ',' | ';' | ':' | '!' | '?' | '、' | '。' | '」' | '）'
        ) || kana(c)
    });
    next_to && follows && calm(before, true) && calm(after, false)
}

/// Whether `text` holds no address, nor what could read as one: words, and stops that end one.
fn calm(text: &str, mut opens: bool) -> bool {
    let mut last = None;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        let ends = chars.peek().is_none_or(|next| matches!(next, ' ' | '\t'));
        let fits = match c {
            // A stop ends a word: it stands in none.
            '.' | ',' | ':' => ends,
            '。' | '、' => ends || last.is_some_and(kana),
            // Before a link, a bracket may open around it.
            '（' | '「' => opens,
            c => {
                plain(c)
                    || matches!(
                        c,
                        '\t' | '\'' | '"' | ';' | '!' | '?' | '-' | '_' | '#' | '>'
                    )
                    || matches!(c, '」' | '）' | '！' | '？')
            }
        };
        if !fits {
            return false;
        }
        if c == '（' {
            // One, and nothing after it but the link.
            opens = chars.peek().is_none();
            if !opens {
                return false;
            }
        }
        last = Some(c);
    }
    true
}

/// Whether `c` is a kana, an ideograph or a Hangul syllable that [`plain`] lets in.
fn kana(c: char) -> bool {
    c >= '\u{3005}' && plain(c)
}
