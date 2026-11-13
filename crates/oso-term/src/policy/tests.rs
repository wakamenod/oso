//! ADR 0120's escape-sequence table, row by row: each test feeds the sequence to the engine,
//! judges what it raised, and checks the reply, the event, or that nothing happened.
//!
//! Must not depend on: the thread.

use super::{Verdict, judge, text};
use crate::event::{Event, Progress, Prompt};

/// The text-cleaning rules, without an engine.
#[test]
fn text_drops_controls_bidi_and_invisibles_and_is_cut_at_256_bytes() {
    assert_eq!(text(b"a\x07b\x1b[31mc\x7f\xc2\x9bd"), "ab[31mcd");
    assert_eq!(text("x\u{202e}y\u{200b}z\u{2066}".as_bytes()), "xyz");
    assert_eq!(text(b"\xff\xfeok"), "\u{fffd}\u{fffd}ok");
    assert_eq!(
        text("a\u{2028}b\u{2029}c\u{feff}d\u{e0041}e".as_bytes()),
        "abcde"
    );
    let long = "é".repeat(200);
    let cut = text(long.as_bytes());
    assert_eq!(cut.len(), 256);
    assert!(cut.chars().all(|c| c == 'é'));
}

#[cfg(unix)]
mod table {
    use super::*;
    use crate::engine::Engine;
    use crate::engine::ghostty::Ghostty;
    use crate::engine::ghostty::tests::{engine, size, snapshot};

    /// What feeding `bytes` to a fresh 80 × 24 terminal comes to.
    fn feed(bytes: &[u8]) -> (Vec<u8>, Vec<Event>) {
        feed_to(&mut engine(80, 24), bytes)
    }

    fn feed_to(e: &mut Ghostty, bytes: &[u8]) -> (Vec<u8>, Vec<Event>) {
        e.feed(bytes);
        let mut effects = Vec::new();
        e.take_effects(&mut effects);
        let mut replies = Vec::new();
        let mut events = Vec::new();
        for effect in effects {
            match judge(effect) {
                Verdict::Reply(bytes) => replies.extend(bytes),
                Verdict::Event(event) => events.push(event),
                Verdict::Drop => {}
            }
        }
        (replies, events)
    }

    fn reply(bytes: &[u8]) -> String {
        String::from_utf8_lossy(&feed(bytes).0).into_owned()
    }

    fn nothing(bytes: &[u8]) {
        let (replies, events) = feed(bytes);
        assert!(
            replies.is_empty(),
            "{bytes:?} replied {:?}",
            String::from_utf8_lossy(&replies)
        );
        assert!(events.is_empty(), "{bytes:?} raised {events:?}");
    }

    #[test]
    fn osc_52_reads_are_denied() {
        nothing(b"\x1b]52;c;?\x07");
        nothing(b"\x1b]52;p;?\x1b\\");
    }

    #[test]
    fn osc_52_writes_reach_the_kill_ring() {
        let (replies, events) = feed(b"\x1b]52;c;aGVsbG8=\x07");
        assert!(replies.is_empty());
        assert!(
            matches!(&events[..], [Event::ClipboardWrite(t)] if t == "hello"),
            "{events:?}"
        );
    }

    #[test]
    fn osc_52_writes_over_one_mib_are_dropped() {
        let mut big = b"\x1b]52;c;".to_vec();
        big.extend(std::iter::repeat_n(b'Q', (1 << 20) / 3 * 4 + 8));
        big.push(0x07);
        nothing(&big);
    }

    /// Kitty's clipboard protocol: a write opens, its text/plain data follows (the MIME
    /// type in base64), and data without a type commits it.
    fn osc_5522_write(id: &str, data: &[u8]) -> Vec<u8> {
        let mut bytes = format!("\x1b]5522;type=write{id}\x1b\\").into_bytes();
        bytes.extend(b"\x1b]5522;type=wdata:mime=dGV4dC9wbGFpbg==;");
        bytes.extend(data);
        bytes.extend(b"\x1b\\\x1b]5522;type=wdata\x1b\\");
        bytes
    }

    #[test]
    fn osc_5522_writes_reach_the_kill_ring_and_get_no_reply() {
        for id in ["", ":id=x-y"] {
            let (replies, events) = feed(&osc_5522_write(id, b"aGVsbG8="));
            assert!(
                replies.is_empty(),
                "{id} replied {:?}",
                String::from_utf8_lossy(&replies)
            );
            assert!(
                matches!(&events[..], [Event::ClipboardWrite(t)] if t == "hello"),
                "{id}: {events:?}"
            );
        }
    }

    #[test]
    fn osc_5522_writes_over_one_mib_are_dropped() {
        let data = vec![b'Q'; (1 << 20) / 3 * 4 + 8];
        nothing(&osc_5522_write("", &data));
    }

    #[test]
    fn osc_1337_copy_reaches_the_kill_ring_and_gets_no_reply() {
        for end in [&b"\x07"[..], b"\x1b\\"] {
            let mut bytes = b"\x1b]1337;Copy=:aGVsbG8=".to_vec();
            bytes.extend(end);
            let (replies, events) = feed(&bytes);
            assert!(
                replies.is_empty(),
                "replied {:?}",
                String::from_utf8_lossy(&replies)
            );
            assert!(
                matches!(&events[..], [Event::ClipboardWrite(t)] if t == "hello"),
                "{events:?}"
            );
        }
    }

    #[test]
    fn titles_are_shown_cleaned_and_cut() {
        let (_, events) = feed(b"\x1b]2;make \xe2\x80\xae build\x07");
        assert!(
            matches!(&events[..], [Event::Title(t)] if t == "make  build"),
            "{events:?}"
        );
        let long = format!("\x1b]0;{}\x07", "x".repeat(1000));
        let (_, events) = feed(long.as_bytes());
        assert!(
            matches!(&events[..], [Event::Title(t)] if t.len() == 256),
            "{events:?}"
        );
    }

    #[test]
    fn the_title_report_the_font_query_and_decrqss_are_denied() {
        let (replies, _) = feed(b"\x1b]2;secret\x07\x1b[21t");
        assert!(replies.is_empty(), "{replies:?}");
        nothing(b"\x1b]50;?\x07");
        nothing(b"\x1bP$qm\x1b\\");
        nothing(b"\x1bP$q r\x1b\\");
        nothing(b"\x1bP$qr\x1b\\");
    }

    /// Allowed, but libghostty-vt 1.0.1 keeps no title stack: pushing and popping change
    /// nothing, and answer nothing.
    #[test]
    fn the_title_stack_is_allowed() {
        let (replies, events) = feed(b"\x1b]2;first\x07\x1b[22;2t\x1b]2;second\x07\x1b[23;2t");
        assert!(replies.is_empty());
        let titles: Vec<&str> = events
            .iter()
            .filter_map(|e| match e {
                Event::Title(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(titles, ["first", "second"]);
    }

    /// OSC 8 links: the engine keeps the target with the cells, for the view's step; nothing
    /// is answered or raised, and the text reaches the grid.
    #[test]
    fn hyperlinks_are_kept_with_their_text_and_answer_nothing() {
        let mut e = engine(20, 2);
        let (replies, events) = feed_to(
            &mut e,
            b"\x1b]8;id=a;https://example.com/\x1b\\link\x1b]8;;\x1b\\ \x1b]8;;file:///tmp\x07f\x1b]8;;\x07",
        );
        assert!(replies.is_empty() && events.is_empty(), "{events:?}");
        let (s, _) = snapshot(&mut e);
        let text: String = s.rows[0].cells.iter().map(|c| c.ch).collect();
        assert!(text.starts_with("link f"), "{text:?}");
    }

    /// The table's "Allowed" modes: each is set, and a mode report says so. Synchronized
    /// updates (2026) are reported set; holding the drawing for them is the view's (step 4).
    #[test]
    fn the_allowed_modes_are_set_and_reported() {
        for mode in [2004, 1004, 2026, 1049, 1000, 1002, 1003, 1006] {
            let mut e = engine(80, 24);
            let set = format!("\x1b[?{mode}h\x1b[?{mode}$p");
            let (replies, events) = feed_to(&mut e, set.as_bytes());
            assert!(events.is_empty(), "{mode}: {events:?}");
            assert_eq!(
                String::from_utf8_lossy(&replies),
                format!("\x1b[?{mode};1$y"),
                "{mode}"
            );
        }
        // A cursor shape, undercurl and an underline color answer nothing.
        nothing(b"\x1b[4 q\x1b[4:3m\x1b[58;2;1;2;3m");
    }

    #[test]
    fn color_queries_are_answered_from_the_theme() {
        assert_eq!(reply(b"\x1b]10;?\x07"), "\x1b]10;rgb:dddd/dddd/dddd\x07");
        assert_eq!(
            reply(b"\x1b]11;?\x1b\\"),
            "\x1b]11;rgb:1111/1111/1111\x1b\\"
        );
        assert_eq!(reply(b"\x1b]4;1;?\x07"), "\x1b]4;1;rgb:0a0a/0101/0202\x07");
        assert!(reply(b"\x1b]12;?\x07").starts_with("\x1b]12;rgb:ffff/0000/0000"));
    }

    #[test]
    fn color_changes_apply_to_that_terminal() {
        let mut e = engine(80, 24);
        let (replies, _) = feed_to(&mut e, b"\x1b]4;1;rgb:12/34/56\x07\x1b]4;1;?\x07");
        assert_eq!(
            String::from_utf8_lossy(&replies),
            "\x1b]4;1;rgb:1212/3434/5656\x07"
        );
        assert_eq!(reply(b"\x1b]4;1;?\x07"), "\x1b]4;1;rgb:0a0a/0101/0202\x07");
    }

    #[test]
    fn reports_of_the_terminals_own_state_are_answered() {
        assert!(reply(b"\x1b[c").starts_with("\x1b[?"), "DA1");
        assert!(reply(b"\x1b[>c").starts_with("\x1b[>"), "DA2");
        assert_eq!(reply(b"\x1b[5n"), "\x1b[0n");
        assert_eq!(reply(b"\x1b[3;5H\x1b[6n"), "\x1b[3;5R");
        assert_eq!(reply(b"\x1b[?2004h\x1b[?2004$p"), "\x1b[?2004;1$y");
        assert_eq!(reply(b"\x1b[?u"), "\x1b[?0u");
        assert_eq!(reply(b"\x1b[18t"), "\x1b[8;24;80t");
        assert_eq!(reply(b"\x1b[14t"), "\x1b[4;384;640t");
        assert!(reply(b"\x1b[>q").contains("oso "), "XTVERSION");
    }

    #[test]
    fn window_moves_resizes_and_iconify_are_denied() {
        for op in 1..=9 {
            nothing(format!("\x1b[{op};10;20t").as_bytes());
        }
    }

    #[test]
    fn enquiry_and_the_checksum_report_are_denied() {
        nothing(b"\x05");
        nothing(b"\x1b[1;1;1;1;24;80*y");
    }

    #[test]
    fn the_working_directory_is_reported() {
        let (_, events) = feed(b"\x1b]7;file://host/tmp/a%20b\x07");
        assert!(
            matches!(&events[..], [Event::Directory(d)] if d == "file://host/tmp/a%20b"),
            "{events:?}"
        );
    }

    #[test]
    fn prompt_marks_are_reported() {
        let (_, events) =
            feed(b"\x1b]133;A\x07$ \x1b]133;B\x07\x1b]133;C;cmdline=ls\x07\x1b]133;D;2\x07");
        let marks: Vec<&Prompt> = events
            .iter()
            .filter_map(|e| match e {
                Event::Prompt(p) => Some(p),
                _ => None,
            })
            .collect();
        assert_eq!(marks.len(), 4, "{events:?}");
        assert_eq!(marks[0], &Prompt::Start);
        assert_eq!(marks[1], &Prompt::Input);
        assert!(matches!(marks[2], Prompt::Output { .. }));
        assert_eq!(marks[3], &Prompt::End { exit: Some(2) });
    }

    #[test]
    fn notifications_and_progress_are_reported() {
        let (_, events) = feed(b"\x1b]9;build done\x07\x1b]777;notify;Title;Body\x07");
        assert!(
            matches!(&events[..], [Event::Notification { body, .. }, Event::Notification { title, .. }]
                if body == "build done" && title == "Title"),
            "{events:?}"
        );
        let (_, events) = feed(b"\x1b]9;4;1;50\x07\x1b]9;4;3\x07\x1b]9;4;0\x07");
        let progress: Vec<Progress> = events
            .iter()
            .filter_map(|e| match e {
                Event::Progress(p) => Some(*p),
                _ => None,
            })
            .collect();
        assert_eq!(
            progress,
            [Progress::Set(50), Progress::Indeterminate, Progress::Remove]
        );
    }

    #[test]
    fn the_bell_is_reported() {
        let (_, events) = feed(b"\x07");
        assert!(matches!(&events[..], [Event::Bell]), "{events:?}");
    }

    #[test]
    fn unknown_sequences_osc_51_and_iterm2_images_are_denied() {
        nothing(b"\x1b]51;Eeval \"(kill-emacs)\"\x07");
        nothing(b"\x1b]1337;File=inline=1:AAAA\x07");
        nothing(b"\x1b]5379;run;abc;cmd\x07");
        nothing(b"\x1b_Xsomething\x1b\\");
    }

    #[test]
    fn sixel_is_denied() {
        let mut e = engine(20, 4);
        let (replies, events) = feed_to(&mut e, b"\x1bPq#0;2;0;0;0~~@@\x1b\\");
        assert!(replies.is_empty() && events.is_empty());
    }

    #[test]
    fn kitty_graphics_are_off_until_step_12() {
        let (replies, events) = feed(b"\x1b_Gi=1,a=q,f=24,s=1,v=1;AAAA\x1b\\");
        assert!(events.is_empty());
        assert!(
            !String::from_utf8_lossy(&replies).contains(";OK"),
            "{replies:?}"
        );
        let mut e = Ghostty::new(size(10, 3), 100, crate::policy::XTVERSION).expect("an engine");
        let (_, events) = feed_to(&mut e, b"\x1b_Ga=T,f=24,s=1,v=1;AAAA\x1b\\");
        assert!(events.is_empty());
    }

    /// DECRQSS is answered by the library, and dropped by the policy.
    #[test]
    fn decrqss_is_answered_by_the_library_and_dropped() {
        let mut e = engine(80, 24);
        e.feed(b"\x1bP$qm\x1b\\");
        let mut effects = Vec::new();
        e.take_effects(&mut effects);
        let raised = effects.iter().any(|effect| {
            matches!(effect, crate::engine::Effect::Reply(r) if r.starts_with(b"\x1bP1$r"))
        });
        assert!(raised, "{effects:?}");
        nothing(b"\x1bP$qm\x1b\\");
    }

    #[test]
    fn replies_that_would_carry_the_programs_bytes_are_denied() {
        nothing(b"\x1b]5522;type=read:mime=dGV4dC9wbGFpbg==:id=a+b\x1b\\");
        let (replies, _) = feed(b"\x1b]5522;type=write:mime=dGV4dC9wbGFpbg==:id=x-y;aGk=\x1b\\");
        assert!(
            replies.is_empty(),
            "{:?}",
            String::from_utf8_lossy(&replies)
        );
        let (replies, _) = feed(b"\x1bP+q544e\x1b\\\x1bP+q636f6c6f7273\x1b\\");
        assert!(
            replies.is_empty(),
            "XTGETTCAP {:?}",
            String::from_utf8_lossy(&replies)
        );
        let (replies, _) = feed(b"\x1b]72;type=list\x1b\\");
        assert!(
            replies.is_empty(),
            "OSC 72 {:?}",
            String::from_utf8_lossy(&replies)
        );
        let (replies, _) = feed(b"\x1b]1337;Copy=;aGk=\x07");
        assert!(
            replies.is_empty(),
            "OSC 1337 {:?}",
            String::from_utf8_lossy(&replies)
        );
    }

    #[test]
    fn in_band_size_reports_are_answered() {
        // Mode 2048: the size now, then after each resize.
        let mut e = engine(80, 24);
        let (replies, _) = feed_to(&mut e, b"\x1b[?2048h");
        let replies = String::from_utf8_lossy(&replies).into_owned();
        assert!(replies.starts_with("\x1b[48;24;80;"), "{replies:?}");
        e.resize(size(100, 30));
        let (replies, _) = feed_to(&mut e, b"");
        let replies = String::from_utf8_lossy(&replies).into_owned();
        assert!(replies.starts_with("\x1b[48;30;100;"), "{replies:?}");
    }

    #[test]
    fn the_reply_allowlist_takes_only_the_tables_shapes() {
        for good in [
            &b"\x1b[?62;22c"[..],
            b"\x1b[>1;10;0c",
            b"\x1b[0n",
            b"\x1b[3;5R",
            b"\x1b[?2004;1$y",
            b"\x1b[?0u",
            b"\x1b[8;24;80t",
            b"\x1b[48;24;80;384;640t",
            b"\x1bP>|oso 1\x1b\\",
            b"\x1bP!|00000000\x1b\\",
            b"\x1b]4;1;rgb:0a0a/0101/0202\x07",
            b"\x1b]11;rgb:1111/1111/1111\x1b\\",
        ] {
            assert!(super::super::allowed_reply(good), "{good:?}");
        }
        for bad in [
            &b"\x1bP1$rm\x1b\\"[..],
            b"\x1bP1+r544e=787465726d\x1b\\",
            b"\x1b]5522;type=write:status=DONE:id=x\x1b\\",
            b"\x1b[2;3t",
            b"\x1b[?62;22cX",
            b"\x1bP>|\x1b[31m\x1b\\",
            b"\x1b]4;1;rgb:zz/00/00\x07",
            b"\x1b]2;title\x07",
        ] {
            assert!(!super::super::allowed_reply(bad), "{bad:?}");
        }
    }

    #[test]
    fn kitty_mediums_and_the_image_store_are_off() {
        let (_, store, file, shm) = engine(10, 3).memory_settings();
        assert_eq!((store, file, shm), (0, false, false));
    }

    #[test]
    fn a_directory_also_comes_as_a_plain_path() {
        for (bytes, path) in [
            (&b"\x1b]9;9;/tmp/nine\x07"[..], "/tmp/nine"),
            (b"\x1b]1337;CurrentDir=/tmp/iterm\x07", "/tmp/iterm"),
        ] {
            let (replies, events) = feed(bytes);
            assert!(replies.is_empty());
            assert!(
                matches!(&events[..], [Event::Directory(d)] if d == path),
                "{bytes:?}: {events:?}"
            );
        }
    }

    #[test]
    fn a_directory_is_kept_as_sent_or_dropped_whole() {
        let (_, events) = feed("\x1b]7;file://h/tmp/a\u{200b}b\x07".as_bytes());
        assert!(
            matches!(&events[..], [Event::Directory(d)] if d == "file://h/tmp/a\u{200b}b"),
            "{events:?}"
        );
        let long = format!("\x1b]7;file://h/{}\x07", "d".repeat(5000));
        let (_, events) = feed(long.as_bytes());
        assert!(
            events.iter().all(|e| !matches!(e, Event::Directory(_))),
            "{events:?}"
        );
    }
}
