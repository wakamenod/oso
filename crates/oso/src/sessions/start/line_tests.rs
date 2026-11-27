//! The line a session starts with, as [`launch_of`] builds it: Remote Control is off in every
//! native session, and only there (ADR 0165).
//!
//! Must not depend on: `app`.

use std::path::PathBuf;

use oso_platform::process::Executable;
use serde_json::Value;

use super::tests::terminal_order;
use super::{Chosen, Extra, Kind, NativeOrder, Order, Runs, connect, launch_of, space_gate, trust};
use crate::sessions::identity::SessionId;
use crate::terminal::shell::Settings;

fn id(n: u8) -> SessionId {
    SessionId::from_random([n; 16])
}

/// Every kind of start: a new session, a resume (an interrupt's, a restart's and a switch from
/// the TUI among them) and a fork.
fn kinds() -> [Kind; 3] {
    [Kind::New, Kind::Resume(id(2)), Kind::Fork(id(3))]
}

fn native() -> Runs {
    native_with(connect::Leave::Blocked)
}

pub(super) fn native_with(remote: connect::Leave) -> Runs {
    let mut editor = oso_core::editor::Editor::default();
    Runs::Native(NativeOrder {
        buffer: editor.add_terminal_buffer("*claude: test*"),
        trust: trust::ByRecord::No,
        by_claim: false,
        gate: space_gate::ByGate::No,
        remote,
        settings: Settings::default(),
        env: Vec::new(),
        out: Box::new(|_| {}),
        permission_mode: None,
        model: None,
        effort: None,
        past: None,
        tui_to_end: None,
    })
}

/// A terminal's start; with `handed`, the TUI's a native session is handed to.
fn terminal(handed: Option<Chosen>) -> Runs {
    let mut editor = oso_core::editor::Editor::default();
    let buffer = editor.add_terminal_buffer("*claude: test*");
    Runs::Terminal(terminal_order(buffer, handed))
}

/// The `--settings` of the line `kind` and `runs` start with, at most one; `gated` as in a
/// space's own directory.
fn settings(kind: Kind, runs: Runs, steer_worktrees: bool, gated: bool) -> Option<Value> {
    let args = line(kind, runs, (steer_worktrees, gated), None);
    let found: Vec<&str> = (args.iter())
        .filter_map(|arg| arg.strip_prefix("--settings="))
        .collect();
    assert!(found.len() <= 1, "{found:?}");
    found
        .first()
        .map(|text| serde_json::from_str(text).unwrap())
}

/// The arguments of the line `kind` and `runs` start with, the session's own model `model`.
fn line(
    kind: Kind,
    runs: Runs,
    (steer_worktrees, gated): (bool, bool),
    model: Option<&str>,
) -> Vec<String> {
    // Not a POSIX shell, so `claude` runs directly: any executable file stands in for both.
    let exe = std::env::current_exe().unwrap();
    let shell = Executable::absolute(&exe).unwrap();
    let order = Order {
        token: 1,
        kind,
        name: "test".to_owned(),
        directory: PathBuf::from("/nowhere"),
        project: None,
        trusted: None,
        baseline: false,
        state: None,
        claude: Some(exe),
        runs,
        brief: None,
        steer_worktrees,
        check_shared: false,
        space: None,
        model: model.map(str::to_owned),
    };
    let launch = launch_of(&order, &id(1), &shell, &Extra::default(), gated).unwrap();
    (launch.args.iter())
        .map(|arg| arg.to_str().unwrap().to_owned())
        .collect()
}

/// The `--model` arguments of a line.
fn models(args: &[String]) -> Vec<&str> {
    (args.iter())
        .filter(|arg| arg.starts_with("--model"))
        .map(String::as_str)
        .collect()
}

/// A session a tool started with a model passes it on every line of its own, terminal and
/// native alike, as one `--model=` argument; what the person chose since (a native session's
/// `/model`, what it handed the TUI) wins (ADR 0121, "A start's model, as amended").
#[test]
fn a_sessions_own_model_goes_on_each_of_its_lines_unless_a_choice_since_names_one() {
    let chose = |model: &str| match native() {
        Runs::Native(order) => Runs::Native(NativeOrder {
            model: Some(model.to_owned()),
            ..order
        }),
        Runs::Terminal(_) => unreachable!(),
    };
    let handed = |model: Option<&str>| {
        terminal(Some(Chosen {
            model: model.map(str::to_owned),
            ..Chosen::default()
        }))
    };
    for kind in kinds() {
        let own = Some("claude-opus-5-5[1m]");
        for runs in [native(), terminal(None), handed(None)] {
            let args = line(kind.clone(), runs, (false, false), own);
            assert_eq!(models(&args), ["--model=claude-opus-5-5[1m]"], "{kind:?}");
            let args = line(kind.clone(), native(), (false, false), None);
            assert_eq!(models(&args), Vec::<&str>::new(), "{kind:?}");
        }
        let args = line(kind.clone(), chose("haiku"), (false, false), own);
        assert_eq!(models(&args), ["--model=haiku"], "{kind:?}");
        if matches!(kind, Kind::Resume(_)) {
            let args = line(kind.clone(), handed(Some("haiku")), (false, false), own);
            assert_eq!(models(&args), ["--model=haiku"], "{kind:?}");
        }
    }
}

#[test]
fn every_native_start_has_remote_control_off() {
    for kind in kinds() {
        for gated in [false, true] {
            let said = settings(kind.clone(), native(), false, gated);
            let off = serde_json::json!({"disableRemoteControl": true});
            assert_eq!(said, Some(off), "{kind:?}");
        }
    }
}

#[test]
fn a_native_start_keeps_the_steering_hook_beside_it() {
    for kind in kinds() {
        let said = settings(kind.clone(), native(), true, false).unwrap();
        assert_eq!(said["disableRemoteControl"], true, "{kind:?}");
        let command = said["hooks"]["PreToolUse"][0]["hooks"][0]["command"].as_str();
        assert!(
            command.is_some_and(|c| c.ends_with(" hook deny-worktree")),
            "{said}"
        );
    }
}

/// Only a start given leave to connect has no block, and then only oso's request connects it
/// (ADR 0181).
#[test]
fn a_start_given_leave_to_connect_is_not_blocked_and_does_not_connect_by_itself() {
    for kind in kinds() {
        let remote = connect::Leave::Connect(connect::Connect::asked(connect::Sent::default()));
        let said = settings(kind.clone(), native_with(remote), true, false).unwrap();
        assert_eq!(said.get("disableRemoteControl"), None, "{kind:?}");
        assert_eq!(said["remoteControlAtStartup"], false, "{kind:?}");
        assert!(said.get("hooks").is_some());
    }
}

/// The person's own settings decide in the TUI, the one a native session is handed to too.
#[test]
fn no_terminal_start_has_remote_control_off() {
    for kind in kinds() {
        for handed in [None, Some(Chosen::default())] {
            let runs = || terminal(handed.clone());
            assert_eq!(settings(kind.clone(), runs(), false, false), None);
            let said = settings(kind.clone(), runs(), true, false).unwrap();
            let keys: Vec<&String> = said.as_object().unwrap().keys().collect();
            assert_eq!(keys, ["hooks"], "{kind:?}");
        }
    }
}
