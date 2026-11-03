//! The per-file size limit and its ratchet (ADR 0038).
//!
//! A file's size is its number of lines outside top-level test items
//! (`#[cfg(test)]` or `#[cfg(all(test, …))]`), comments and blank lines
//! included. Files listed in `guardrails/size-allowlist.txt` may exceed the
//! limit up to their recorded size, and must leave the list once they are
//! back under it.

use std::collections::BTreeMap;

use crate::source::{Token, test_item_lines};

/// The most lines of non-test code a file may have. Changing it needs an ADR.
pub const MAX_LINES: usize = 600;

/// The lines of `src` outside top-level test items.
pub fn non_test_lines(src: &str, tokens: &[Token]) -> usize {
    let test_lines: usize = test_item_lines(tokens)
        .iter()
        .map(|(start, end)| end - start + 1)
        .sum();
    src.lines().count() - test_lines
}

/// The allowlist: each file with the most lines it may have.
pub fn parse_allowlist(text: &str) -> Result<BTreeMap<String, usize>, String> {
    let mut allowed = BTreeMap::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.split('#').next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }
        let entry = line
            .split_once(char::is_whitespace)
            .and_then(|(path, max)| Some((path.to_owned(), max.trim().parse().ok()?)));
        let Some((path, max)) = entry else {
            return Err(format!("line {}: expected `<path> <max lines>`", n + 1));
        };
        if allowed.insert(path, max).is_some() {
            return Err(format!("line {}: listed twice", n + 1));
        }
    }
    Ok(allowed)
}

/// Every file over its limit, and every allowlist entry that must go or
/// shrink. `sizes` maps each checked file to its size.
pub fn problems(sizes: &BTreeMap<String, usize>, allowed: &BTreeMap<String, usize>) -> Vec<String> {
    let mut problems = Vec::new();
    for (path, &size) in sizes {
        match allowed.get(path) {
            None if size > MAX_LINES => problems.push(format!(
                "{path} has {size} lines of non-test code, over the limit of {MAX_LINES} \
                 (ADR 0038); split it by responsibility"
            )),
            Some(_) if size <= MAX_LINES => problems.push(format!(
                "{path} is down to {size} lines; remove it from guardrails/size-allowlist.txt"
            )),
            Some(&max) if size > max => problems.push(format!(
                "{path} grew to {size} lines, past its allowlisted {max}; the allowlist may \
                 only shrink, so split the file instead"
            )),
            _ => {}
        }
    }
    for path in allowed.keys() {
        if !sizes.contains_key(path) {
            problems.push(format!(
                "{path} is in guardrails/size-allowlist.txt but does not exist; remove it"
            ));
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{MAX_LINES, non_test_lines, parse_allowlist, problems};
    use crate::source::tokenize;

    fn sizes(entries: &[(&str, usize)]) -> BTreeMap<String, usize> {
        entries.iter().map(|&(p, n)| (p.to_owned(), n)).collect()
    }

    #[test]
    fn test_modules_do_not_count() {
        let src = "fn a() {}\n\n#[cfg(test)]\nmod tests {\n    fn t() {}\n}\n";
        assert_eq!(non_test_lines(src, &tokenize(src)), 2);
    }

    #[test]
    fn the_allowlist_parses_and_rejects_bad_lines() {
        let allowed = parse_allowlist("# header\na.rs 700\nb.rs  650 # why\n").unwrap();
        assert_eq!(allowed, sizes(&[("a.rs", 700), ("b.rs", 650)]));
        assert!(parse_allowlist("a.rs\n").is_err());
        assert!(parse_allowlist("a.rs 1\na.rs 2\n").is_err());
    }

    #[test]
    fn the_ratchet_only_shrinks() {
        let allowed = sizes(&[("big.rs", 700)]);
        let ok = sizes(&[("big.rs", 690), ("small.rs", MAX_LINES)]);
        assert!(problems(&ok, &allowed).is_empty());
        for bad in [
            sizes(&[("big.rs", 701)]),
            sizes(&[("big.rs", MAX_LINES)]),
            sizes(&[("big.rs", 700), ("new.rs", MAX_LINES + 1)]),
            sizes(&[]),
        ] {
            assert_eq!(problems(&bad, &allowed).len(), 1, "{bad:?}");
        }
    }
}
