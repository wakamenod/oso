//! EditorConfig (ADR 0115): parsing a `.editorconfig` and applying it to a file's path, as the
//! loader thread does before a file's first screen. Run with `-- editorconfig`.
//!
//! Must not depend on: other workspace crates or crate internals; it uses only the public
//! API.

use std::time::{Duration, Instant};

use std::path::{Path, PathBuf};

use oso_core::editorconfig::{Applied, Budget, File, MAX_FILE, MAX_RANGE_GLOB, VISIT_GLOBS};

/// A path several directories deep, as most files are.
const PATH: &str = "crates/oso-core/src/editor/indent/region.rs";

/// p50 and p99 of parsing `text` and applying it to [`PATH`], and what it found wrong.
fn costs(label: &str, text: &str) {
    costs_at(label, text, PATH);
}

/// [`costs`], applying it to `path`.
fn costs_at(label: &str, text: &str, path: &str) {
    let mut samples: Vec<Duration> = Vec::new();
    let mut problems = 0;
    for _ in 0..21 {
        let started = Instant::now();
        let file = File::parse(text, &mut Budget::default());
        problems = file.problems().len();
        let files = vec![(PathBuf::from("/.editorconfig"), file)];
        let applied = Applied::from_walk(&Path::new("/").join(path), files);
        std::hint::black_box(applied.properties().indent_size());
        samples.push(started.elapsed());
    }
    samples.sort();
    println!(
        "{label:<60} {:>5} KiB  p50 {:>9.2?}  p99 {:>9.2?}  ({problems} skipped)",
        text.len() >> 10,
        samples[samples.len() / 2],
        samples[samples.len() - 1],
    );
}

/// Sections of `glob`, each setting the indentation, up to the file's bound.
fn filled(glob: &str) -> String {
    let mut text = String::from("root = true\n");
    loop {
        let section = format!("[{glob}]\nindent_size = 2\n");
        if text.len() + section.len() > MAX_FILE {
            return text;
        }
        text.push_str(&section);
    }
}

/// A glob of `piece` repeated up to `bytes`.
fn repeated(piece: &str, bytes: usize) -> String {
    piece.repeat(bytes / piece.len())
}

pub fn editorconfig_costs() {
    println!("== EditorConfig: parse and apply to one path");
    let typical = "root = true\n\n[*]\nindent_style = space\nindent_size = 4\n\
                   end_of_line = lf\ncharset = utf-8\ntrim_trailing_whitespace = true\n\
                   insert_final_newline = true\n\n[*.md]\ntrim_trailing_whitespace = false\n\n\
                   [Makefile]\nindent_style = tab\n\n[*.{js,ts,json,yml}]\nindent_size = 2\n";
    costs("a typical file", typical);
    costs("1000 sections of `*`", &filled("*"));
    costs("1000 sections of `*.{rs,toml}`", &filled("*.{rs,toml}"));
    costs("sections of 60 bytes of `**`", &filled(&"**".repeat(30)));
    costs("sections of `{a,b}` 8 deep", &filled(&nested(8)));
    // What a visit compiles at most: one glob of 2 KiB, or 128 of 16 bytes.
    for piece in ["?", "*a", "**/", "[!a-z]", "{a,b,c}", "{1..99}", "{**a,?b}"] {
        let one = repeated(piece, VISIT_GLOBS);
        costs(&format!("one 2 KiB glob of `{piece}`"), &filled_once(&one));
        let short = repeated(piece, 16);
        costs(
            &format!("128 globs of 16 bytes of `{piece}`"),
            &filled(&short),
        );
    }
    // Translating braces and sets that never close.
    costs(
        "2 KiB of `{` then `}`",
        &filled_once(&format!("{}{}", "{".repeat(1024), "}".repeat(1024))),
    );
    costs("2 KiB of `[`", &filled_once(&"[".repeat(VISIT_GLOBS)));
    costs("2 KiB of `{a`", &filled_once(&"{a".repeat(VISIT_GLOBS / 2)));
    // The longest globs with a number range, each against a 4 KiB path of digits it matches.
    let digits = "123/".repeat(1023) + "123";
    let ranged = format!("{}{{1..999}}", repeated("**/", MAX_RANGE_GLOB - 8));
    costs_at(
        "128 B of `**/` and a range, a 4 KiB path",
        &filled_once(&ranged),
        &digits,
    );
    let ranged = format!("{}{{1..999}}", repeated("**/{1..999}/", MAX_RANGE_GLOB - 8));
    costs_at(
        "128 B of `**/{1..999}/`, a 4 KiB path",
        &filled_once(&ranged),
        &digits,
    );
    let section = format!("[{ranged}]\nindent_size = 2\n");
    let text = format!(
        "root = true\n{}",
        section.repeat(VISIT_GLOBS / ranged.len() + 1)
    );
    costs_at("2 KiB of those globs, a 4 KiB path", &text, &digits);
    // Matching long paths.
    for (label, path) in [
        ("a 255-byte name", "a".repeat(255)),
        ("a 1 KiB path of 4-byte names", "aaa/".repeat(256)),
        ("a 4 KiB path of digits", "123/".repeat(1024)),
    ] {
        for piece in ["?", "*a", "**/"] {
            let glob = repeated(piece, VISIT_GLOBS);
            costs_at(
                &format!("one 2 KiB glob of `{piece}`, {label}"),
                &filled_once(&glob),
                &path,
            );
        }
    }
}

/// One section of `glob`, setting the indentation.
fn filled_once(glob: &str) -> String {
    format!("root = true\n[{glob}]\nindent_size = 2\n")
}

/// Braces nested `depth` deep.
fn nested(depth: usize) -> String {
    format!("{}x{}", "{a,".repeat(depth), "}".repeat(depth))
}
