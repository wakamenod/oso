//! Commands for the session as a whole: cancelling what is in progress, and
//! exiting, at once or once the terminals are idle (ADR 0034, ADR 0059, ADR 0071, ADR 0133).
//! Each cancels a restart that waits for the native sessions (ADR 0165, part 4b).
//!
//! Must not depend on: winit, the GPU or `bindings`; a command sees only the
//! host-owned state and does not know its keys.

use oso_core::buffer::{BufferId, Mode};
use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::{Editor, GitRequest, Quit};
use oso_core::minibuffer::Third;

/// The session commands.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "keyboard-quit",
        doc: "Cancel what is in progress, one thing at a time.\n\nA pending prefix key or prefix argument goes first, in the key resolver. Then the region is hidden, then the innermost prompt or question closes, and runs nothing, and then isearch takes away a failing part, or else ends where it began. In a git status or its log, it also stops what git runs there. With nothing else to do, it cancels an exit or an `oso-restart` that waits.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: keyboard_quit,
        state: None,
    },
    Command {
        name: "save-buffers-kill-terminal",
        doc: "Exit oso, if nothing is at risk.\n\nWith a modified buffer, a save running, or a program running in a terminal, this asks whether to exit, and says how many. It names the sessions with a turn under way, the terminals with a job running, and the repositories git commits in. `y` exits, unless more is at risk by then, when it asks again, and a save still running then gets to finish. With work under way, `w` waits for it, as `quit-when-idle` does. An `oso-restart` that waits is cancelled.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: save_buffers_kill_terminal,
        state: None,
    },
    Command {
        name: "quit-when-idle",
        doc: "Exit oso once no session has a turn under way, no terminal runs a job, and git commits nowhere.\n\nYou go on working meanwhile. Once every terminal has been idle for 2 seconds, oso exits as `C-x C-c` would, and if more is at risk by then than you agreed to lose, or a review comment of yours is not sent, it stops and says so instead. `C-x C-c` cancels, as it asks again and only `w` waits again, and so does `C-g` where oso has the keys: a terminal sends it to its program. A shell started with no job control, as `sh -c` is, shows no job, and a job in the background is not waited for. An `oso-restart` that waits is cancelled.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: quit_when_idle,
        state: None,
    },
];

/// The most buffers a list in an exit's question names; the rest are counted.
const NAMED: usize = 3;

fn keyboard_quit(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    // Whatever else it does, a person's `C-g` drops a search for cursors (ADR 0091).
    ctx.editor.drop_cursor_search();
    // A review comment's draft that holds text goes to the kill ring, which is said in place
    // of "Quit" (ADR 0122).
    let draft_kept = ctx.editor.draft().is_some() && !ctx.editor.current().1.text().is_empty();
    let quit = ctx.editor.quit();
    if quit != Quit::Nothing {
        ctx.request_redraw();
    }
    if quit == Quit::Prompt && draft_kept {
        ctx.editor.echo("Draft in the kill ring: C-y");
        return Ok(());
    }
    // The search goes on, and its line stays (ADR 0082).
    if quit == Quit::SearchStep {
        return Ok(());
    }
    // A file operation stops after the file in hand, and says how far it got (ADR 0134).
    if quit == Quit::Operation {
        ctx.editor.echo("Stopping after the file in hand…");
        return Ok(());
    }
    // In a git status or its log it stops what git runs in the repository, if anything, which
    // the host says (ADR 0127, step 1b).
    if quit == Quit::Nothing {
        let (window, buffer) = ctx.editor.current();
        if matches!(buffer.mode(), Mode::GitStatus | Mode::GitLog) {
            let buffer = window.buffer();
            ctx.editor.request_git(GitRequest::Stop { buffer });
        }
    }
    // With nothing else to do, it cancels an exit that waits for the terminals (ADR 0133).
    if quit == Quit::Nothing && ctx.editor.quit_when_idle() {
        ctx.editor.set_quit_when_idle(false);
        // What the person agreed to lose for that exit is asked about again next time.
        ctx.editor.reset_exit_guard();
        ctx.editor.echo("Quit when idle cancelled");
        ctx.request_redraw();
        return Ok(());
    }
    // And a restart that waits for the native sessions (ADR 0165, part 4b).
    if quit == Quit::Nothing && ctx.editor.restart_when_idle() {
        ctx.editor.set_restart_when_idle(false);
        ctx.editor.echo("Restart when idle cancelled");
        ctx.request_redraw();
        return Ok(());
    }
    // Then it drops a keyboard macro being recorded (ADR 0113).
    if quit == Quit::Nothing && ctx.editor.macro_recording() {
        ctx.editor
            .macro_cancel("Keyboard macro recording cancelled");
        ctx.request_redraw();
        return Ok(());
    }
    log::debug!("Quit");
    ctx.editor.echo("Quit");
    Ok(())
}

fn save_buffers_kill_terminal(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    // An exit that waited for the terminals asks again, and waits again only on `w`; a
    // restart that waited for the native sessions is cancelled (ADR 0165, part 4b).
    ctx.editor.set_quit_when_idle(false);
    ctx.editor.set_restart_when_idle(false);
    let (turns, jobs) = busy(ctx.editor);
    let runs: Vec<BufferId> = ctx.editor.git_runs().collect();
    // Comments of the person's not sent yet are lost with the editor when their review could
    // not be saved, and a draft always (ADR 0122, step 6), and work and programs in terminals
    // end with it (ADR 0120,
    // ADR 0133), as git's runs do (ADR 0127): one question says all of it.
    let mut lost = Vec::new();
    match super::review::unsent_everywhere(ctx.editor) {
        0 => {}
        1 => lost.push("1 comment of yours will be lost".to_owned()),
        n => lost.push(format!("{n} comments of yours will be lost")),
    }
    match turns.len() {
        0 => {}
        1 => lost.push(format!(
            "a turn is under way in {}",
            names(ctx.editor, &turns)
        )),
        _ => lost.push(format!(
            "turns are under way in {}",
            names(ctx.editor, &turns)
        )),
    }
    match jobs.len() {
        0 => {}
        1 => lost.push(format!("a job runs in {}", names(ctx.editor, &jobs))),
        _ => lost.push(format!("jobs run in {}", names(ctx.editor, &jobs))),
    }
    if !runs.is_empty() {
        lost.push(format!("git runs in {}", names(ctx.editor, &runs)));
    }
    let work = !turns.is_empty() || !jobs.is_empty() || !runs.is_empty();
    let more = if work { " more" } else { "" };
    let others = ctx.editor.running_terminals().count();
    match others.saturating_sub(turns.len() + jobs.len()) {
        0 => {}
        1 if work => lost.push("1 more terminal ends".to_owned()),
        1 => lost.push("1 terminal runs a program".to_owned()),
        n => lost.push(format!("{n}{more} terminals end")),
    }
    let lost = (!lost.is_empty()).then(|| lost.join(", and "));
    let question = match (ctx.editor.confirm_exit(ctx.confirmed()), lost) {
        (Ok(()), None) => {
            ctx.request_exit();
            return Ok(());
        }
        (Ok(()), Some(_)) if ctx.confirmed() => {
            ctx.request_exit();
            return Ok(());
        }
        (Ok(()), Some(lost)) => format!("{lost}; exit anyway?"),
        (Err(refused), None) => format!("{refused}; exit without saving?"),
        (Err(refused), Some(lost)) => format!("{lost}, and {refused}; exit without saving?"),
    };
    if !work {
        return ask(
            ctx,
            question,
            "save-buffers-kill-terminal",
            ["exit", "stay"],
        );
    }
    let third = Third {
        command: "quit-when-idle",
        verb: "wait",
    };
    ctx.editor
        .ask_with_third(
            question,
            "save-buffers-kill-terminal",
            ["exit", "stay"],
            third,
        )
        .map_err(|error| CommandError::new(error.to_string()))?;
    ctx.request_redraw();
    Ok(())
}

fn quit_when_idle(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    // From `M-x`, nothing was agreed to: the exit asks about unsaved text once idle. As `w` to
    // `C-x C-c`'s question, what that question named was.
    if !ctx.confirmed() {
        ctx.editor.reset_exit_guard();
    }
    ctx.editor.set_quit_when_idle(true);
    // The exit replaces a restart that waited (ADR 0165, part 4b).
    ctx.editor.set_restart_when_idle(false);
    let (turns, jobs) = busy(ctx.editor);
    let count = |n: usize, one: &str, many: &str| match n {
        0 => None,
        1 => Some(format!("1 {one}")),
        n => Some(format!("{n} {many}")),
    };
    let runs = ctx.editor.git_runs().count();
    let under_way: Vec<String> = [
        count(turns.len(), "turn", "turns"),
        count(jobs.len(), "job", "jobs"),
        count(runs, "git run", "git runs"),
    ]
    .into_iter()
    .flatten()
    .collect();
    ctx.editor.echo(if under_way.is_empty() {
        "Quitting when idle; C-x C-c n cancels".to_owned()
    } else {
        format!(
            "Quitting when idle: {} under way; C-x C-c n cancels",
            under_way.join(" and ")
        )
    });
    ctx.request_redraw();
    Ok(())
}

/// The busy terminals: hosted sessions with a turn under way, then those with a job running.
fn busy(editor: &Editor) -> (Vec<BufferId>, Vec<BufferId>) {
    editor.busy_terminals().partition(|id| {
        editor
            .buffer(*id)
            .is_some_and(oso_core::buffer::Buffer::hosts_session)
    })
}

/// The names of the buffers `ids`, up to [`NAMED`], and how many more.
pub(crate) fn names(editor: &Editor, ids: &[BufferId]) -> String {
    let named: Vec<&str> = (ids.iter().take(NAMED))
        .filter_map(|id| editor.buffer(*id))
        .map(oso_core::buffer::Buffer::name)
        .collect();
    match ids.len().saturating_sub(NAMED) {
        0 => named.join(", "),
        more => format!("{} and {more} more", named.join(", ")),
    }
}

/// Asks `question`, whose `y` runs `command`, and shows it, with `verbs` saying what `y`
/// and `n` do (ADR 0075).
pub(super) fn ask(
    ctx: &mut Ctx<'_>,
    question: String,
    command: &'static str,
    verbs: [&'static str; 2],
) -> Result<(), CommandError> {
    ctx.editor
        .ask_with(question, command, verbs)
        .map_err(|error| CommandError::new(error.to_string()))?;
    ctx.request_redraw();
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::commands::tests::{editor, run};
    use oso_core::buffer::Replacement;
    use oso_core::command::{Args, Effects};
    use oso_core::minibuffer::Kind;

    #[test]
    fn keyboard_quit_changes_nothing() {
        let mut editor = editor("");
        assert_eq!(
            run(&mut editor, "keyboard-quit", Args::NONE),
            (Ok(()), Effects::default())
        );
    }

    #[test]
    fn save_buffers_kill_terminal_exits() {
        let mut editor = editor("");
        let (_, effects) = run(&mut editor, "save-buffers-kill-terminal", Args::NONE);
        assert!(effects.exit);
        assert!(!effects.redraw);
    }

    #[test]
    fn save_buffers_kill_terminal_asks_once_while_a_terminal_runs_a_program() {
        let mut editor = editor("");
        let terminal = editor.add_terminal_buffer("*terminal*");
        editor.set_running(terminal, true);
        let (result, effects) = run(&mut editor, "save-buffers-kill-terminal", Args::NONE);
        assert_eq!(result, Ok(()));
        assert!(!effects.exit);
        let prompt = editor.prompt().expect("a question");
        assert_eq!(
            prompt.label(),
            "1 terminal runs a program; exit anyway? (y or n)"
        );
        editor.quit();
        let id = editor.window().buffer();
        let edit = Replacement {
            range: 0..0,
            text: "x",
        };
        editor.apply_edits(id, &[edit]).unwrap();
        let (_, effects) = run(&mut editor, "save-buffers-kill-terminal", Args::NONE);
        assert!(!effects.exit);
        assert_eq!(
            editor.prompt().expect("a question").label(),
            "1 terminal runs a program, and 1 modified buffer; exit without saving? (y or n)"
        );
    }

    #[test]
    fn save_buffers_kill_terminal_asks_with_a_modified_buffer() {
        let mut editor = editor("");
        let id = editor.window().buffer();
        let edit = Replacement {
            range: 0..0,
            text: "x",
        };
        editor.apply_edits(id, &[edit]).unwrap();
        let (result, effects) = run(&mut editor, "save-buffers-kill-terminal", Args::NONE);
        assert_eq!(result, Ok(()));
        assert!(!effects.exit);
        let prompt = editor.prompt().expect("a question");
        assert_eq!(
            prompt.label(),
            "1 modified buffer; exit without saving? (y or n)"
        );
        assert_eq!(
            prompt.kind(),
            Kind::Question {
                yes: "save-buffers-kill-terminal",
                verbs: ["exit", "stay"],
                third: None,
            }
        );
    }

    fn echo(editor: &oso_core::editor::Editor) -> String {
        let mut status = oso_core::editor::Status::default();
        editor.status_into(editor.tiled().0.id(), &mut status);
        status.echo
    }

    fn run_confirmed(
        editor: &mut oso_core::editor::Editor,
        name: &str,
    ) -> (Result<(), String>, Effects) {
        let registry = crate::commands::registry();
        let id = registry.lookup(name).unwrap();
        let invocation = oso_core::command::Invocation {
            confirmed: true,
            ..oso_core::command::Invocation::default()
        };
        let mut ctx = oso_core::command::Ctx::invoked(editor, invocation);
        let result = (registry.execute(id, &mut ctx, Args::NONE)).map_err(|e| e.to_string());
        (result, ctx.effects())
    }

    /// An editor with a hosted session whose turn is under way and a shell running a job,
    /// besides an idle shell.
    fn busy_editor() -> (oso_core::editor::Editor, [oso_core::buffer::BufferId; 3]) {
        let mut editor = editor("");
        let session = editor.add_terminal_buffer("*claude: docs*");
        editor.set_hosts_session(session, true);
        let job = editor.add_terminal_buffer("*terminal*");
        let idle = editor.add_terminal_buffer("*terminal*");
        for id in [session, job, idle] {
            editor.set_running(id, true);
        }
        editor.set_session_turn(session, true);
        editor.set_terminal_job(job, true);
        (editor, [session, job, idle])
    }

    #[test]
    fn an_exit_names_the_work_under_way_and_offers_to_wait_for_it() {
        let (mut editor, [_, job, _]) = busy_editor();
        let (result, effects) = run(&mut editor, "save-buffers-kill-terminal", Args::NONE);
        assert_eq!(result, Ok(()));
        assert!(!effects.exit);
        let prompt = editor.prompt().expect("a question");
        assert_eq!(
            prompt.label(),
            "a turn is under way in *claude: docs*, and a job runs in *terminal*, and 1 more \
             terminal ends; exit anyway? (y, n or w)"
        );
        let Kind::Question { third, .. } = prompt.kind() else {
            panic!("not a question");
        };
        assert_eq!(third.map(|third| third.verb), Some("wait"));
        // `w` waits, and says for what.
        run(&mut editor, "answer-third", Args::NONE).0.unwrap();
        assert!(editor.prompt().is_none());
        let Some(oso_core::minibuffer::Outcome::Run(name)) = editor.take_prompt_outcome() else {
            panic!("w runs nothing");
        };
        let (_, effects) = run(&mut editor, name, Args::NONE);
        assert!(!effects.exit);
        assert!(editor.quit_when_idle());
        assert_eq!(
            echo(&editor),
            "Quitting when idle: 1 turn and 1 job under way; C-x C-c n cancels"
        );
        // A job that ends leaves the turn to wait for.
        editor.set_terminal_job(job, false);
        assert_eq!(editor.busy_terminals().count(), 1);
    }

    #[test]
    fn c_g_or_another_exit_stops_the_wait() {
        let (mut editor, _) = busy_editor();
        run(&mut editor, "quit-when-idle", Args::NONE).0.unwrap();
        assert!(editor.quit_when_idle());
        run(&mut editor, "keyboard-quit", Args::NONE).0.unwrap();
        assert!(!editor.quit_when_idle());
        assert_eq!(echo(&editor), "Quit when idle cancelled");
        // `C-x C-c` while it waits asks again, waiting no more unless `w` says so.
        run(&mut editor, "quit-when-idle", Args::NONE).0.unwrap();
        run(&mut editor, "save-buffers-kill-terminal", Args::NONE)
            .0
            .unwrap();
        assert!(!editor.quit_when_idle());
        assert!(editor.prompt().is_some());
    }

    #[test]
    fn c_g_another_exit_or_a_quit_when_idle_stops_a_restart_that_waits() {
        let mut editor = editor("");
        // The host waits for the native sessions (ADR 0165, part 4b).
        editor.set_restart_when_idle(true);
        run(&mut editor, "keyboard-quit", Args::NONE).0.unwrap();
        assert!(!editor.restart_when_idle());
        assert_eq!(echo(&editor), "Restart when idle cancelled");
        // With nothing waiting, `C-g` says only "Quit".
        run(&mut editor, "keyboard-quit", Args::NONE).0.unwrap();
        assert_eq!(echo(&editor), "Quit");
        editor.set_restart_when_idle(true);
        run(&mut editor, "save-buffers-kill-terminal", Args::NONE)
            .0
            .unwrap();
        assert!(!editor.restart_when_idle());
        let (mut editor, _) = busy_editor();
        editor.set_restart_when_idle(true);
        run(&mut editor, "quit-when-idle", Args::NONE).0.unwrap();
        assert!(!editor.restart_when_idle());
        assert!(editor.quit_when_idle());
    }

    #[test]
    fn many_busy_terminals_are_named_up_to_three() {
        let mut editor = editor("");
        for _ in 0..5 {
            let id = editor.add_terminal_buffer("*terminal*");
            editor.set_running(id, true);
            editor.set_terminal_job(id, true);
        }
        run(&mut editor, "save-buffers-kill-terminal", Args::NONE)
            .0
            .unwrap();
        assert_eq!(
            editor.prompt().unwrap().label(),
            "jobs run in *terminal*, *terminal*<2>, *terminal*<3> and 2 more; exit anyway? \
             (y, n or w)"
        );
    }

    #[test]
    fn a_confirmed_exit_goes_whatever_runs() {
        let (mut editor, _) = busy_editor();
        let (_, effects) = run_confirmed(&mut editor, "save-buffers-kill-terminal");
        assert!(effects.exit);
    }

    #[test]
    fn keyboard_quit_closes_the_innermost_prompt_and_runs_nothing() {
        let mut editor = editor("");
        editor.ask("First?", "keyboard-quit").unwrap();
        editor.ask("Second?", "keyboard-quit").unwrap();
        assert_eq!(editor.prompt().unwrap().label(), "[2] Second? (y or n)");
        let (_, effects) = run(&mut editor, "keyboard-quit", Args::NONE);
        assert!(effects.redraw);
        assert_eq!(editor.prompt().unwrap().label(), "First? (y or n)");
        assert!(editor.take_prompt_outcome().is_none());
        run(&mut editor, "keyboard-quit", Args::NONE).0.unwrap();
        assert!(editor.prompt().is_none());
    }
}
