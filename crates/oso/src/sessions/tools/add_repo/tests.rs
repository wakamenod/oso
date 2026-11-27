//! What `space_add_repo`'s reply says of a native orchestrator (ADR 0176, part O-2).
//!
//! Must not depend on: the app or threads; the rules are plain data and functions.

use super::{AddDir, add_dir};

/// A native orchestrator, which runs no `/add-dir`, is resumed with the repository once its
/// turn ends, asking no one (ADR 0185), and is told of its next resume when oso cannot.
#[test]
fn a_native_orchestrator_is_resumed_with_the_repository() {
    assert_eq!(add_dir(true, false, true, true), AddDir::ResumeQueued);
    assert_eq!(add_dir(true, false, false, true), AddDir::NextResume);
    assert_eq!(add_dir(true, true, true, true), AddDir::Given);
    assert_eq!(add_dir(false, false, true, true), AddDir::WaitsForTrust);
    assert_eq!(AddDir::ResumeQueued.word(), "resume queued");
    for native in [AddDir::ResumeQueued, AddDir::WaitsForTrust] {
        let note = native.note("/r/web", true);
        assert!(note.contains("resumes you with /r/web"), "{note}");
        assert!(
            !note.contains("paste") && !note.contains("offers"),
            "{note}"
        );
    }
}

/// The reply says oso types and sends `/add-dir` itself once the turn ends, asking no one
/// (ADR 0185), and a restricted one waits for the person's trust (ADR 0161, Q5).
#[test]
fn the_reply_says_what_reads_the_added_repository() {
    assert_eq!(add_dir(false, true, true, false), AddDir::WaitsForTrust);
    assert_eq!(add_dir(true, true, false, false), AddDir::Given);
    assert_eq!(add_dir(true, false, true, false), AddDir::Queued);
    assert_eq!(add_dir(true, false, false, false), AddDir::NotQueued);
    let native = add_dir(true, false, false, true);
    assert_eq!(native, AddDir::NextResume);
    let note = AddDir::Queued.note("/r/web", false);
    assert!(
        note.contains("oso types /add-dir /r/web") && note.contains("this turn ends"),
        "{note}"
    );
    assert!(
        !note.contains("ask them") && !note.contains("person"),
        "{note}"
    );
    let trust = AddDir::WaitsForTrust.note("/r/web", false);
    assert!(
        trust.contains("trust-project") && trust.contains("oso then types"),
        "{trust}"
    );
    assert!(!trust.contains("offers"), "{trust}");
    assert_eq!(AddDir::Queued.word(), "queued");
    assert_eq!(AddDir::NotQueued.word(), "not queued");
}
