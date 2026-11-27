//! Whether a space's orchestrator may stand in for the person on a native worker's request
//! (ADR 0186): which requests it may allow, and how much of one it is shown. What stays the
//! person's is a [`PersonOnly`] reason.
//!
//! Must not depend on: the app, the file system or the clock; plain functions over a request's
//! tool and input, the worker's directory and the person's rules.
//!
//! - Every check is lexical and best-effort, never a boundary: a link inside the worker's
//!   directory may lead out of it, and a Bash command may spell a name or a command in ways no
//!   check reads (a variable, a glob, an encoding). Once the person lists Bash, the
//!   orchestrator's judgment is the guard.
//! - A name is judged as the disk may read it, so that a spelling that differs only there
//!   catches more, never less: letters that macOS and Windows fold to ASCII are taken as that
//!   ASCII, any other character beyond ASCII is dropped, ASCII case is ignored, and, as Windows
//!   reads a name, trailing dots and spaces and a stream after `:` are cut.
//! - A secret is judged before the person's rules, so that a request naming one is listed with
//!   no input whatever the rules say.

use std::path::{Component, Path, PathBuf};

use oso_core::buffer::Buffer;
use regex::{Regex, RegexBuilder};
use serde_json::{Value, json};

use super::asks::RequestId;
use crate::channel::protocol::permission::Call;

/// The most bytes of JSON a request's input is listed with: past them it is not listed at all.
pub const MAX_LISTED: usize = 8 * 1024;

/// The most compiled size of one of the person's deny patterns, which is not oso's.
const MAX_PATTERN_SIZE: usize = 1 << 20;

/// What a Bash command is always denied for, whatever the person adds: what reaches
/// another computer, pushes, merges or publishes, deletes a tree, runs as another user, starts
/// an agent or drives oso, or names Claude Code's settings, a socket or a `config.toml`, as
/// oso's is, which says what an orchestrator may answer. Each ignores case and crosses lines.
pub const DEFAULT_DENY: [&str; 18] = [
    r"\bgit\b.*\bpush\b",
    r"\bgh\b.*\b(?:pr\s+merge|repo|api|gist|release|secret|workflow)\b",
    r"\b(?:curl|wget|ssh|scp|sftp|rsync|nc|ncat|socat|telnet)\b",
    r"\b(?:sudo|doas|su)\b",
    r"\brm\b.*\s-(?:-r|[a-z]*r)",
    r"\bfind\b.*\s-delete\b",
    // As a command's name only, so that `oso-core` or a directory `oso/` is not one.
    r#"(?:^|[\s;&|(){}<>/`@])claude(?:-code)?(?:\.exe)?(?:$|[\s;&|(){}<>`@])"#,
    r#"(?:^|[\s;&|(){}<>/`])oso(?:-restart)?(?:\.exe)?(?:$|[\s;&|(){}<>`])"#,
    r"\.claude",
    r"\bsettings(?:\.local)?\.json",
    r"\.mcp\.json",
    r"config\.toml",
    r"\.sock\b",
    // A `..` part, from a worker whose directory is under a `.claude`, is added in `denied`.
    r"\bgit\b.*(?:\s-c\s|\bconfig\b).*\b(?:core\.sshcommand|alias|core\.hookspath)\b",
    r"\bgit\b.*\bremote\b.*\b(?:add|set-url)\b",
    r"\b(?:npx|bunx|pnpx|dlx)\b.*\b(?:anthropic|claude)\b",
    // What git runs for ssh or takes as its configuration, set in the environment.
    r"\bGIT_(?:SSH(?:_COMMAND)?|CONFIG_\w+)\s*=",
    r"\b(?:p?kill|killall)\b",
];

/// The tools whose fields name a file or a directory, and those fields.
const PATH_FIELDS: [&str; 3] = ["file_path", "path", "notebook_path"];

/// What the person lets a space's orchestrator answer (`[spaces] orchestrator-answers` and
/// `orchestrator-answers-deny`).
#[derive(Debug, Clone, Default)]
pub struct Rules {
    /// The tools it may allow: none means it allows nothing.
    pub tools: Vec<String>,
    /// The Bash commands it never allows.
    pub deny: Vec<Regex>,
}

impl Rules {
    /// The rules for `tools`, with [`DEFAULT_DENY`] and `deny`'s patterns, which add to the
    /// defaults and never take one away. Each pattern ignores case, as the disk on macOS and
    /// Windows finds a command, and `.` crosses lines; a pattern's own `(?-i)` or `(?-s)` turns
    /// either off.
    ///
    /// # Errors
    ///
    /// The first pattern of `deny` that does not compile, or is too large.
    pub fn new(tools: &[String], deny: &[String]) -> Result<Self, regex::Error> {
        let deny = (DEFAULT_DENY.into_iter())
            .chain(deny.iter().map(String::as_str))
            .map(compile)
            .collect::<Result<_, _>>()?;
        Ok(Self {
            tools: tools.to_vec(),
            deny,
        })
    }

    /// Whether `pattern` is one [`Rules::new`] takes, by itself.
    ///
    /// # Errors
    ///
    /// Why it does not compile, or is too large.
    pub fn check(pattern: &str) -> Result<(), regex::Error> {
        compile(pattern).map(drop)
    }
}

/// A deny pattern, its size bounded, as it is not oso's.
fn compile(pattern: &str) -> Result<Regex, regex::Error> {
    RegexBuilder::new(pattern)
        .case_insensitive(true)
        .dot_matches_new_line(true)
        .size_limit(MAX_PATTERN_SIZE)
        .build()
}

/// Why only the person may allow a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersonOnly {
    /// The person lets no orchestrator answer: no tool is listed.
    Off,
    /// Its tool is not listed, or is an MCP tool's, which none answers.
    Tool,
    /// It is a question or a plan, on a card.
    Card,
    /// Its input is not listed whole.
    NotShown,
    /// It names a file that may hold secrets.
    Secret,
    /// It names a path outside the worker's directory, or in `.git`, `.claude` or `.mcp.json`.
    Outside,
    /// Its command matches a deny pattern.
    Denied,
}

impl PersonOnly {
    /// Why, for the orchestrator.
    pub fn says(self) -> &'static str {
        match self {
            Self::Off => "the person lets no orchestrator answer: orchestrator-answers is empty",
            Self::Tool => "the person does not list this tool in orchestrator-answers",
            Self::Card => "questions and plans are the person's",
            Self::NotShown => "its input is not listed whole",
            Self::Secret => "it names a file that may hold secrets",
            Self::Outside => {
                "it names a path outside the worker's directory, or in .git, .claude or .mcp.json"
            }
            Self::Denied => "its command matches orchestrator-answers-deny",
        }
    }
}

/// Whether a space's orchestrator may allow the request of `tool` with `input`, on a card if
/// `card`, of a worker in `directory`, under `rules`; else why only the person may.
///
/// # Errors
///
/// Why only the person may allow it, the first of: [`PersonOnly::Card`],
/// [`PersonOnly::NotShown`], [`PersonOnly::Secret`], [`PersonOnly::Off`], [`PersonOnly::Tool`],
/// [`PersonOnly::Outside`] and [`PersonOnly::Denied`].
pub fn eligible(
    tool: &str,
    input: Option<&Value>,
    card: bool,
    directory: &Path,
    rules: &Rules,
) -> Result<(), PersonOnly> {
    if card {
        return Err(PersonOnly::Card);
    }
    let input = match listed(input, MAX_LISTED) {
        (Some(input), _) => input,
        (None, _) => return Err(PersonOnly::NotShown),
    };
    let bash = tool == "Bash";
    let command = input
        .get("command")
        .and_then(Value::as_str)
        .filter(|_| bash);
    if names_secret(tool, &input, command) {
        return Err(PersonOnly::Secret);
    }
    if bash && command.is_none() {
        return Err(PersonOnly::NotShown);
    }
    if rules.tools.is_empty() {
        return Err(PersonOnly::Off);
    }
    if tool.starts_with("mcp__") || !rules.tools.iter().any(|listed| listed == tool) {
        return Err(PersonOnly::Tool);
    }
    if reaches_outside(tool, &input, directory) {
        return Err(PersonOnly::Outside);
    }
    if command.is_some_and(|command| denied(command, directory, rules)) {
        return Err(PersonOnly::Denied);
    }
    Ok(())
}

/// The request `id` of `call`, on a card if `card`, shown `shown_s` seconds ago to the person, of
/// a worker in `directory`, as its space's orchestrator is listed it under `rules`: whether it
/// may allow it, and why not, and its input but where it names a secret, is a card's, or is
/// longer than [`MAX_LISTED`].
pub fn listing(
    id: RequestId,
    call: &Call,
    card: bool,
    shown_s: u64,
    directory: &Path,
    rules: &Rules,
) -> Value {
    let tool = call.tool_name.as_str();
    let input = call.input.as_ref();
    let verdict = eligible(tool, input, card, directory, rules);
    let (whole, cut) = listed(input, MAX_LISTED);
    let withheld = matches!(verdict, Err(PersonOnly::Card | PersonOnly::Secret));
    json!({
        "id": id.to_string(),
        "tool": tool,
        "input": whole.filter(|_| !withheld),
        "input_cut": cut,
        "shown_s": shown_s,
        "answerable": verdict.is_ok(),
        "person_only": verdict.err().map(PersonOnly::says),
    })
}

/// `input` as a listing shows it: whole if it is at most `max` bytes as JSON, else nothing,
/// and whether it was cut. An input the bridge did not pass is cut.
pub fn listed(input: Option<&Value>, max: usize) -> (Option<Value>, bool) {
    match input {
        Some(input) if json_fits(input, max) => (Some(input.clone()), false),
        _ => (None, true),
    }
}

/// Whether `value` is at most `max` bytes as JSON, counted without keeping them, and stopped
/// at the first byte past.
fn json_fits(value: &Value, max: usize) -> bool {
    struct Count {
        left: usize,
    }
    impl std::io::Write for Count {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.left = (self.left.checked_sub(bytes.len()))
                .ok_or_else(|| std::io::Error::other("past the most"))?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    serde_json::to_writer(&mut Count { left: max }, value).is_ok()
}

/// A field of `input` that names where a call acts: a path field, or a search's pattern of
/// names, Glob's `pattern` and Grep's `glob`, which is relative to its `path`.
#[derive(Clone, Copy)]
enum Field<'a> {
    Path(Option<&'a str>),
    Glob(Option<&'a str>),
}

/// The fields of `input` that name where a call acts. A field that is no string comes as
/// `None`, and a search with no `path` as an absent one, as it acts in the session's directory
/// at the time.
fn fields<'a>(tool: &str, input: &'a Value) -> Vec<Field<'a>> {
    let mut fields: Vec<Field<'a>> = (PATH_FIELDS.iter())
        .filter_map(|&field| input.get(field).map(|value| Field::Path(value.as_str())))
        .collect();
    let glob = match tool {
        "Glob" => Some("pattern"),
        "Grep" => Some("glob"),
        _ => None,
    };
    if let Some(glob) = glob {
        fields.extend(input.get(glob).map(|value| Field::Glob(value.as_str())));
        if input.get("path").is_none() {
            fields.push(Field::Path(None));
        }
    }
    fields
}

/// Whether a string of `input`, a token of `command` as the shell may join it, or a search's
/// pattern of names that may match one, names a file that may hold secrets (ADR 0044).
fn names_secret(tool: &str, input: &Value, command: Option<&str>) -> bool {
    let mut strings = Vec::new();
    leaves(input, &mut strings);
    let globs = fields(tool, input)
        .into_iter()
        .filter_map(|field| match field {
            Field::Glob(glob) => glob,
            Field::Path(_) => None,
        });
    let words =
        |leaf: &&str| secret_name(leaf) || (tokens(leaf).iter()).any(|word| secret_name(word));
    strings.iter().any(words)
        || globs.into_iter().any(glob_may_be_secret)
        || command.is_some_and(|command| (tokens(command).iter()).any(|token| secret_name(token)))
}

/// Every string in `value`, however deep, an object's keys among them.
fn leaves<'a>(value: &'a Value, strings: &mut Vec<&'a str>) {
    match value {
        Value::String(text) => strings.push(text),
        Value::Array(items) => items.iter().for_each(|item| leaves(item, strings)),
        Value::Object(fields) => fields.iter().for_each(|(key, item)| {
            strings.push(key);
            leaves(item, strings);
        }),
        _ => {}
    }
}

/// Whether a pattern of names may match a secret: with its `*` dropped it names one, or it has
/// a wildcard other than `*`, `**` and `*.ext`, or a list, which a judge of its text cannot
/// follow.
fn glob_may_be_secret(glob: &str) -> bool {
    let plain_part = |part: &str| {
        !part.contains('*')
            || part == "*"
            || part == "**"
            || part.strip_prefix("*.").is_some_and(|ext| {
                ext.split('.').all(|piece| {
                    !piece.is_empty() && piece.bytes().all(|b| b.is_ascii_alphanumeric())
                })
            })
    };
    glob.contains(['?', '[', ']', '{', '}', ','])
        || !glob.split(['/', '\\']).all(plain_part)
        || secret_name(&glob.replace('*', ""))
}

/// The tokens of `command` as the shell may join or split its words: its quotes and escapes
/// taken out, a line's continuation joined; once split at blanks and the shell's punctuation,
/// once with the empty expansions and lists inside a word (`$()`, backquotes, `{,}`, a `$`
/// before quotes) dropped and split at blanks and operators alone, and once read with
/// backslashes as a Windows path's; each also with its wildcards dropped.
fn tokens(command: &str) -> Vec<String> {
    let renderings = renderings(command);
    let mut tokens: Vec<String> = Vec::new();
    for (rendering, split) in
        renderings
            .iter()
            .zip([is_separator as fn(char) -> bool, is_operator, is_separator])
    {
        for token in rendering.split(split).filter(|token| !token.is_empty()) {
            tokens.push(token.to_owned());
            if token.contains(['*', '?', '[', ']']) {
                tokens.push(token.replace(['*', '?', '[', ']'], ""));
            }
        }
    }
    tokens
}

/// `command` as the shell may read it: unquoted, then with empty expansions and lists in a word
/// dropped, and with backslashes as a Windows path's.
fn renderings(command: &str) -> [String; 3] {
    let joined = command.replace("\\\r\n", "").replace("\\\n", "");
    let plain = joined.replace(['\'', '"', '\\'], "");
    let joined_words = plain.replace(['$', '(', ')', '`', '{', '}', ','], "");
    let windows = joined.replace(['\'', '"'], "").replace('\\', "/");
    [plain, joined_words, windows]
}

/// What splits a command into tokens, once unquoted: blanks and the shell's punctuation.
fn is_separator(c: char) -> bool {
    c.is_whitespace() || ";&|()<>{}=:,`$".contains(c)
}

/// What splits a command into words where nothing inside a word is: blanks and operators.
fn is_operator(c: char) -> bool {
    c.is_whitespace() || ";&|<>=:".contains(c)
}

/// Whether `name`, read as the disk may read it, matches a secret pattern: whole, and as
/// Windows cuts each part.
fn secret_name(name: &str) -> bool {
    let read = |windows: bool| -> PathBuf {
        (Path::new(name).components())
            .filter_map(|part| match part {
                Component::Normal(part) => {
                    let folded = folded(&part.to_string_lossy());
                    Some(if windows { as_windows(folded) } else { folded })
                }
                _ => None,
            })
            .filter(|part| !part.is_empty())
            .collect()
    };
    Buffer::judge_name(&read(false)).is_withheld() || Buffer::judge_name(&read(true)).is_withheld()
}

/// Whether a field of `input` acts outside `directory`, or names `.git`, `.claude` or
/// `.mcp.json` below it. Only an absolute path is inside, as Claude Code takes a relative one
/// from its shell's directory, which a `cd` changes; a pattern of names is taken under the
/// directory, and none may hold a `..`.
fn reaches_outside(tool: &str, input: &Value, directory: &Path) -> bool {
    fields(tool, input).into_iter().any(|field| match field {
        Field::Path(Some(path)) => !Path::new(path).is_absolute() || !inside(path, directory),
        Field::Glob(Some(glob)) => glob.contains("..") || !inside(glob, directory),
        Field::Path(None) | Field::Glob(None) => true,
    })
}

fn inside(path: &str, directory: &Path) -> bool {
    if path.starts_with('~') {
        return false;
    }
    let path = directory.join(path);
    if path.components().any(|part| part == Component::ParentDir) {
        return false;
    }
    let Ok(below) = path.strip_prefix(directory) else {
        return false;
    };
    !(below.components()).any(|part| match part {
        Component::Normal(part) => reserved(&part.to_string_lossy()),
        _ => true,
    })
}

/// Whether a part of a path names `.git`, `.claude` or `.mcp.json` as the disk may read it, is a
/// Windows short name, which may be any of them, or is only dots and spaces, which Windows may
/// read as `..`.
fn reserved(part: &str) -> bool {
    let folded = folded(part);
    let dots = !folded.is_empty() && folded.chars().all(|c| c == '.' || c == ' ');
    let short = (folded.split_once('~'))
        .is_some_and(|(_, after)| after.starts_with(|c: char| c.is_ascii_digit()));
    let names = [".git", ".claude", ".mcp.json"];
    dots || short
        || names.contains(&folded.as_str())
        || names.contains(&as_windows(folded.clone()).as_str())
}

/// A part of a path as the disk may read it, in ASCII lower case: what folds to ASCII taken as
/// it, and any other character beyond ASCII dropped.
fn folded(part: &str) -> String {
    let mut name = String::with_capacity(part.len());
    for c in part.chars() {
        match c {
            _ if c.is_ascii() => name.push(c.to_ascii_lowercase()),
            '\u{17f}' => name.push('s'),
            '\u{212a}' => name.push('k'),
            '\u{df}' | '\u{1e9e}' => name.push_str("ss"),
            '\u{131}' | '\u{130}' => name.push('i'),
            '\u{fb00}' => name.push_str("ff"),
            '\u{fb01}' => name.push_str("fi"),
            '\u{fb02}' => name.push_str("fl"),
            '\u{fb03}' => name.push_str("ffi"),
            '\u{fb04}' => name.push_str("ffl"),
            '\u{fb05}' | '\u{fb06}' => name.push_str("st"),
            // Fullwidth ASCII.
            '\u{ff01}'..='\u{ff5e}' => {
                let ascii = char::from_u32(u32::from(c) - 0xff01 + 0x21).unwrap_or('_');
                name.push(ascii.to_ascii_lowercase());
            }
            _ => {}
        }
    }
    name
}

/// A folded part as Windows reads it: a stream after `:`, and trailing dots and spaces, cut.
fn as_windows(mut name: String) -> String {
    if let Some((before, _)) = name.split_once(':') {
        name.truncate(before.len());
    }
    let kept = name.trim_end_matches(['.', ' ']).len();
    name.truncate(kept);
    name
}

/// Whether `command` matches a deny pattern of `rules`, as written or as the shell may join it,
/// with the worker's own `directory` read as `.` where it ends at a separator not followed by
/// `..`, so that its own path, under `.claude` as a worktree's is, is not denied. A worker whose
/// directory is under a `.claude` may name no `..` part, which may climb into it.
fn denied(command: &str, directory: &Path, rules: &Rules) -> bool {
    let written = own_as_dot(command, &directory.display().to_string());
    let [plain, joined, windows] = renderings(&written);
    let under_claude = (directory.components()).any(|part| match part {
        Component::Normal(part) => as_windows(folded(&part.to_string_lossy())) == ".claude",
        _ => false,
    });
    let climbs = || {
        let parts = |text: &String, split: fn(char) -> bool| {
            (text.split(|c: char| c == '/' || split(c))).any(|part| part == "..")
        };
        [&written, &plain, &windows]
            .iter()
            .any(|text| parts(text, is_separator))
            || parts(&joined, is_operator)
    };
    let all = [&written, &plain, &joined, &windows];
    (under_claude && climbs())
        || (rules.deny.iter()).any(|deny| all.iter().any(|text| deny.is_match(text)))
}

/// `command` with `own`, a directory's text, read as `.` where it is the whole of a path's
/// start: at the command's start or after a blank, a quote or an operator, and followed by its
/// end, a separator not before `..`, or what ends a word.
fn own_as_dot(command: &str, own: &str) -> String {
    if own.is_empty() {
        return command.to_owned();
    }
    let own = own.trim_end_matches(['/', '\\']);
    let mut out = String::with_capacity(command.len());
    let mut rest = command;
    while let Some(at) = rest.find(own) {
        let after = &rest[at + own.len()..];
        let starts = (rest[..at].chars().next_back())
            .is_none_or(|c| c.is_whitespace() || "'\"=:;&|()<>`".contains(c));
        let ends = starts
            && match after.chars().next() {
                None => true,
                Some('/' | '\\') => !after[1..].starts_with(".."),
                Some(c) => !(c.is_alphanumeric() || "._-~".contains(c)),
            };
        out.push_str(&rest[..at]);
        out.push_str(if ends { "." } else { own });
        rest = after;
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod bash_tests;
#[cfg(test)]
mod tests;
