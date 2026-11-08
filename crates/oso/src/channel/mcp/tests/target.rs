//! Which instance `oso mcp` connects to, and how often it looks through them all.
//!
//! Must not depend on: a socket; a stand-in says which instances have the channel on.

use std::io;
use std::time::Instant;

use super::super::{PROBE_EVERY, Target};

/// A stand-in for the control directory: the instances whose channel is on, and each connect
/// it was asked for.
struct Instances {
    on: Vec<u32>,
    asked: Vec<Option<u32>>,
}

impl Instances {
    fn new(on: &[u32]) -> Self {
        Self {
            on: on.to_vec(),
            asked: Vec::new(),
        }
    }

    fn connect(&mut self, pid: Option<u32>) -> io::Result<(u32, ())> {
        self.asked.push(pid);
        match (pid, self.on.as_slice()) {
            (Some(pid), on) if on.contains(&pid) => Ok((pid, ())),
            (Some(_), _) | (None, []) => Err(io::ErrorKind::NotFound.into()),
            (None, [only]) => Ok((*only, ())),
            (None, _) => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "found more than one agent channel (pids 1, 2); choose one with --pid",
            )),
        }
    }
}

#[test]
fn it_looks_through_the_instances_only_until_it_knows_one() {
    let mut instances = Instances::new(&[41]);
    let mut target = Target::new(None);
    let now = Instant::now();
    for _ in 0..3 {
        let (pid, ()) = target.connect(now, |pid| instances.connect(pid)).unwrap();
        assert_eq!(pid, 41);
    }
    assert_eq!(instances.asked, [None, Some(41), Some(41)]);
}

#[test]
fn it_looks_again_at_most_once_each_while_it_finds_none() {
    let mut instances = Instances::new(&[]);
    let mut target = Target::new(None);
    let start = Instant::now();
    let off = target
        .connect(start, |pid| instances.connect(pid))
        .unwrap_err();
    assert!(off.contains("agent-channel-start"), "{off}");
    instances.on = vec![41];
    let soon = start + PROBE_EVERY / 2;
    let again = target.connect(soon, |pid| instances.connect(pid));
    assert_eq!(again.unwrap_err(), off, "told what it found last");
    assert_eq!(instances.asked, [None]);
    let later = start + PROBE_EVERY;
    assert_eq!(
        target
            .connect(later, |pid| instances.connect(pid))
            .unwrap()
            .0,
        41
    );
}

#[test]
fn an_instance_that_went_is_looked_past_unless_pid_named_it() {
    let mut instances = Instances::new(&[41]);
    let mut target = Target::new(None);
    let now = Instant::now();
    target.connect(now, |pid| instances.connect(pid)).unwrap();
    // oso restarted, as another process.
    instances.on = vec![42];
    let (pid, ()) = target.connect(now, |pid| instances.connect(pid)).unwrap();
    assert_eq!(pid, 42);

    let mut given = Target::new(Some(41));
    let mut instances = Instances::new(&[42]);
    let refused = given
        .connect(now, |pid| instances.connect(pid))
        .unwrap_err();
    assert!(refused.contains("--pid"), "{refused}");
    assert_eq!(instances.asked, [Some(41)], "never another instance");
}

#[test]
fn several_instances_are_named_without_their_ids() {
    let mut instances = Instances::new(&[1, 2]);
    let mut target = Target::new(None);
    let told = target
        .connect(Instant::now(), |pid| instances.connect(pid))
        .unwrap_err();
    assert!(told.contains("several") && told.contains("--pid"), "{told}");
    assert!(!told.contains('1') && !told.contains('2'), "{told}");
}
