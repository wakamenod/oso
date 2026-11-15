//! When a status is read: what the person does reads it, what an agent does only marks it
//! stale, and the person's next key in it reads it then (ADR 0127, ADR 0085).
//!
//! Must not depend on: winit or the window; these are the decision's own tests.

use super::{Decision, Seen};

fn seen() -> Seen {
    Seen {
        was_shown: false,
        shown: true,
        stale: false,
        agent_stale: false,
        keyed: false,
        agent: false,
    }
}

#[test]
fn a_status_the_person_shows_is_read() {
    assert_eq!(seen().decide(), Decision::Read);
    let waited = Seen {
        was_shown: true,
        stale: true,
        ..seen()
    };
    assert_eq!(waited.decide(), Decision::Read);
    let hidden = Seen {
        shown: false,
        ..seen()
    };
    assert_eq!(hidden.decide(), Decision::Nothing);
}

#[test]
fn a_status_an_agent_shows_is_only_marked_stale() {
    let agent = Seen {
        agent: true,
        ..seen()
    };
    assert_eq!(agent.decide(), Decision::Stale);
    let waited = Seen {
        was_shown: true,
        stale: true,
        ..agent
    };
    assert_eq!(waited.decide(), Decision::Stale);
}

#[test]
fn a_stale_status_is_read_on_the_persons_key_in_it_only() {
    let stale = Seen {
        was_shown: true,
        agent_stale: true,
        ..seen()
    };
    assert_eq!(stale.decide(), Decision::Nothing);
    let keyed = Seen {
        keyed: true,
        ..stale
    };
    assert_eq!(keyed.decide(), Decision::Read);
    // An agent's command in it is no key of the person's.
    let by_agent = Seen {
        agent: true,
        ..keyed
    };
    assert_eq!(by_agent.decide(), Decision::Nothing);
}
