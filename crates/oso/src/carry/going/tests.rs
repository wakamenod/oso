//! Tests of the order of `oso-restart`, with no process: fakes stand for this instance's UI
//! thread and for the new instance.
//!
//! Must not depend on: processes or sockets.

use serde_json::{Value, json};

use super::{ANSWER_TIME, Ended, Hello, Here, There, hand_over};
use crate::carry::fds::Fd;
use crate::carry::{Op, STATE_VERSION};

#[derive(Default)]
struct FakeHere {
    log: Vec<String>,
    /// The state the export gives.
    state: Value,
    /// Whether the hand-over is refused; otherwise it gives no descriptor, as tests have none.
    refuse_hand_over: bool,
    refuse_exit: bool,
    refuse_export: bool,
}

impl Here for FakeHere {
    fn ask(&mut self, op: Op) -> Result<(Value, Vec<Fd>), String> {
        self.log
            .push(format!("here {op:?}").chars().take(40).collect());
        match op {
            Op::ExportState { .. } if self.refuse_export => Err("a password is read".into()),
            Op::ExportState { .. } => Ok((self.state.clone(), Vec::new())),
            Op::ExportText { offset, .. } => Ok((
                json!({ "text": "ab", "end": offset as u64 + 2 }),
                Vec::new(),
            )),
            Op::HandOverTerminals { .. } if self.refuse_hand_over => Err("edited".into()),
            Op::HandOverTerminals { .. } => Ok((json!({}), Vec::new())),
            _ => Err("not asked here".into()),
        }
    }

    fn exit(&mut self, generation: u64) -> Result<(), String> {
        self.log.push(format!("here exit {generation}"));
        if self.refuse_exit {
            Err("exit refused".into())
        } else {
            Ok(())
        }
    }

    fn stays(&mut self, why: String) {
        self.log.push(format!("here stays: {why}"));
    }
}

#[derive(Default)]
struct FakeThere {
    log: Vec<String>,
    hello: Option<Hello>,
    refuse: Option<&'static str>,
}

impl There for FakeThere {
    fn hello(&mut self) -> Result<Hello, String> {
        self.hello.clone().ok_or_else(|| "it said nothing".into())
    }

    fn ask(
        &mut self,
        say: &str,
        fields: Value,
        _fds: Vec<Fd>,
        patience: std::time::Duration,
    ) -> Result<Value, String> {
        let at = fields
            .get("offset")
            .map_or(String::new(), |o| format!(" {o}"));
        self.log
            .push(format!("there {say}{at} within {}s", patience.as_secs()));
        if self.refuse == Some(say) {
            return Err("no".into());
        }
        Ok(json!({ "generation": 1 }))
    }

    fn stay(&mut self) {
        self.log.push("there stay".into());
    }

    fn stop(&mut self) {
        self.log.push("there stopped".into());
    }
}

fn hello() -> Option<Hello> {
    Some(Hello {
        versions: (2, STATE_VERSION),
        library: Some("lib".into()),
    })
}

fn state(items: &[u64], terminals: usize) -> Value {
    json!({
        "generation": 7,
        "export": 1,
        "items": items,
        "terminals": vec![json!({}); terminals],
    })
}

fn run(here: &mut FakeHere, there: &mut FakeThere) -> Ended {
    hand_over(here, there)
}

#[test]
fn a_restart_carries_the_pieces_the_state_and_the_masters_then_exits() {
    let mut here = FakeHere {
        state: state(&[4], 0),
        ..FakeHere::default()
    };
    let mut there = FakeThere {
        hello: hello(),
        ..FakeThere::default()
    };
    // No terminals: nothing is handed over.
    assert_eq!(run(&mut here, &mut there), Ended::Replaced);
    let answer = ANSWER_TIME.as_secs();
    assert_eq!(
        there.log,
        [
            format!("there stage-piece 0 within {answer}s"),
            format!("there stage-piece 2 within {answer}s"),
            format!("there take-state within {answer}s"),
        ]
    );
    assert_eq!(here.log.last().unwrap(), "here exit 7");
}

#[test]
fn terminals_wait_for_the_new_instance_to_hold_their_masters() {
    // The fake hands over no descriptor, which a real UI thread would for each terminal.
    let mut here = FakeHere {
        state: state(&[], 1),
        ..FakeHere::default()
    };
    let mut there = FakeThere {
        hello: hello(),
        ..FakeThere::default()
    };
    let ended = run(&mut here, &mut there);
    assert_eq!(
        ended,
        Ended::Stayed("0 masters were handed over for 1 terminals".into())
    );
    assert_eq!(there.log.last().unwrap(), "there stay");
    assert!(!here.log.iter().any(|line| line.starts_with("here exit")));
}

#[test]
fn a_new_instance_that_says_nothing_or_reads_older_states_is_stopped() {
    for hello in [
        None,
        Some(Hello {
            versions: (2, STATE_VERSION - 1),
            library: None,
        }),
    ] {
        let mut here = FakeHere {
            state: state(&[], 0),
            ..FakeHere::default()
        };
        let mut there = FakeThere {
            hello,
            ..FakeThere::default()
        };
        let Ended::Stopped(why) = run(&mut here, &mut there) else {
            panic!("not stopped");
        };
        assert!(why.starts_with("the new oso did not take over"), "{why}");
        assert_eq!(there.log, ["there stopped"]);
        // Nothing was exported, and this instance says why it stays.
        assert_eq!(here.log.len(), 1);
        assert!(here.log[0].starts_with("here stays"));
    }
}

#[test]
fn a_refused_export_or_state_stops_the_new_instance_and_this_one_stays() {
    let mut here = FakeHere {
        refuse_export: true,
        ..FakeHere::default()
    };
    let mut there = FakeThere {
        hello: hello(),
        ..FakeThere::default()
    };
    assert_eq!(
        run(&mut here, &mut there),
        Ended::Stopped("cannot restart: a password is read".into())
    );
    assert_eq!(there.log, ["there stopped"]);

    let mut here = FakeHere {
        state: state(&[], 0),
        ..FakeHere::default()
    };
    let mut there = FakeThere {
        hello: hello(),
        refuse: Some("take-state"),
        ..FakeThere::default()
    };
    assert!(matches!(run(&mut here, &mut there), Ended::Stopped(_)));
    assert_eq!(there.log.last().unwrap(), "there stopped");
    assert!(here.log.last().unwrap().starts_with("here stays"));
}

#[test]
fn an_exit_refused_after_the_import_tells_the_new_instance_to_stay() {
    let mut here = FakeHere {
        state: state(&[], 0),
        refuse_exit: true,
        ..FakeHere::default()
    };
    let mut there = FakeThere {
        hello: hello(),
        ..FakeThere::default()
    };
    assert_eq!(
        run(&mut here, &mut there),
        Ended::Stayed("exit refused".into())
    );
    assert_eq!(there.log.last().unwrap(), "there stay");
    assert_eq!(here.log.last().unwrap(), "here stays: exit refused");
}

#[test]
fn a_refused_hand_over_tells_the_new_instance_to_stay() {
    let mut here = FakeHere {
        state: state(&[], 2),
        refuse_hand_over: true,
        ..FakeHere::default()
    };
    let mut there = FakeThere {
        hello: hello(),
        ..FakeThere::default()
    };
    assert_eq!(run(&mut here, &mut there), Ended::Stayed("edited".into()));
    assert!(!there.log.iter().any(|line| line.contains("take-masters")));
}

#[test]
fn the_names_passed_leave_the_loader_and_path_out() {
    let names = super::inherited_names();
    assert!(
        !names
            .iter()
            .any(|name| name == "PATH" || name.starts_with("DYLD_"))
    );
}
