use super::*;

fn term() -> GhosttyEngine {
    GhosttyEngine::new(20, 5, 1000)
}

fn snap(e: &mut GhosttyEngine) -> Snapshot {
    let mut s = Snapshot::default();
    e.snapshot(&mut s);
    s
}

#[test]
fn plain_text_appears() {
    let mut e = term();
    e.feed(b"hello\r\nworld  ");
    let text = e.screen_text();
    assert_eq!(text.len(), 5);
    assert_eq!(text[0], "hello");
    assert_eq!(text[1], "world");
    assert_eq!(text[2], "");
    assert_eq!(e.name(), "libghostty-vt");
}

#[test]
fn sgr_colors_and_flags() {
    let mut e = term();
    e.feed(b"\x1b[31mX\x1b[0m\x1b[1;4;7;38;5;200;48;2;1;2;3mY\x1b[0m");
    let s = snap(&mut e);
    let x = s.row(0)[0];
    assert_eq!(x.ch, 'X');
    assert_eq!(x.fg, Color::Indexed(1));
    assert_eq!(x.bg, Color::Default);
    let y = s.row(0)[1];
    assert_eq!(y.ch, 'Y');
    assert_eq!(y.fg, Color::Indexed(200));
    assert_eq!(y.bg, Color::Rgb(1, 2, 3));
    assert_eq!(y.flags, flags::BOLD | flags::UNDERLINE | flags::INVERSE);
    assert_eq!(
        s.row(0)[2],
        Cell {
            ch: ' ',
            ..Cell::default()
        }
    );
}

#[test]
fn erased_cells_keep_background() {
    let mut e = term();
    e.feed(b"\x1b[44m\x1b[2K");
    let s = snap(&mut e);
    assert_eq!(s.row(0)[5].bg, Color::Indexed(4));
}

#[test]
fn wide_char_sets_wide_and_spacer() {
    let mut e = term();
    e.feed("日x".as_bytes());
    let s = snap(&mut e);
    assert_eq!(s.row(0)[0].ch, '日');
    assert_eq!(s.row(0)[0].flags & flags::WIDE, flags::WIDE);
    assert_eq!(s.row(0)[1].flags & flags::WIDE_SPACER, flags::WIDE_SPACER);
    assert_eq!(s.row(0)[2].ch, 'x');
    assert_eq!(e.screen_text()[0], "日x");
}

#[test]
fn combining_mark_is_grapheme() {
    let mut e = term();
    e.feed("ae\u{301}b".as_bytes());
    let s = snap(&mut e);
    assert_eq!(s.row(0)[1].ch, 'e');
    assert_eq!(s.row(0)[1].flags & flags::GRAPHEME, flags::GRAPHEME);
    assert_eq!(s.graphemes, vec![(1, "e\u{301}".to_string())]);
    assert_eq!(s.row(0)[2].ch, 'b');
    assert_eq!(e.screen_text()[0], "ae\u{301}b");
}

#[test]
fn dirty_rows_only() {
    let mut e = term();
    e.feed(b"a\r\nb\r\nc");
    let mut s = Snapshot::default();
    e.snapshot(&mut s);
    assert!(s.dirty.iter().all(|&d| d));
    e.snapshot(&mut s);
    assert!(s.dirty.iter().all(|&d| !d), "{:?}", s.dirty);
    e.feed(b"\x1b[2;1Hz");
    // screen_text in between must not swallow the damage.
    assert_eq!(e.screen_text()[1], "z");
    e.snapshot(&mut s);
    assert_eq!(s.row(1)[0].ch, 'z');
    assert!(s.dirty[1]);
    assert!(!s.dirty[0] && !s.dirty[4], "{:?}", s.dirty);
    assert_eq!((s.cursor_row, s.cursor_col, s.cursor_visible), (1, 1, true));
}

#[test]
fn grapheme_entries_follow_rebuilt_rows() {
    let mut e = term();
    e.feed("e\u{301}\r\nx".as_bytes());
    let mut s = Snapshot::default();
    e.snapshot(&mut s);
    assert_eq!(s.graphemes.len(), 1);
    e.feed(b"\x1b[1;1Hq");
    e.snapshot(&mut s);
    assert!(s.graphemes.is_empty());
    assert_eq!(s.row(0)[0].ch, 'q');
}

#[test]
fn hyperlink_sets_link() {
    let mut e = term();
    e.feed(b"\x1b]8;;https://example.com\x1b\\L\x1b]8;;\x1b\\N");
    let s = snap(&mut e);
    assert_eq!(s.row(0)[0].flags & flags::LINK, flags::LINK);
    assert_eq!(s.row(0)[1].flags & flags::LINK, 0);
}

#[test]
fn title() {
    let mut e = term();
    e.feed(b"\x1b]2;my title\x07");
    assert_eq!(e.drain_effects(), vec![Effect::Title("my title".into())]);
    assert!(e.drain_effects().is_empty());
}

#[test]
fn title_report_is_off() {
    let mut e = term();
    e.feed(b"\x1b]2;rm -rf\x07\x1b[21t");
    assert_eq!(e.drain_effects(), vec![Effect::Title("rm -rf".into())]);
}

#[test]
fn working_directory() {
    let mut e = term();
    e.feed(b"\x1b]7;file://host/tmp/x\x1b\\");
    assert_eq!(
        e.drain_effects(),
        vec![Effect::WorkingDirectory("file://host/tmp/x".into())]
    );
}

#[test]
fn prompt_marks() {
    let mut e = term();
    e.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07ls\r\n\x1b]133;C\x07out\r\n\x1b]133;D;0\x07");
    assert_eq!(
        e.drain_effects(),
        vec![
            Effect::PromptMark {
                kind: 'A',
                params: String::new()
            },
            Effect::PromptMark {
                kind: 'B',
                params: String::new()
            },
            Effect::PromptMark {
                kind: 'C',
                params: String::new()
            },
            Effect::PromptMark {
                kind: 'D',
                params: "0".into()
            },
        ]
    );
}

#[test]
fn progress() {
    let mut e = term();
    e.feed(b"\x1b]9;4;1;50\x07\x1b]9;4;3\x07");
    assert_eq!(
        e.drain_effects(),
        vec![
            Effect::Progress {
                state: 1,
                percent: Some(50)
            },
            Effect::Progress {
                state: 3,
                percent: None
            }
        ]
    );
}

#[test]
fn notifications() {
    let mut e = term();
    e.feed(b"\x1b]777;notify;T;B\x07\x1b]9;just a body\x07");
    assert_eq!(
        e.drain_effects(),
        vec![
            Effect::Notification {
                title: "T".into(),
                body: "B".into()
            },
            Effect::Notification {
                title: String::new(),
                body: "just a body".into()
            },
        ]
    );
}

#[test]
fn bell() {
    let mut e = term();
    e.feed(b"\x07");
    assert_eq!(e.drain_effects(), vec![Effect::Bell]);
}

#[test]
fn cursor_position_report() {
    let mut e = term();
    e.feed(b"\x1b[6n");
    assert_eq!(
        e.drain_effects(),
        vec![Effect::Reply(b"\x1b[1;1R".to_vec())]
    );
}

#[test]
fn device_attributes_like_alacritty() {
    let mut e = term();
    e.feed(b"\x1b[c");
    assert_eq!(e.drain_effects(), vec![Effect::Reply(b"\x1b[?6c".to_vec())]);
}

#[test]
fn text_area_size_report() {
    let mut e = term();
    e.feed(b"\x1b[18t");
    assert_eq!(
        e.drain_effects(),
        vec![Effect::Reply(b"\x1b[8;5;20t".to_vec())]
    );
}

#[test]
fn unknown_osc_and_apc() {
    let mut e = term();
    e.feed(b"\x1b]1234;hi\x07\x1b_Zhello\x1b\\");
    assert_eq!(
        e.drain_effects(),
        vec![
            Effect::UnknownOsc(b"1234;hi".to_vec()),
            Effect::Apc(b"Zhello".to_vec())
        ]
    );
}

#[test]
fn unknown_osc_is_capped() {
    let mut e = term();
    let mut seq = b"\x1b]4321;".to_vec();
    seq.extend(std::iter::repeat_n(b'a', 10_000));
    seq.push(0x07);
    e.feed(&seq);
    match e.drain_effects().as_slice() {
        [Effect::UnknownOsc(b)] => assert_eq!(b.len(), UNKNOWN_MAX_BYTES),
        other => panic!("{other:?}"),
    }
}

#[test]
fn clipboard_write_is_reported_and_not_answered() {
    let mut e = term();
    e.feed(b"\x1b]52;c;aGVsbG8=\x07");
    assert_eq!(
        e.drain_effects(),
        vec![Effect::ClipboardWrite(b"hello".to_vec())]
    );
}

#[test]
fn clipboard_read_is_denied() {
    let mut e = term();
    e.feed(b"\x1b]52;c;?\x07");
    let effects = e.drain_effects();
    assert_eq!(effects.first(), Some(&Effect::ClipboardRead));
    // After a denial the library answers OSC 52 with an empty clipboard: no data.
    assert_eq!(&effects[1..], &[Effect::Reply(b"\x1b]52;c;\x07".to_vec())]);
}

/// The library answers color queries itself and has no callback for them, so
/// `Effect::ColorQuery` never appears: OSC 10/11 get no reply while no default
/// foreground/background is configured, and OSC 4 is answered from the built-in palette.
#[test]
fn color_queries_are_answered_by_the_library() {
    let mut e = term();
    e.feed(b"\x1b]10;?\x07\x1b]11;?\x07\x1b]4;1;?\x07");
    assert_eq!(
        e.drain_effects(),
        vec![Effect::Reply(b"\x1b]4;1;rgb:cccc/6666/6666\x07".to_vec())]
    );
}

#[test]
fn modes() {
    let mut e = term();
    let m = e.modes();
    assert!(!m.alt_screen && m.cursor_visible && m.mouse_tracking == 0);
    e.feed(b"\x1b[?1049h\x1b[?1h\x1b[?2004h\x1b[?1000h\x1b[?1002h\x1b[?1006h\x1b[?1004h\x1b[?25l\x1b[>5u");
    let m = e.modes();
    assert_eq!(
        m,
        Modes {
            alt_screen: true,
            app_cursor: true,
            bracketed_paste: true,
            mouse_tracking: 1002,
            sgr_mouse: true,
            focus_events: true,
            cursor_visible: false,
            kitty_keyboard_flags: 5,
        }
    );
    e.feed(b"\x1b[?1049l");
    assert!(!e.modes().alt_screen);
}

#[test]
fn scrollback() {
    let mut e = GhosttyEngine::new(20, 10, 1000);
    for i in 0..100 {
        e.feed(format!("line {i}\r\n").as_bytes());
    }
    // 100 newlines: the cursor ends on an empty row, so 91 lines left the screen.
    assert_eq!(e.history_len(), 91);
    let text = e.screen_text();
    assert_eq!(text[0], "line 91");
    assert_eq!(text[9], "");

    let mut e = GhosttyEngine::new(20, 10, 1000);
    let lines: Vec<String> = (0..100).map(|i| format!("line {i}")).collect();
    e.feed(lines.join("\r\n").as_bytes());
    assert_eq!(e.history_len(), 90);
}

#[test]
fn resize_rebuilds_snapshot() {
    let mut e = term();
    e.feed(b"abc");
    let mut s = Snapshot::default();
    e.snapshot(&mut s);
    e.resize(30, 8);
    e.snapshot(&mut s);
    assert_eq!((s.cols, s.rows), (30, 8));
    assert_eq!(s.cells.len(), 240);
    assert_eq!(s.row(0)[2].ch, 'c');
    assert!(s.dirty.iter().all(|&d| d));
    assert_eq!(e.screen_text().len(), 8);
}

#[test]
fn memory_is_reported() {
    let mut e = term();
    let m = e.memory();
    assert!(m.grid_bytes.unwrap() > 0);
}

#[test]
fn version() {
    let v = version_info();
    assert!(
        v.starts_with("libghostty-vt 0.1.0-dev (source 1.4.0-main+83fb956, "),
        "{v}"
    );
    assert!(
        v.contains("file medium false") && v.contains("shm medium false"),
        "{v}"
    );
}

#[test]
fn manifest_decoding_matches_the_library() {
    let mut e = GhosttyEngine::new(40, 4, 100);
    e.feed("plain \x1b[1;31mred\x1b[0m 日本 e\u{301} \x1b]8;;x\x1b\\link\x1b]8;;\x1b\\".as_bytes());
    e.feed(b"\r\n\x1b[48;5;33m\x1b[K\r\n\x1b[48;2;9;8;7m\x1b[K\x1b[0m");
    e.update_render();
    e.start_rows();
    let mut checked = 0;
    while e.next_row() {
        for &raw in e.row_raw_cells() {
            let (a, b) = (decode_raw(e.layout.as_ref(), raw), decode_raw_ffi(raw));
            assert_eq!(
                (
                    a.codepoint,
                    a.tag,
                    a.wide,
                    a.has_styling,
                    a.style_id,
                    a.has_link
                ),
                (
                    b.codepoint,
                    b.tag,
                    b.wide,
                    b.has_styling,
                    b.style_id,
                    b.has_link
                ),
                "raw {raw:#x}"
            );
            if a.tag == ffi::CELL_CONTENT_BG_COLOR_PALETTE
                || a.tag == ffi::CELL_CONTENT_BG_COLOR_RGB
            {
                let fast = GhosttyEngine::cell_bg(e.layout.as_ref(), raw, a.tag);
                assert!(
                    matches!(fast, Color::Indexed(33) | Color::Rgb(9, 8, 7)),
                    "{fast:?}"
                );
            }
            checked += 1;
        }
    }
    assert_eq!(checked, 160);
    assert!(e.layout.is_some());
}
