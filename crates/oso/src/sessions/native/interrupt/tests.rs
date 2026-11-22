//! The interrupt's rules (ADR 0165, step 2c): an interrupt from its request to its fallback, a
//! signal, a stop and a resume, or, with no signal, given up.
//!
//! Must not depend on: the app or a process; the tests give the instants.

use std::time::{Duration, Instant};

use super::{ANSWER_WAIT, END_WAIT, Interrupt, NotBegun, SIGNAL_WAIT, Then};
use crate::sessions::native::control::tests::{ok, on, refused};
use crate::sessions::native::control::{Answered, Control, RECEIPT};
use crate::sessions::native::wire::control::INITIALIZE;

/// The id an interrupt begun on tier 2 asks with.
fn asked(interrupt: &mut Interrupt, control: &mut Control, now: Instant) -> u64 {
    match interrupt.begin(control, now) {
        Ok(Then::Request(id)) => id,
        other => panic!("{other:?}"),
    }
}

#[test]
fn on_tier_two_an_interrupt_is_a_request_that_ends_with_its_turn() {
    let now = Instant::now();
    let mut control = on(now);
    let mut interrupt = Interrupt::new(true);
    let id = asked(&mut interrupt, &mut control, now);
    assert!(interrupt.interrupting());
    assert_eq!(interrupt.begin(&mut control, now), Err(NotBegun::Already));
    assert_eq!(interrupt.deadline(), Some(now + ANSWER_WAIT));
    assert_eq!(
        interrupt.answered(&mut control, id + 1, None, now),
        Vec::new(),
        "not its answer"
    );
    assert_eq!(interrupt.answered(&mut control, id, None, now), Vec::new());
    assert_eq!(interrupt.deadline(), Some(now + END_WAIT));
    interrupt.turn_ended();
    assert!(!interrupt.interrupting());
    assert!(!interrupt.resumes());
    assert_eq!(interrupt.deadline(), None);
    assert_ne!(asked(&mut interrupt, &mut control, now), id);
}

#[test]
fn a_request_unanswered_refused_or_whose_turn_goes_on_falls_back_to_a_signal_saying_why() {
    let now = Instant::now();
    let signalled = |why: &str| vec![Then::Log(why.to_owned()), Then::Signal];

    let (mut control, mut interrupt) = (on(now), Interrupt::new(true));
    let id = asked(&mut interrupt, &mut control, now);
    assert_eq!(
        interrupt.due(&mut control, now + ANSWER_WAIT),
        signalled("interrupt was not answered")
    );
    assert!(interrupt.resumes());
    assert_eq!(control.answered(&ok(id)), Answered::Other, "forgotten");

    let (mut control, mut interrupt) = (on(now), Interrupt::new(true));
    let id = asked(&mut interrupt, &mut control, now);
    assert_eq!(
        interrupt.answered(&mut control, id, Some("no"), now),
        signalled("interrupt was refused: no")
    );

    let (mut control, mut interrupt) = (on(now), Interrupt::new(true));
    let id = asked(&mut interrupt, &mut control, now);
    interrupt.answered(&mut control, id, None, now);
    let later = now + Duration::from_secs(1);
    assert_eq!(interrupt.due(&mut control, later), Vec::new());
    assert_eq!(
        interrupt.due(&mut control, now + END_WAIT),
        signalled("the turn went on after its interrupt")
    );

    let (mut control, mut interrupt) = (on(now), Interrupt::new(true));
    asked(&mut interrupt, &mut control, now);
    assert_eq!(
        interrupt.not_written(&mut control, now),
        signalled("the interrupt request was not written")
    );
}

#[test]
fn on_tier_one_the_signal_is_followed_by_a_stop_and_the_process_resumes() {
    let now = Instant::now();
    let mut control = Control::started(now);
    control.answered(&refused(INITIALIZE));
    let mut interrupt = Interrupt::new(true);
    assert_eq!(interrupt.begin(&mut control, now), Ok(Then::Signal));
    assert!(interrupt.resumes());
    interrupt.turn_ended();
    assert!(interrupt.resumes(), "the process ends after its turn");
    assert_eq!(interrupt.deadline(), Some(now + SIGNAL_WAIT));
    assert_eq!(
        interrupt.due(&mut control, now + SIGNAL_WAIT),
        vec![Then::Stop]
    );
    assert_eq!(
        interrupt.due(&mut control, now + SIGNAL_WAIT * 2),
        Vec::new(),
        "stopped once"
    );
    assert!(interrupt.resumes());
    assert_eq!(interrupt.deadline(), None);
    assert_eq!(interrupt.begin(&mut control, now), Err(NotBegun::Already));
}

#[test]
fn with_no_signal_an_interrupt_the_request_does_not_carry_is_given_up() {
    let now = Instant::now();
    let mut control = Control::started(now);
    let mut interrupt = Interrupt::new(false);
    assert_eq!(interrupt.begin(&mut control, now), Err(NotBegun::NoWay));
    assert!(!interrupt.interrupting());

    let mut control = on(now);
    asked(&mut interrupt, &mut control, now);
    assert_eq!(
        interrupt.due(&mut control, now + ANSWER_WAIT),
        vec![
            Then::Log("interrupt was not answered".to_owned()),
            Then::GaveUp
        ]
    );
    assert!(!interrupt.interrupting());
    assert!(
        !interrupt.resumes(),
        "nothing resumes what was not signalled"
    );
}

/// One step of the person, the CLI or the clock, for the property below.
#[derive(Debug, Clone)]
enum Step {
    Interrupt,
    Answer { initialize: bool, success: bool },
    TurnEnded,
    NotWritten,
    Init(bool),
    Wait(u64),
}

fn step() -> impl proptest::strategy::Strategy<Value = Step> {
    use proptest::prelude::*;
    prop_oneof![
        Just(Step::Interrupt),
        (any::<bool>(), any::<bool>()).prop_map(|(initialize, success)| Step::Answer {
            initialize,
            success
        }),
        Just(Step::TurnEnded),
        Just(Step::NotWritten),
        any::<bool>().prop_map(Step::Init),
        (0_u64..20_000).prop_map(Step::Wait),
    ]
}

proptest::proptest! {
    /// Whatever comes, what is due is carried out: after `due(now)`, nothing is due at or
    /// before `now`, so the event loop never wakes for a deadline already past.
    #[test]
    fn a_deadline_carried_out_is_never_left_in_the_past(
        steps in proptest::collection::vec(step(), 0..64),
        signals in proptest::bool::ANY,
    ) {
        let mut now = Instant::now();
        let mut control = Control::started(now);
        let mut interrupt = Interrupt::new(signals);
        let mut last = INITIALIZE;
        for step in steps {
            match step {
                Step::Interrupt => {
                    if let Ok(Then::Request(id)) = interrupt.begin(&mut control, now) {
                        last = id;
                    }
                }
                Step::Answer { initialize, success } => {
                    let id = if initialize { INITIALIZE } else { last };
                    let answer = if success { ok(id) } else { refused(id) };
                    if let Answered::Request { id, error, .. } = control.answered(&answer) {
                        interrupt.answered(&mut control, id, error.as_deref(), now);
                    }
                }
                Step::TurnEnded => interrupt.turn_ended(),
                Step::NotWritten => {
                    if interrupt.interrupting() && !interrupt.resumes() {
                        interrupt.not_written(&mut control, now);
                    }
                }
                Step::Init(listed) => {
                    let names = if listed { vec![RECEIPT.to_owned()] } else { Vec::new() };
                    control.system_init(&names);
                }
                Step::Wait(millis) => now += Duration::from_millis(millis),
            }
            control.due(now);
            interrupt.due(&mut control, now);
            for by in [control.deadline(), interrupt.deadline()].into_iter().flatten() {
                proptest::prop_assert!(by > now, "{by:?} is not after {now:?}");
            }
        }
    }
}
