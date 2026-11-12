//! Generated output for the throughput and snapshot benchmarks, deterministic.

/// xorshift64*: small, fast, and enough for test data.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    pub fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }

    pub fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }
}

const WORDS: &[&str] = &[
    "the",
    "terminal",
    "compile",
    "error",
    "warning",
    "fn",
    "let",
    "mut",
    "src/main.rs:42:7",
    "Finished",
    "release",
    "target(s)",
    "in",
    "0.42s",
    "Compiling",
    "oso-core",
    "v0.1.0",
    "test",
    "ok",
    "passed",
];

/// Lines like `cargo` and `ls` print: plain ASCII words, 40 to 120 columns.
pub fn plain(size: usize) -> Vec<u8> {
    let mut rng = Rng(1);
    let mut out = Vec::with_capacity(size + 256);
    while out.len() < size {
        let width = 40 + rng.below(80) as usize;
        let start = out.len();
        while out.len() - start < width {
            out.extend_from_slice(WORDS[rng.below(WORDS.len() as u64) as usize].as_bytes());
            out.push(b' ');
        }
        out.extend_from_slice(b"\r\n");
    }
    out
}

/// The same, with an SGR change every word or two: 16, 256 and true colors, bold, underline.
pub fn colored(size: usize) -> Vec<u8> {
    let mut rng = Rng(2);
    let mut out = Vec::with_capacity(size + 256);
    while out.len() < size {
        let width = 40 + rng.below(80) as usize;
        let start = out.len();
        while out.len() - start < width * 2 {
            match rng.below(5) {
                0 => out.extend_from_slice(format!("\x1b[3{}m", rng.below(8)).as_bytes()),
                1 => out.extend_from_slice(format!("\x1b[38;5;{}m", rng.below(256)).as_bytes()),
                2 => out.extend_from_slice(
                    format!(
                        "\x1b[38;2;{};{};{}m",
                        rng.below(256),
                        rng.below(256),
                        rng.below(256)
                    )
                    .as_bytes(),
                ),
                3 => out.extend_from_slice(b"\x1b[1;4m"),
                _ => out.extend_from_slice(b"\x1b[0m"),
            }
            out.extend_from_slice(WORDS[rng.below(WORDS.len() as u64) as usize].as_bytes());
            out.push(b' ');
        }
        out.extend_from_slice(b"\x1b[0m\r\n");
    }
    out
}

const WIDE: &[&str] = &[
    "日本語",
    "吾輩は猫である",
    "中文",
    "한국어",
    "👍",
    "👨‍👩‍👧‍👦",
    "e\u{301}",
    "ＡＢＣ",
    "ｱｲｳ",
    "🇯🇵",
    " ",
];

/// CJK, emoji, ZWJ sequences and combining marks.
pub fn wide(size: usize) -> Vec<u8> {
    let mut rng = Rng(3);
    let mut out = Vec::with_capacity(size + 256);
    while out.len() < size {
        for _ in 0..(5 + rng.below(20)) {
            out.extend_from_slice(WIDE[rng.below(WIDE.len() as u64) as usize].as_bytes());
        }
        out.extend_from_slice(b"\r\n");
    }
    out
}

/// A full-screen program redrawing: cursor moves, erases, colored text, scroll regions,
/// synchronized updates, on the alternate screen.
pub fn tui(size: usize) -> Vec<u8> {
    let mut rng = Rng(4);
    let mut out = Vec::with_capacity(size + 256);
    out.extend_from_slice(b"\x1b[?1049h");
    while out.len() < size {
        out.extend_from_slice(b"\x1b[?2026h");
        for _ in 0..(10 + rng.below(40)) {
            let row = 1 + rng.below(50);
            let col = 1 + rng.below(150);
            out.extend_from_slice(format!("\x1b[{row};{col}H\x1b[K").as_bytes());
            out.extend_from_slice(
                format!("\x1b[38;5;{};48;5;{}m", rng.below(256), rng.below(256)).as_bytes(),
            );
            out.extend_from_slice(WORDS[rng.below(WORDS.len() as u64) as usize].as_bytes());
            if rng.chance(10) {
                out.extend_from_slice(
                    format!(
                        "\x1b[{};{}r\x1b[S\x1b[r",
                        1 + rng.below(20),
                        21 + rng.below(29)
                    )
                    .as_bytes(),
                );
            }
        }
        out.extend_from_slice(b"\x1b[0m\x1b[?2026l");
    }
    out
}

/// One screen of `rows` lines of `cols` colored cells, from the top left.
pub fn colored_screen(cols: usize, rows: usize) -> Vec<u8> {
    let mut rng = Rng(5);
    let mut out = Vec::new();
    for row in 0..rows {
        out.extend_from_slice(format!("\x1b[{};1H", row + 1).as_bytes());
        let mut col = 0;
        while col < cols {
            out.extend_from_slice(format!("\x1b[38;5;{}m", rng.below(256)).as_bytes());
            let n = (cols - col).min(1 + rng.below(12) as usize);
            for _ in 0..n {
                out.push(b'a' + rng.below(26) as u8);
            }
            col += n;
        }
    }
    out.extend_from_slice(b"\x1b[0m");
    out
}

/// A full line of `cols` ASCII cells and a newline, for filling scrollback.
pub fn line_of(cols: usize) -> Vec<u8> {
    let mut line: Vec<u8> = (0..cols).map(|i| b'a' + (i % 26) as u8).collect();
    line.extend_from_slice(b"\r\n");
    line
}

/// One line of `plain`'s kind: `cargo`-like words, 40 to 120 columns.
pub fn plain_line(rng: &mut Rng) -> Vec<u8> {
    let mut out = Vec::new();
    let width = 40 + rng.below(80) as usize;
    while out.len() < width {
        out.extend_from_slice(WORDS[rng.below(WORDS.len() as u64) as usize].as_bytes());
        out.push(b' ');
    }
    out.extend_from_slice(b"\r\n");
    out
}

/// One line of `colored`'s kind: an SGR change every word.
pub fn colored_line(rng: &mut Rng) -> Vec<u8> {
    let mut out = Vec::new();
    let width = 40 + rng.below(80) as usize;
    let mut visible = 0;
    while visible < width {
        match rng.below(3) {
            0 => out.extend_from_slice(format!("\x1b[3{}m", rng.below(8)).as_bytes()),
            1 => out.extend_from_slice(format!("\x1b[38;5;{}m", rng.below(256)).as_bytes()),
            _ => out.extend_from_slice(b"\x1b[1;4m"),
        }
        let word = WORDS[rng.below(WORDS.len() as u64) as usize];
        out.extend_from_slice(word.as_bytes());
        out.push(b' ');
        visible += word.len() + 1;
    }
    out.extend_from_slice(b"\x1b[0m\r\n");
    out
}
