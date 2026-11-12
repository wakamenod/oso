//! EditorConfig (ADR 0115): parsing a `.editorconfig` and applying it to a file's path, as the
//! loader thread does before a file's first screen. Run with `-- editorconfig`.
//!
//! Must not depend on: other workspace crates or crate internals; it uses only the public
//! API.

use std::time::{Duration, Instant};

use oso_core::editorconfig::{Budget, File, MAX_FILE, MAX_GLOB, Properties, VISIT_GLOBS};

/// A path several directories deep, as most files are.
const PATH: &str = "crates/oso-core/src/editor/indent/region.rs";

/// p50 and p99 of parsing `text` and applying it to [`PATH`], and what it found wrong.
fn costs(label: &str, text: &str) {
    let mut samples: Vec<Duration> = Vec::new();
    let mut problems = 0;
    for _ in 0..21 {
        let started = Instant::now();
        let file = File::parse(text, &mut Budget::default());
        let mut properties = Properties::default();
        properties.apply(&file, 0, PATH);
        std::hint::black_box(properties.indent_size());
        samples.push(started.elapsed());
        problems = file.problems().len();
    }
    samples.sort();
    println!(
        "{label:<52} {:>5} KiB  p50 {:>9.2?}  p99 {:>9.2?}  ({problems} skipped)",
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
    costs("sections of 60 bytes of `?`", &filled(&"?".repeat(60)));
    costs(
        "sections of 16 bytes of `?` (the budget's worst)",
        &filled(&"?".repeat(16)),
    );
    costs("sections of 16 bytes of `[!a]`", &filled(&"[!a]".repeat(4)));
    costs(
        "one section of 2 KiB of `?`",
        &filled(&"?".repeat(VISIT_GLOBS)),
    );
    costs("sections of 60 bytes of `**`", &filled(&"**".repeat(30)));
    costs("sections of `{a,b}` 8 deep", &filled(&nested(8)));
    for (label, piece) in [
        ("4 KiB globs of `?`", "?"),
        ("4 KiB globs of `*a`", "*a"),
        ("4 KiB globs of `**/`", "**/"),
        ("4 KiB globs of `[!a-z]`", "[!a-z]"),
        ("4 KiB globs of `{a,b,c}`", "{a,b,c}"),
        ("4 KiB globs of `{1..99}`", "{1..99}"),
        ("4 KiB globs of `{**a,?b}`", "{**a,?b}"),
    ] {
        costs(label, &filled(&repeated(piece, MAX_GLOB)));
    }
}

/// Braces nested `depth` deep.
fn nested(depth: usize) -> String {
    format!("{}x{}", "{a,".repeat(depth), "}".repeat(depth))
}
