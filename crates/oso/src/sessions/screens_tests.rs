//! The bundled rules against screens recorded from Claude Code's TUI (ADR 0121): each must give
//! the state the TUI was in. When a rule changes for a new TUI, its screens are added here.
//!
//! Must not depend on: `app` or `dispatch`; these tests drive the sessions' own
//! parts, with no window and no terminal.
//!
//! The screens were recorded from Claude Code 2.1.322 in a terminal 100 columns by 32 rows, and
//! scrubbed of paths, names, account details and the time of day. The version, the model names
//! and the banners no rule reads are placeholders or cut, and two screens are cut to the rows
//! their rules read. Its title stayed `✳ …` throughout, so the title's spinner is not what
//! these screens are told by.

use super::engine::evaluate;
use super::rules::RuleSet;
use super::screen::Evidence;
use super::state::{Reason, Target};

const IDLE: Target = Target::Idle { visible: false };

/// Each screen, its title, the verdict it must give and the rule that gives it.
const SCREENS: &[(&str, &str, &str, Target, &str)] = &[
    (
        "start-trust",
        include_str!("screens/start-trust.txt"),
        "",
        Target::NeedsInput(Reason::Dialog),
        "confirm-dialog",
    ),
    (
        "idle-start",
        include_str!("screens/idle-start.txt"),
        "✳ Claude Code",
        IDLE,
        "prompt",
    ),
    (
        "slash-menu",
        include_str!("screens/slash-menu.txt"),
        "✳ Claude Code",
        IDLE,
        "prompt",
    ),
    (
        "help",
        include_str!("screens/help.txt"),
        "✳ Claude Code",
        Target::Hold,
        "help",
    ),
    (
        "model-picker",
        include_str!("screens/model-picker.txt"),
        "✳ Claude Code",
        Target::Hold,
        "model-picker",
    ),
    (
        "resume-picker",
        include_str!("screens/resume-picker.txt"),
        "✳ Claude Code",
        Target::Hold,
        "resume-picker",
    ),
    (
        "config",
        include_str!("screens/config.txt"),
        "✳ Claude Code",
        Target::Hold,
        "config",
    ),
    // Recorded with a dialog open behind the viewer, as its footer says: the viewer hides the
    // dialog but not that one waits, so the session needs input, for a dialog it cannot name.
    (
        "transcript-viewer",
        include_str!("screens/transcript-viewer.txt"),
        "✳ Claude Code",
        Target::NeedsInput(Reason::Dialog),
        "transcript-viewer-dialog",
    ),
    (
        "working",
        include_str!("screens/working.txt"),
        "✳ Date to now.txt",
        Target::Working,
        "interrupt-footer",
    ),
    (
        "working-plan-mode",
        include_str!("screens/working-plan-mode.txt"),
        "✳ Date to now.txt",
        Target::Working,
        "interrupt-footer",
    ),
    (
        "permission-bash",
        include_str!("screens/permission-bash.txt"),
        "✳ Date to now.txt",
        Target::NeedsInput(Reason::Permission),
        "permission",
    ),
    (
        "done",
        include_str!("screens/done.txt"),
        "✳ Date to now.txt",
        IDLE,
        "prompt",
    ),
    (
        "question",
        include_str!("screens/question.txt"),
        "✳ Date to now.txt",
        Target::NeedsInput(Reason::Question),
        "question",
    ),
    (
        "question-cancelled",
        include_str!("screens/question-cancelled.txt"),
        "✳ Date to now.txt",
        IDLE,
        "prompt",
    ),
    (
        "plan-approval",
        include_str!("screens/plan-approval.txt"),
        "✳ Date to now.txt",
        Target::NeedsInput(Reason::Plan),
        "plan-approval",
    ),
    (
        "plan-cancelled",
        include_str!("screens/plan-cancelled.txt"),
        "✳ Date to now.txt",
        IDLE,
        "prompt",
    ),
    // A session started with `--name`, whose prompt box's top rule ends with that name, after a
    // message from another session started a turn that ended.
    (
        "message",
        include_str!("screens/message.txt"),
        "✳ Fixture ping",
        IDLE,
        "prompt",
    ),
];

#[test]
fn each_recorded_screen_gives_its_state() {
    let rules = RuleSet::bundled();
    for &(name, text, title, target, rule) in SCREENS {
        let evidence = Evidence::new(text.lines(), title, None);
        let verdict = evaluate(&rules, &evidence);
        assert_eq!(verdict.target, target, "{name}");
        let matched = verdict.by.unwrap_or_else(|why| panic!("{name}: {why:?}"));
        assert_eq!(matched.name, rule, "{name}");
    }
}

#[test]
fn the_working_screen_is_told_by_its_status_line_without_its_footer() {
    let rules = RuleSet::bundled();
    let text = include_str!("screens/working.txt").replace("· esc to interrupt", "");
    let verdict = evaluate(&rules, &Evidence::from_text(&text));
    assert_eq!(verdict.target, Target::Working);
    assert_eq!(verdict.by.unwrap().name, "status-line");
}

#[test]
fn the_title_spinner_and_progress_tell_work_and_its_end() {
    let rules = RuleSet::bundled();
    let spinning = Evidence::new(std::iter::empty(), "⠂ Claude Code", None);
    assert_eq!(evaluate(&rules, &spinning).target, Target::Working);
    let progress = |state, value| {
        let progress = super::screen::Progress::new(state, value);
        evaluate(
            &rules,
            &Evidence::new(std::iter::empty(), "✳ Claude Code", progress),
        )
        .target
    };
    assert_eq!(progress(3, None), Target::Working);
    assert_eq!(progress(1, Some(20)), Target::Working);
    assert_eq!(progress(0, None), Target::Idle { visible: true });
}

#[test]
fn an_mcp_elicitation_needs_input() {
    // Written from the dialog's documented shape, not recorded: no server here asks for input.
    let text = "\
────────────────────────
 MCP server \"tracker\" requests your input
 Project name: _
 ❯ Accept
   Decline
 Esc to cancel
";
    let verdict = evaluate(&RuleSet::bundled(), &Evidence::from_text(text));
    assert_eq!(verdict.target, Target::NeedsInput(Reason::Elicitation));
}

#[test]
fn an_unknown_prompt_is_idle_never_needs_input() {
    let text = "\
────────────────────────
 Do you trust this new widget?
 ❯ 1. Sure
   2. Nope
";
    let verdict = evaluate(&RuleSet::bundled(), &Evidence::from_text(text));
    assert_eq!(verdict.target, Target::Idle { visible: false });
    assert!(verdict.by.is_err());
}

#[test]
fn the_transcript_viewer_with_no_dialog_behind_it_holds() {
    let text = include_str!("screens/transcript-viewer.txt").replace("dialog waiting · ", "");
    let verdict = evaluate(&RuleSet::bundled(), &Evidence::from_text(&text));
    assert_eq!(verdict.target, Target::Hold);
    assert_eq!(verdict.by.unwrap().name, "transcript-viewer");
}

#[test]
fn a_dialog_that_quotes_the_viewers_footer_is_still_the_dialog() {
    // What the agent wrote, in the dialog's last rows, is not the viewer's footer.
    let text = include_str!("screens/permission-bash.txt").replace(
        " Esc to cancel · Tab to amend",
        " Showing detailed transcript · ctrl+o to toggle\n Esc to cancel · Tab to amend",
    );
    let text = text.replace(
        " date > now.txt",
        " echo 'dialog waiting · Showing detailed transcript · ctrl+o'",
    );
    let verdict = evaluate(&RuleSet::bundled(), &Evidence::from_text(&text));
    assert_eq!(verdict.target, Target::NeedsInput(Reason::Permission));
    assert_eq!(verdict.by.unwrap().name, "permission");
}

#[test]
fn dialog_text_with_no_rule_row_above_it_is_no_dialog() {
    // Model output that looks like a dialog, with no prompt box on the screen.
    let text = "\
 Do you want to proceed?
 ❯ 1. Yes
   2. No
 Enter to confirm · Esc to cancel
 Esc to cancel · Tab to amend
";
    let verdict = evaluate(&RuleSet::bundled(), &Evidence::from_text(text));
    assert!(
        !matches!(verdict.target, Target::NeedsInput(_)),
        "{:?}",
        verdict.target
    );
}

#[test]
fn an_unknown_dialog_with_an_esc_footer_is_idle_never_held() {
    // With no prompt box on the screen, so that no dialog rule reads it either.
    let text = "\
 Do you trust this new widget?
 ❯ 1. Sure
   2. Nope
 Enter to confirm · Esc to cancel
";
    let verdict = evaluate(&RuleSet::bundled(), &Evidence::from_text(text));
    assert_eq!(verdict.target, IDLE, "{:?}", verdict.by);
    let reworded = include_str!("screens/question.txt").replace(
        "Enter to select · ↑/↓ to navigate · Esc to cancel",
        "Press enter to pick · Esc to cancel",
    );
    let verdict = evaluate(&RuleSet::bundled(), &Evidence::from_text(&reworded));
    assert!(
        matches!(verdict.target, Target::Idle { .. }),
        "{:?}",
        verdict
    );
}

#[test]
fn every_bundled_needs_input_rule_outranks_every_hold_rule() {
    let rules = RuleSet::bundled();
    let lowest_need = rules
        .rules()
        .iter()
        .filter(|rule| matches!(rule.target, Target::NeedsInput(_)))
        .map(|rule| rule.priority)
        .min()
        .unwrap();
    for rule in rules.rules() {
        if rule.target == Target::Hold {
            assert!(rule.priority < lowest_need, "{} ranks too high", rule.name);
        }
    }
}
