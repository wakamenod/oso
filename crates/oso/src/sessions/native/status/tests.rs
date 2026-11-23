//! The status line's rules (ADR 0165, part 3a): which model, context, cost and window it says,
//! from recorded lines and made ones, and that whatever the stream says, it reads as itself.
//!
//! Must not depend on: the app or a process.

use std::ops::{Deref, DerefMut};

use proptest::prelude::*;

use super::{StatusLine, display_name, dollars};
use crate::sessions::native::mode::ModeState;
use crate::sessions::native::wire::status_line::{Model, Usage};
use crate::sessions::native::wire::{Event, parse};

const STATUS_2_1_330: &str = include_str!("../fixtures/2.1.330/status.jsonl");

/// A status line with the permission mode it is given (part 3b), as the app holds them.
#[derive(Default)]
struct Line {
    status: StatusLine,
    modes: ModeState,
    /// The effort level noted (part 3c).
    effort: Option<&'static str>,
}

impl Deref for Line {
    type Target = StatusLine;

    fn deref(&self) -> &StatusLine {
        &self.status
    }
}

impl DerefMut for Line {
    fn deref_mut(&mut self) -> &mut StatusLine {
        &mut self.status
    }
}

/// The line after `lines`, as the app folds them: answers' models, usage and mode given apart.
fn folded(lines: &str) -> Line {
    let mut line = Line::default();
    for text in lines.lines() {
        fold(&mut line, parse(text.as_bytes()));
    }
    line
}

fn fold(line: &mut Line, event: Event) {
    match event {
        Event::Answer(answer) => {
            if let Some(mode) = answer.mode {
                line.modes.said(&mode, false);
            }
            if let Some(models) = answer.models {
                line.initialized(models);
            }
            if let Some(usage) = answer.usage {
                line.usage_answered(usage);
            }
        }
        event => {
            line.modes.event(&event, false);
            line.status.event(&event);
        }
    }
}

fn shown(line: &Line) -> String {
    let mut out = String::new();
    line.status.write(line.modes.shown(), line.effort, &mut out);
    out
}

fn model(value: &str, resolved: &str, display: &str) -> Model {
    Model {
        value: value.to_owned(),
        resolved: resolved.to_owned(),
        display: display.to_owned(),
        description: String::new(),
        efforts: None,
    }
}

fn read(line: &mut Line, text: &str) {
    fold(line, parse(text.as_bytes()));
}

fn answer(model: &str, parent: &str, usage: &str) -> String {
    format!(
        r#"{{"type":"assistant","parent_tool_use_id":{parent},"message":{{"id":"m","model":"{model}","content":[],"usage":{usage}}}}}"#
    )
}

#[test]
fn a_recorded_resumed_turn_says_each_part() {
    let line = folded(STATUS_2_1_330);
    // 2 + 1 924 + 16 194 tokens read of a 1 000 000 window; the event's 0.25 of the week.
    assert_eq!(
        shown(&line),
        "Haiku 5.5 · acceptEdits · ctx 2% · $0.00 · 7d 25%"
    );
}

#[test]
fn before_its_turn_a_resumed_session_says_get_usages_cost_and_window() {
    let before: String = STATUS_2_1_330
        .lines()
        .take(2)
        .map(|l| l.to_owned() + "\n")
        .collect();
    // `initialize` says the mode too (part 3b).
    assert_eq!(shown(&folded(&before)), "default · $0.00 · 7d 40%");
    let mut line = Line::default();
    line.usage_answered(Usage {
        five_hour: Some(63.0),
        seven_day: Some(28.0),
        cost_usd: Some(0.0),
    });
    assert_eq!(
        shown(&line),
        "5h 63%",
        "the more used window; no cost of nothing"
    );
}

#[test]
fn the_model_shown_is_the_last_to_answer_on_the_main_thread_else_inits() {
    let mut line = Line::default();
    line.initialized(vec![
        model("default", "claude-opus-5-5", "Default (recommended)"),
        model("opus", "claude-opus-5-5", "Opus 5.5"),
        model("sonnet", "claude-sonnet-5-5", "Sonnet 5.5"),
    ]);
    read(
        &mut line,
        r#"{"type":"system","subtype":"init","model":"claude-opus-5-5"}"#,
    );
    assert_eq!(shown(&line), "Opus 5.5", "an alias's name, not default's");
    let usage = r#"{"input_tokens":1}"#;
    read(&mut line, &answer("claude-sonnet-5-5", "null", usage));
    assert_eq!(shown(&line), "Sonnet 5.5", "plan mode answers with another");
    read(&mut line, &answer("claude-haiku-5-5", "\"call\"", usage));
    read(
        &mut line,
        &answer("<synthetic>", "null", r#"{"input_tokens":0}"#),
    );
    assert_eq!(
        shown(&line),
        "Sonnet 5.5",
        "a subagent's and a local command's say nothing"
    );
    line.effort = Some("high");
    assert_eq!(shown(&line), "Sonnet 5.5 high");
}

#[test]
fn a_model_not_listed_is_its_id_less_claude() {
    assert_eq!(display_name(&[], "claude-fable-5-2"), "fable-5-2");
    assert_eq!(display_name(&[], "custom"), "custom");
    let listed = [model("claude-opus-5", "claude-opus-5", "Opus 5")];
    assert_eq!(display_name(&listed, "claude-opus-5"), "Opus 5");
}

#[test]
fn context_is_the_last_answers_input_over_its_models_window() {
    let mut line = Line::default();
    let usage = r#"{"input_tokens":2,"cache_creation_input_tokens":4735,"cache_read_input_tokens":250000,"output_tokens":9}"#;
    read(&mut line, &answer("claude-haiku-5-5", "null", usage));
    assert_eq!(shown(&line), "haiku-5-5", "no window before a result");
    let result = r#"{"type":"result","modelUsage":{"claude-sonnet-5-5":{"contextWindow":200000},"claude-haiku-5-5":{"contextWindow":1000000}}}"#;
    read(&mut line, result);
    assert_eq!(shown(&line), "haiku-5-5 · ctx 25%");
    let only = r#"{"type":"result","modelUsage":{"other":{"contextWindow":0}}}"#;
    read(&mut line, only);
    assert_eq!(
        shown(&line),
        "haiku-5-5 · ctx 25%",
        "a window of nothing is none"
    );
}

#[test]
fn cost_is_cents_then_whole_dollars() {
    assert_eq!(dollars(0.0036), "$0.00");
    assert_eq!(dollars(0.825), "$0.82");
    assert_eq!(dollars(9.99), "$9.99");
    assert_eq!(dollars(9.996), "$10");
    assert_eq!(dollars(12.4), "$12");
}

#[test]
fn the_window_is_the_last_events_named_one_and_others_are_none() {
    let mut line = Line::default();
    line.usage_answered(Usage {
        five_hour: Some(10.0),
        seven_day: Some(28.0),
        cost_usd: None,
    });
    let event = |kind: &str, used: &str| {
        format!(
            r#"{{"type":"rate_limit_event","rate_limit_info":{{"rateLimitType":"{kind}","unifiedWindows":{{"{kind}":{{"utilization":{used}}}}}}}}}"#
        )
    };
    read(&mut line, &event("five_hour", "0.634"));
    assert_eq!(
        shown(&line),
        "5h 63%",
        "the event's window, over get_usage's"
    );
    read(&mut line, &event("amber_gauge", "0.9"));
    read(&mut line, &event("seven_day", "-1"));
    read(&mut line, &event("seven_day", "1e9"));
    assert_eq!(shown(&line), "5h 63%");
}

#[test]
fn the_mode_follows_init_and_status_and_odd_text_is_not_shown() {
    let mut line = Line::default();
    read(
        &mut line,
        r#"{"type":"system","subtype":"init","permissionMode":"default"}"#,
    );
    assert_eq!(shown(&line), "default");
    read(
        &mut line,
        r#"{"type":"system","subtype":"status","status":null,"permissionMode":"plan"}"#,
    );
    assert_eq!(shown(&line), "plan");
    read(
        &mut line,
        r#"{"type":"system","subtype":"status","status":"requesting"}"#,
    );
    assert_eq!(shown(&line), "plan", "a status with no mode keeps it");
    read(
        &mut line,
        r#"{"type":"system","subtype":"init","model":"claude-haiku-5-5"}"#,
    );
    let odd = "{\"type\":\"system\",\"subtype\":\"init\",\"model\":\"a\\u001b[2Jb\"}";
    read(&mut line, odd);
    assert_eq!(
        shown(&line),
        "plan",
        "a model given that reads as no model is none"
    );
    let odd = "{\"type\":\"system\",\"subtype\":\"status\",\"permissionMode\":\"au\\u202eto\"}";
    read(&mut line, odd);
    assert_eq!(
        shown(&line),
        "",
        "nor is a mode left saying what the CLI no more says"
    );
}

#[test]
fn a_part_that_holds_a_dot_a_space_or_was_cut_is_not_shown() {
    let mode = |mode: &str| {
        serde_json::json!({"type": "system", "subtype": "status", "permissionMode": mode})
            .to_string()
    };
    // Each would pass for more than a model, in a narrow modeline too.
    for model in [
        "claude-x · default · ctx 1%",
        "x  plan",
        "a\u{2022}b",
        "a\u{2e31}b",
        "a\u{1427}b",
        "a\u{30fb}b",
        &"m".repeat(300),
    ] {
        let mut line = Line::default();
        read(&mut line, &mode("acceptEdits"));
        let init = serde_json::json!({"type": "system", "subtype": "init", "model": model});
        read(&mut line, &init.to_string());
        let answer = serde_json::json!({"type": "assistant", "message": {
            "id": "m", "model": model, "content": [],
        }});
        read(&mut line, &answer.to_string());
        assert_eq!(shown(&line), "acceptEdits", "{model:?}");
    }
    for odd in ["plan · x", "plan x", "a\u{b7}b", &"p".repeat(300)] {
        let mut line = Line::default();
        read(&mut line, &mode("acceptEdits"));
        read(&mut line, &mode(odd));
        assert_eq!(shown(&line), "", "{odd:?}");
    }
    let mut line = Line {
        effort: Some("high · auto"),
        ..Line::default()
    };
    read(
        &mut line,
        r#"{"type":"system","subtype":"init","model":"claude-haiku-5-5"}"#,
    );
    assert_eq!(shown(&line), "haiku-5-5", "nor an effort that holds one");
}

#[test]
fn a_cost_or_a_use_of_minus_nothing_is_none() {
    let mut line = Line::default();
    read(&mut line, r#"{"type":"result","total_cost_usd":-0.0}"#);
    let event = r#"{"type":"rate_limit_event","rate_limit_info":{"rateLimitType":"five_hour","unifiedWindows":{"five_hour":{"utilization":-0.0}}}}"#;
    read(&mut line, event);
    assert_eq!(shown(&line), "");
}

#[test]
fn a_listed_model_that_could_pass_for_another_is_left_out() {
    let answer = "{\"type\":\"control_response\",\"response\":{\"subtype\":\"success\",\
        \"request_id\":\"oso-0\",\"response\":{\"models\":[\
        {\"value\":\"opus\",\"resolvedModel\":\"claude-opus-5-5\",\"displayName\":\"Op\\u202eus\"},\
        {\"value\":\"x\",\"resolvedModel\":\"claude-x\",\"displayName\":\"\"},\
        {\"value\":\"y\",\"resolvedModel\":\"claude-y\",\"displayName\":\"Y \\u00b7 plan\"},\
        {\"value\":\"z\",\"resolvedModel\":\"claude-z\",\"displayName\":\"Z  plan\"},\
        {\"value\":\"w w\",\"resolvedModel\":\"claude-w\",\"displayName\":\"W\"},\
        {\"value\":\"sonnet\",\"resolvedModel\":\"claude-sonnet-5-5\",\"displayName\":\"Sonnet 5.5\"}]}}}";
    let Event::Answer(answer) = parse(answer.as_bytes()) else {
        panic!("not an answer");
    };
    let models = answer.models.unwrap();
    assert_eq!(models, [model("sonnet", "claude-sonnet-5-5", "Sonnet 5.5")]);
}

/// A line the status line reads, with odd values.
fn shaped() -> impl Strategy<Value = String> {
    let value = prop_oneof![
        Just("null".to_owned()),
        Just("-1".to_owned()),
        Just("1e308".to_owned()),
        Just("18446744073709551615".to_owned()),
        Just("\"\\u0000\\u202e\\n\"".to_owned()),
        "\"[a-z ]{0,300}\"",
        (0..2_000_000u64).prop_map(|n| n.to_string()),
    ];
    (0..6usize, value.clone(), value).prop_map(|(kind, a, b)| match kind {
        0 => format!(r#"{{"type":"system","subtype":"init","model":{a},"permissionMode":{b}}}"#),
        1 => format!(r#"{{"type":"system","subtype":"status","permissionMode":{a}}}"#),
        2 => format!(
            r#"{{"type":"assistant","message":{{"id":"m","model":{a},"content":[],"usage":{{"input_tokens":{b},"cache_read_input_tokens":{b}}}}}}}"#
        ),
        3 => format!(
            r#"{{"type":"result","total_cost_usd":{a},"modelUsage":{{"m":{{"contextWindow":{b}}}}}}}"#
        ),
        4 => format!(
            r#"{{"type":"rate_limit_event","rate_limit_info":{{"rateLimitType":"five_hour","unifiedWindows":{{"five_hour":{{"utilization":{a}}}}}}}}}"#
        ),
        _ => format!(
            r#"{{"type":"control_response","response":{{"subtype":"success","request_id":"oso-0","response":{{"models":[{{"value":{a},"resolvedModel":{a},"displayName":{b}}}],"rate_limits":{{"seven_day":{{"utilization":{b}}}}},"session":{{"total_cost_usd":{a}}}}}}}}}"#
        ),
    })
}

proptest! {
    #[test]
    fn whatever_the_stream_says_the_line_reads_as_itself_and_is_bounded(
        lines in prop::collection::vec(
            prop_oneof![
                2 => shaped(),
                1 => (0..7usize).prop_map(|at| STATUS_2_1_330.lines().nth(at).unwrap_or("").to_owned()),
            ],
            0..30,
        ),
    ) {
        let mut line = Line::default();
        for text in &lines {
            fold(&mut line, parse(text.as_bytes()));
        }
        let out = shown(&line);
        prop_assert!(!out.chars().any(|c| c.is_control() || ('\u{202a}'..='\u{202e}').contains(&c)), "{out:?}");
        // A model's, its effort's and a mode's names of at most 256 bytes, and short parts.
        prop_assert!(out.len() <= 3 * 256 + 64, "{}", out.len());
    }
}
