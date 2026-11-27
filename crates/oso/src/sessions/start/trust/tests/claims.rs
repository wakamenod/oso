//! The judge's table for a resume's worktree claim, and the start thread's looks at real
//! worktrees, as `git worktree add` leaves them, with no git run (ADR 0165, "Worktrees'
//! resumes").
//!
//! Must not depend on: `app`.

use std::path::Path;
#[cfg(unix)]
use std::path::PathBuf;

use oso_core::project::TrustList;

use super::super::{Claims, Judged, Kept, NotTrusted, TrustRecord, judge};
use super::{OTHER, ROOT, open, record, said_with};

const ASKED: Claims<'static> = Claims::Asked { home: None };

#[test]
fn only_a_resume_with_no_buffer_open_and_a_root_not_listed_asks_its_claim() {
    let other = record(Path::new(OTHER), Some((1, 2)));
    let read = Some(TrustRecord::Read(&other));
    assert_eq!(said_with(None, read, true, ASKED), Ok("claimed"));
    // A new start, a fork, an adoption and a moved session.
    assert_eq!(
        said_with(None, read, true, Claims::Never),
        Err(NotTrusted::NotOpen)
    );
    assert_eq!(
        said_with(None, None, true, ASKED),
        Err(NotTrusted::NotOpen),
        "with no record asked, there is none to trust its repository"
    );
    // A root the record holds is the record's to judge, whatever identity it holds it with.
    let listed = record(Path::new(ROOT), Some((1, 2)));
    let held = Some(TrustRecord::Read(&listed));
    assert_eq!(said_with(None, held, true, ASKED), Ok("listed"));
    // An open buffer decides.
    assert_eq!(said_with(Some(Some(open())), read, true, ASKED), Ok("open"));
    assert_eq!(
        said_with(Some(None), read, true, ASKED),
        Err(NotTrusted::Untrusted)
    );
    assert_eq!(
        said_with(None, Some(TrustRecord::Refused), true, ASKED),
        Err(NotTrusted::NoRecord)
    );
    assert_eq!(
        judge(false, Path::new(ROOT), None, read, true, ASKED).unwrap_err(),
        NotTrusted::NotAtRoot
    );
}

#[test]
fn a_resume_waits_for_the_record_before_an_untrusted_buffer_refuses() {
    let unread = Some(TrustRecord::Unread);
    for open in [None, Some(None)] {
        assert_eq!(said_with(open, unread, true, ASKED), Ok("waits"));
    }
    assert_eq!(
        said_with(Some(None), unread, false, ASKED),
        Err(NotTrusted::Untrusted),
        "not for ever"
    );
    assert_eq!(
        said_with(Some(None), unread, true, Claims::Never),
        Err(NotTrusted::Untrusted),
        "nor for a fork or an adoption"
    );
}

/// What the UI keeps of a claim judged at `ROOT` by `list`.
fn kept(list: &TrustList) -> Kept {
    let judged = judge(
        true,
        Path::new(ROOT),
        None,
        Some(TrustRecord::Read(list)),
        true,
        ASKED,
    );
    let judged = judged.unwrap();
    assert!(matches!(judged, Judged::Claimed(_)));
    judged.kept().expect("kept")
}

#[test]
fn a_claims_start_under_way_is_taken_back_as_any_root_of_the_record_goes() {
    let list = record(Path::new(OTHER), Some((1, 2)));
    let kept = kept(&list);
    let read = |list: &TrustList| kept.still_trusted(None, TrustRecord::Read(list));
    assert!(read(&list), "an event that changes nothing");
    let mut wider = list.format();
    wider.push_str(&record(Path::new(ROOT), Some((3, 4))).format());
    let (wider, skipped) = TrustList::parse(&wider);
    assert_eq!(skipped, 0);
    assert!(read(&wider), "another root trusted");
    assert!(!read(&TrustList::default()), "its repository untrusted");
    #[cfg(unix)]
    assert!(
        !read(&record(Path::new(OTHER), Some((1, 9)))),
        "trusted as another directory"
    );
    assert!(!kept.still_trusted(None, TrustRecord::Refused));
    assert!(
        kept.still_trusted(None, TrustRecord::Unread),
        "judged again"
    );
    let open_now = |open, list: &TrustList| kept.still_trusted(Some(open), TrustRecord::Read(list));
    assert!(!open_now(None, &list), "a buffer of it open and restricted");
    assert!(open_now(Some(open()), &list));
    assert!(
        !open_now(Some(open()), &TrustList::default()),
        "a trusted buffer of it keeps no repository that went"
    );
}

/// A main repository and a worktree of it, as git leaves them, in a directory of their own.
#[cfg(unix)]
struct Pair {
    scratch: super::Scratch,
    base: PathBuf,
    main: PathBuf,
    worktree: PathBuf,
}

#[cfg(unix)]
impl Pair {
    /// The worktree at `at` under the base, `main` beside or above it.
    fn at(name: &str, at: &str) -> Self {
        let scratch = super::scratch(name);
        let base = std::fs::canonicalize(&scratch.0).unwrap();
        let (main, worktree) = (base.join("main"), base.join(at));
        std::fs::create_dir_all(&main).unwrap();
        worktree_of(&main, &worktree);
        Self {
            scratch,
            base,
            main,
            worktree,
        }
    }

    fn new(name: &str) -> Self {
        Self::at(name, "main/.claude/worktrees/wt")
    }

    fn list(&self) -> TrustList {
        record(&self.main, super::identity(&self.main))
    }
}

/// Makes `worktree` a worktree of `main`, each naming the other.
#[cfg(unix)]
fn worktree_of(main: &Path, worktree: &Path) {
    let entry = main.join(".git/worktrees/wt");
    std::fs::create_dir_all(&entry).unwrap();
    std::fs::create_dir_all(worktree).unwrap();
    let git = worktree.join(".git");
    std::fs::write(entry.join("gitdir"), format!("{}\n", git.display())).unwrap();
    std::fs::write(&git, format!("gitdir: {}\n", entry.display())).unwrap();
}

#[cfg(unix)]
mod looks {
    use std::fs;
    use std::os::unix::fs::symlink;

    use super::super::super::{Admitted, ByRecord, Claimed, NoClaim, settle};
    use super::*;
    use crate::git::TrustedRoot;
    use crate::sessions::start::{Kind, NativeOrder, Order, Runs, still_gated};

    fn claimed_with(list: &TrustList, root: &Path, home: Option<&Path>) -> Claimed {
        let record = Some(TrustRecord::Read(list));
        match judge(true, root, None, record, false, Claims::Asked { home }) {
            Ok(Judged::Claimed(claimed)) => claimed,
            other => panic!("not judged by a claim: {other:?}"),
        }
    }

    fn look(list: &TrustList, root: &Path) -> Result<TrustedRoot, NoClaim> {
        claimed_with(list, root, None).look()
    }

    #[track_caller]
    fn refused(list: &TrustList, root: &Path, why: NoClaim) {
        assert_eq!(look(list, root).map(|_| ()), Err(why));
    }

    #[test]
    fn a_worktree_of_a_repository_the_record_trusts_is_admitted_with_no_buffer_open() {
        for at in ["main/.claude/worktrees/wt", "beside"] {
            let pair = Pair::at("claim", at);
            let root = look(&pair.list(), &pair.worktree).expect("its claim holds both ways");
            assert_eq!(root.path(), pair.worktree);
            assert_eq!(root.identity(), super::super::identity(&pair.worktree));
            // Its trust is the main repository's, not its own path's.
            assert!(root.listed_in(&pair.list()));
            assert!(!root.listed_in(&TrustList::default()));
            drop(pair.scratch);
        }
    }

    #[test]
    fn a_claim_one_way_is_none() {
        let pair = Pair::new("one-way");
        let list = pair.list();
        // It names the entry, which records another place.
        let copy = pair.base.join("copy");
        fs::create_dir(&copy).unwrap();
        fs::copy(pair.worktree.join(".git"), copy.join(".git")).unwrap();
        refused(&list, &copy, NoClaim::NotOne);
        // The entry records it, and it names a repository of its own.
        fs::remove_file(pair.worktree.join(".git")).unwrap();
        fs::create_dir(pair.worktree.join(".git")).unwrap();
        refused(&list, &pair.worktree, NoClaim::NotOne);
        // Or one the record does not hold.
        let other = pair.base.join("other");
        fs::create_dir(&other).unwrap();
        fs::remove_dir(pair.worktree.join(".git")).unwrap();
        worktree_of(&other, &pair.worktree);
        refused(&list, &pair.worktree, NoClaim::Unlisted);
    }

    #[test]
    fn its_main_repository_must_be_listed_as_the_directory_it_is() {
        let pair = Pair::at("main", "beside");
        let elsewhere = record(&pair.base.join("elsewhere"), Some((1, 2)));
        refused(&elsewhere, &pair.worktree, NoClaim::Unlisted);
        refused(&TrustList::default(), &pair.worktree, NoClaim::Unlisted);
        let list = pair.list();
        // Made again at the same path, its record of the worktree with it.
        fs::rename(&pair.main, pair.base.join("old")).unwrap();
        fs::create_dir(&pair.main).unwrap();
        worktree_of(&pair.main, &pair.worktree);
        refused(&list, &pair.worktree, NoClaim::NotOne);
        // Only its parent listed trusts nothing.
        let parent = record(&pair.base, super::super::identity(&pair.base));
        refused(&parent, &pair.worktree, NoClaim::Unlisted);
    }

    #[test]
    fn a_listed_root_never_falls_back_to_its_claim() {
        let pair = Pair::new("listed");
        let mut lines = pair.list().format();
        lines.push_str(&record(&pair.worktree, Some((1, 2))).format());
        let (list, skipped) = TrustList::parse(&lines);
        assert_eq!(skipped, 0);
        let record = Some(TrustRecord::Read(&list));
        let judged = judge(true, &pair.worktree, None, record, false, ASKED);
        let Ok(Judged::Listed(listed)) = judged else {
            panic!("not the record's: {judged:?}");
        };
        assert!(listed.look().is_none(), "another directory was recorded");
    }

    #[test]
    fn a_link_at_the_root_on_its_way_or_at_its_git_is_no_worktree() {
        let pair = Pair::at("links", "real/wt");
        let list = pair.list();
        let link = pair.base.join("link");
        symlink(&pair.worktree, &link).unwrap();
        refused(&list, &link, NoClaim::NotOne);
        let way = pair.base.join("way");
        symlink(pair.base.join("real"), &way).unwrap();
        refused(&list, &way.join("wt"), NoClaim::NotOne);
        let git = pair.worktree.join(".git");
        fs::rename(&git, pair.base.join("moved")).unwrap();
        symlink(pair.base.join("moved"), &git).unwrap();
        refused(&list, &pair.worktree, NoClaim::NotOne);
    }

    #[test]
    fn what_is_no_worktree_of_its_own_root_is_refused() {
        let pair = Pair::new("others");
        let list = pair.list();
        let nested = pair.main.join("vendor/lib");
        fs::create_dir_all(nested.join(".git")).unwrap();
        refused(&list, &nested, NoClaim::NotOne);
        let submodule = pair.main.join("sub");
        fs::create_dir(&submodule).unwrap();
        fs::write(submodule.join(".git"), "gitdir: ../.git/modules/sub\n").unwrap();
        refused(&list, &submodule, NoClaim::NotOne);
        let below = pair.worktree.join("src");
        fs::create_dir(&below).unwrap();
        refused(&list, &below, NoClaim::NotOne);
        let plain = pair.base.join("plain");
        fs::create_dir(&plain).unwrap();
        assert!(look(&list, &plain).is_err(), "no project at all");
        refused(&list, &pair.base.join("gone"), NoClaim::NotOne);
        // The home directory, however it is spelled, is never a project.
        let home = pair.base.join("home");
        symlink(&pair.worktree, &home).unwrap();
        for home in [&pair.worktree, &home] {
            let claimed = claimed_with(&list, &pair.worktree, Some(home));
            assert_eq!(claimed.look().map(|_| ()), Err(NoClaim::NotOne));
        }
    }

    #[test]
    fn a_refusal_names_no_path_its_git_names() {
        for why in [NoClaim::Revoked, NoClaim::Unlisted, NoClaim::NotOne] {
            let said = why.said();
            assert!(!said.contains('/'), "{said}");
        }
        assert_eq!(
            NoClaim::Unlisted.reason(),
            "its repository is not trusted now: trust-project it, then session-resume"
        );
        assert_eq!(
            NoClaim::NotOne.reason(),
            "its directory is neither a trusted repository nor a worktree of one now"
        );
    }

    /// A resume's order of `claimed` in `directory`, of the project at `project`.
    fn order(claimed: Claimed, directory: &Path, project: Option<&Path>) -> Order {
        let mut editor = oso_core::editor::Editor::default();
        let buffer = editor.add_terminal_buffer("*claude: test*");
        Order {
            token: 1,
            kind: Kind::Resume(crate::sessions::identity::SessionId::from_random([1; 16])),
            name: "test".to_owned(),
            directory: directory.to_owned(),
            project: project.map(Path::to_owned),
            trusted: None,
            baseline: false,
            state: None,
            claude: None,
            runs: Runs::Native(NativeOrder {
                buffer,
                trust: ByRecord::Claim(claimed),
                by_claim: true,
                gate: crate::sessions::start::space_gate::ByGate::No,
                remote: crate::sessions::start::connect::Leave::Blocked,
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
            space: None,
            model: None,
        }
    }

    #[test]
    fn the_order_must_run_at_the_root_its_claim_was_judged_at() {
        let pair = Pair::new("order");
        let root = pair.worktree.as_path();
        let order_in = |directory: &Path, project: Option<&Path>| {
            let mut order = order(claimed_with(&pair.list(), root, None), directory, project);
            let settled = settle(&mut order);
            (settled, order.trusted)
        };
        let (settled, trusted) = order_in(root, Some(root));
        assert!(settled.is_ok_and(|admitted| admitted.is_some()));
        assert_eq!(trusted.expect("its gate").path(), root);
        for (directory, project) in [
            (pair.main.as_path(), Some(root)),
            (root, Some(pair.main.as_path())),
            (root, None),
        ] {
            let (settled, trusted) = order_in(directory, project);
            assert_eq!(settled.map(|_| ()), Err(NoClaim::NotOne.said()));
            assert!(trusted.is_none());
        }
    }

    /// `pair`'s worktree as the start thread settles it, and what the UI keeps of it.
    fn settled(pair: &Pair) -> (Admitted, Order, Kept) {
        let root = pair.worktree.as_path();
        let claimed = claimed_with(&pair.list(), root, None);
        let kept = Judged::Claimed(claimed.clone()).kept().expect("kept");
        let mut order = order(claimed, root, Some(root));
        let admitted = settle(&mut order).expect("admitted").expect("by its claim");
        assert!(order.trusted.is_some(), "its gate");
        (admitted, order, kept)
    }

    /// The start thread's last look.
    fn last(admitted: Admitted, order: &Order) -> Result<(), String> {
        still_gated(&order.runs, None, Some(admitted), order.trusted.as_ref())
    }

    #[test]
    fn what_changed_between_the_look_and_the_spawn_refuses() {
        // Nothing did.
        let pair = Pair::at("same", "beside");
        let (admitted, order, _) = settled(&pair);
        assert_eq!(last(admitted, &order), Ok(()));
        let (admitted, order, _) = settled(&pair);
        // Its record in the main repository names another place.
        let gitdir = pair.main.join(".git/worktrees/wt/gitdir");
        fs::write(&gitdir, "/nowhere/.git\n").unwrap();
        assert_eq!(last(admitted, &order), Err(NoClaim::NotOne.said()));
        // Another directory at its place, which the repository records as well.
        let pair = Pair::at("swapped", "beside");
        let (admitted, order, _) = settled(&pair);
        fs::rename(&pair.worktree, pair.base.join("old")).unwrap();
        worktree_of(&pair.main, &pair.worktree);
        assert_eq!(last(admitted, &order), Err(NoClaim::NotOne.said()));
        // Another main repository.
        let pair = Pair::at("main-swapped", "beside");
        let (admitted, order, _) = settled(&pair);
        fs::rename(&pair.main, pair.base.join("old")).unwrap();
        fs::create_dir(&pair.main).unwrap();
        worktree_of(&pair.main, &pair.worktree);
        assert_eq!(last(admitted, &order), Err(NoClaim::NotOne.said()));
    }

    #[test]
    fn what_the_ui_takes_back_refuses_before_the_look_and_before_the_spawn() {
        let pair = Pair::new("taken-back");
        let (admitted, order, kept) = settled(&pair);
        kept.revoke();
        assert!(
            order.trusted.as_ref().unwrap().revoked(),
            "the root it made shares it"
        );
        assert_eq!(last(admitted, &order), Err(NoClaim::Revoked.said()));
        let claimed = claimed_with(&pair.list(), &pair.worktree, None);
        Judged::Claimed(claimed.clone()).kept().unwrap().revoke();
        assert_eq!(claimed.look().map(|_| ()), Err(NoClaim::Revoked));
    }
}
