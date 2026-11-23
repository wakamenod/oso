//! What `space_add_repo`'s reply says of a native orchestrator (ADR 0176, part O-2).
//!
//! Must not depend on: the app or threads; the rules are plain data and functions.

use super::{AddDir, add_dir};

/// A native orchestrator, which runs no `/add-dir`, is offered a resume with the repository
/// when oso asked, and is told of its next resume when it could not ask.
#[test]
fn a_native_orchestrator_is_offered_a_resume_with_the_repository() {
    assert_eq!(add_dir(true, false, true, true), AddDir::ResumeOffered);
    assert_eq!(add_dir(true, false, false, true), AddDir::NextResume);
    assert_eq!(add_dir(true, true, true, true), AddDir::Given);
    assert_eq!(add_dir(false, false, true, true), AddDir::WaitsForTrust);
    assert_eq!(AddDir::ResumeOffered.word(), "resume offered");
    for native in [AddDir::ResumeOffered, AddDir::WaitsForTrust] {
        let note = native.note("/r/web", true);
        assert!(note.contains("resume you with /r/web"), "{note}");
        assert!(!note.contains("paste"), "{note}");
    }
}
