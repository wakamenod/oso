//! The space gate's judge and the start thread's look at an order (ADR 0176).
//!
//! Must not depend on: `app`.

use std::path::{Path, PathBuf};

use super::{ByGate, NotAdmitted, Standing, judge, settle};
use crate::sessions::identity::SessionId;
use crate::sessions::spaces::{Member, SpaceId, Stored};

#[cfg(not(windows))]
const ROOT: &str = "/nowhere/spaces";
#[cfg(windows)]
const ROOT: &str = r"C:\nowhere\spaces";

fn id(n: u8) -> SessionId {
    SessionId::from_random([n; 16])
}

/// An open space `fix-login` whose directory oso made under [`ROOT`], with orchestrator 1 and
/// earlier orchestrator 2.
fn space() -> Stored {
    let member = Member::at(&Path::new(ROOT).join("repo"));
    let mut stored = Stored::of_repository(
        SpaceId::from_random([9; 16]),
        "fix-login".to_owned(),
        member,
        1,
    );
    stored.directory = Some(directory());
    stored.directory_identity = Some((1, 2));
    stored.orchestrator = Some(id(1));
    stored.earlier = vec![id(2)];
    stored
}

fn directory() -> PathBuf {
    Path::new(ROOT).join("fix-login")
}

fn said(stored: &Stored, root: Option<&Path>, who: u8, persons: bool) -> Result<(), NotAdmitted> {
    let id = id(who);
    let standing = Standing::Of {
        id: &id,
        persons_there: persons,
    };
    judge(stored, root, &directory(), standing).map(|_| ())
}

#[test]
fn the_current_and_earlier_orchestrators_and_the_persons_sessions_run_natively_there() {
    let stored = space();
    let root = Some(Path::new(ROOT));
    assert_eq!(said(&stored, root, 1, false), Ok(()));
    assert_eq!(said(&stored, root, 2, false), Ok(()));
    assert_eq!(said(&stored, root, 3, true), Ok(()));
    assert_eq!(
        said(&stored, root, 3, false),
        Err(NotAdmitted::NotTheSpaces)
    );
}

/// A new orchestrator, which has no conversation yet, runs natively in its open space's
/// directory oso made, as its other sessions do (part O-3).
#[test]
fn a_new_orchestrator_runs_natively_only_in_its_open_spaces_directory_oso_made() {
    let root = Some(Path::new(ROOT));
    let its = SpaceId::from_random([9; 16]);
    let new = |stored: &Stored, root: Option<&Path>| {
        let standing = Standing::NewOrchestrator { space: &its };
        judge(stored, root, &directory(), standing).map(|_| ())
    };
    // Another space's new orchestrator does not run in this one's directory.
    let other = SpaceId::from_random([8; 16]);
    let elsewhere = Standing::NewOrchestrator { space: &other };
    assert_eq!(
        judge(&space(), root, &directory(), elsewhere).map(|_| ()),
        Err(NotAdmitted::NotTheSpaces)
    );
    let mut none_yet = space();
    none_yet.orchestrator = None;
    none_yet.earlier.clear();
    assert_eq!(new(&none_yet, root), Ok(()));
    let mut closed = space();
    closed.archived = true;
    assert_eq!(new(&closed, root), Err(NotAdmitted::Closed));
    let moved = Path::new(ROOT).join("elsewhere");
    assert_eq!(new(&space(), Some(&moved)), Err(NotAdmitted::NotMade));
    assert_eq!(new(&space(), None), Err(NotAdmitted::NotMade));
}

#[test]
fn a_closed_space_or_a_directory_not_made_under_the_root_admits_none() {
    let root = Some(Path::new(ROOT));
    let mut closed = space();
    closed.archived = true;
    assert_eq!(said(&closed, root, 1, false), Err(NotAdmitted::Closed));
    let other = Path::new(ROOT).join("elsewhere");
    assert_eq!(
        said(&space(), Some(&other), 1, false),
        Err(NotAdmitted::NotMade)
    );
    assert_eq!(said(&space(), None, 1, false), Err(NotAdmitted::NotMade));
    // A directory oso did not make where the system has identities.
    let mut unmade = space();
    unmade.directory_identity = None;
    let expected = if cfg!(unix) {
        Err(NotAdmitted::NotMade)
    } else {
        Ok(())
    };
    assert_eq!(said(&unmade, root, 1, false), expected);
}

/// A native order in [`directory`] with `gate`, and the space's checks as given.
fn order(gate: ByGate, space: Option<crate::sessions::start::SpaceStart>) -> super::Order {
    use crate::sessions::start::{Kind, NativeOrder, Order, Runs};
    let mut editor = oso_core::editor::Editor::default();
    let buffer = editor.add_terminal_buffer("*claude: test*");
    Order {
        token: 1,
        kind: Kind::Resume(id(1)),
        name: "test".to_owned(),
        directory: directory(),
        project: None,
        trusted: None,
        baseline: false,
        state: None,
        claude: None,
        runs: Runs::Native(NativeOrder {
            buffer,
            trust: crate::sessions::start::trust::ByRecord::No,
            gate,
            settings: crate::terminal::shell::Settings::default(),
            env: Vec::new(),
            out: Box::new(|_| {}),
            permission_mode: None,
            model: None,
            effort: None,
            past: None,
            tui_to_end: None,
        }),
        brief: None,
        steer_worktrees: false,
        check_shared: false,
        space,
    }
}

fn checks(
    made: Option<(u64, u64)>,
    in_space_directory: bool,
) -> crate::sessions::start::SpaceStart {
    crate::sessions::start::SpaceStart {
        in_space_directory,
        made,
        members: Vec::new(),
        trusted: Vec::new(),
        reads: Vec::new(),
        prompt: String::new(),
        instructions: None,
    }
}

fn gate() -> ByGate {
    let one = id(1);
    let standing = Standing::Of {
        id: &one,
        persons_there: false,
    };
    ByGate::Gate(judge(&space(), Some(Path::new(ROOT)), &directory(), standing).unwrap())
}

#[test]
fn the_start_thread_takes_a_gate_only_where_the_spaces_checks_looked() {
    let passes = settle(&mut order(gate(), Some(checks(Some((1, 2)), true))));
    assert!(passes.is_ok_and(|gate| gate.is_some()));
    for space in [
        None,
        Some(checks(Some((1, 2)), false)),
        Some(checks(Some((3, 4)), true)),
    ] {
        assert!(settle(&mut order(gate(), space)).is_err());
    }
    let mut elsewhere = order(gate(), Some(checks(Some((1, 2)), true)));
    elsewhere.directory = Path::new(ROOT).join("other");
    assert!(settle(&mut elsewhere).is_err());
    assert!(settle(&mut order(ByGate::Waits, None)).is_err());
    assert!(settle(&mut order(ByGate::No, None)).is_ok_and(|gate| gate.is_none()));
}

#[test]
fn a_gate_taken_back_refuses_the_start() {
    let gate = gate();
    if let ByGate::Gate(gate) = &gate {
        gate.clone().revoke();
    }
    let refused = settle(&mut order(gate, Some(checks(Some((1, 2)), true))));
    assert_eq!(refused.err().as_deref(), Some(super::TAKEN_BACK));
}

/// An order never runs under a project's trust and a space's gate at once.
#[test]
fn an_order_judged_by_the_record_too_is_refused() {
    let mut both = order(gate(), Some(checks(Some((1, 2)), true)));
    if let crate::sessions::start::Runs::Native(native) = &mut both.runs {
        native.trust = crate::sessions::start::trust::ByRecord::Unread;
    }
    assert!(settle(&mut both).is_err());
}
