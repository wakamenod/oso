//! Starting a review in a space whose orchestrator is recorded (ADR 0163): `review-layout`'s
//! menu there asks for the space's repository, by completion over its trusted members when it
//! has two or more, then `review-start`'s menu what of it to review: a pull request, a worktree
//! or this checkout. The project chosen is the review menu's target until the menu closes. A
//! pull request opens on the worktree its head is checked out in, found before opening.
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here runs git or looks at
//! the disk: the projects are found on the detector's thread, and a pull request's head on
//! `oso-review-head`.
//!
//! - A member's or worktree's project comes from the detector's own answer, which judged its
//!   trust as for a buffer's file (ADR 0114), and is added with `Editor::add_project`.
//! - Every step checks its answer against what the host listed: a repository of the space in
//!   front that oso trusts, a worktree of it the host knows.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use oso_core::complete::Candidate;
use oso_core::editor::{ReviewStart, ScopeKind};
use oso_core::minibuffer::PromptId;
use oso_core::project::{Found, ProjectId, RootIdentity, same_place};

use super::super::{App, UserEvent};
use super::{RESTRICTED, ReviewEvent, state};
use crate::commands::review::{
    MAX_NAME, REVIEW_REPOSITORIES_LIST, REVIEW_START_MENU, REVIEW_WORKTREES_LIST, one_line,
    pull_number, selected_project,
};
use crate::git::{Git, TrustedRoot, empty_hooks};
use crate::project::RootAnswer;
use crate::sessions::spaces::SpaceId;
use crate::sessions::spaces::stored::{Member, is_plain};

/// What a start in a space keeps between its steps, until it ends: once neither of its menus
/// nor a prompt a head of theirs opened is open, and nothing it asked for is on its way.
#[derive(Default)]
pub(in crate::app) struct Starting {
    /// The space in front the start began in: an answer that comes once another is in front
    /// is dropped.
    space: Option<SpaceId>,
    /// The repository chosen.
    chosen: Option<Chosen>,
    /// A root being found, and what follows once it is.
    finding: Option<Finding>,
    /// A pull request's head being looked for.
    head: Option<Head>,
    /// The worktrees `review-worktree` lists, the main checkout first.
    worktrees: Vec<PathBuf>,
    /// The review menu's target is the start's.
    targeted: bool,
    /// `review-start` was the menu open last, so that its `p` finds the head's worktree.
    from_start: bool,
    /// One of the start's menus was open at the last sync, so that a prompt open now is a
    /// head's.
    menu_seen: bool,
    /// The prompt a head of the start's menus opened, which still reads the target.
    head_prompt: Option<PromptId>,
    /// Which look for a head this is: answers of an earlier one are dropped.
    serial: u64,
    /// The projects of the repositories found so far, by root, kept across starts, so that a
    /// repository found once goes on at once. Each is taken only while it is trusted and its
    /// root is still the directory it was, by device and inode: where the system has none, the
    /// detector is asked again.
    known: Vec<(PathBuf, ProjectId)>,
}

/// The repository a start chose: its root and identity as the space holds them, and its
/// project once found.
struct Chosen {
    root: PathBuf,
    identity: Option<(u64, u64)>,
    project: Option<ProjectId>,
}

struct Finding {
    root: PathBuf,
    then: Then,
}

/// What follows once a root's project is found.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Then {
    /// The repository's: `review-start`'s menu.
    StartMenu,
    /// A worktree's: the review menu for it.
    ReviewMenu,
    /// A pull request's head's worktree: its review, of the pull request this number names.
    PullRequest(u64),
}

struct Head {
    serial: u64,
    /// The repository's project, which the review opens on when no worktree holds the head.
    project: ProjectId,
    number: u64,
    /// The root the look runs git in, revoked as its project stops being trusted.
    root: TrustedRoot,
    stop: Arc<AtomicBool>,
}

/// What `oso-review-head` found of a pull request's head.
#[derive(Debug)]
pub(crate) struct HeadFound {
    serial: u64,
    worktree: Option<PathBuf>,
}

impl App {
    /// Whether the space in front has its orchestrator recorded, where `review-layout`'s menu
    /// asks for the repository first: the dispatcher leaves that menu to the host there. A space
    /// whose repositories are all untrusted goes the host's way too, which then opens the
    /// review menu as before, as judging trust at each sync would cost more.
    pub(super) fn sync_review_start(&mut self) {
        let spaces = self.watch.spaces();
        let orchestrated = (spaces.current())
            .and_then(|at| spaces.stored(at))
            .is_some_and(|stored| stored.orchestrator.is_some() && !stored.members.is_empty());
        self.dispatcher.set_review_menu_by_host(orchestrated);
    }

    /// After the requests: the start's target stays while one of its menus is open, or the
    /// prompt a head of theirs opened; it goes as another prompt opens or once none is, and
    /// the start ends once nothing it asked for is on its way.
    pub(super) fn sync_review_target(&mut self) {
        let starting = &mut self.reviews.starting;
        if starting.chosen.is_none() && !starting.targeted {
            return;
        }
        let menu = self.dispatcher.menu_name();
        if matches!(menu, Some(REVIEW_START_MENU | "review")) {
            starting.menu_seen = true;
            starting.head_prompt = None;
            return;
        }
        let prompt = self.editor.prompt().map(oso_core::minibuffer::Prompt::id);
        if let Some(prompt) = prompt {
            if std::mem::take(&mut starting.menu_seen) {
                starting.head_prompt = Some(prompt);
            }
            if starting.head_prompt == Some(prompt) {
                return;
            }
        }
        starting.menu_seen = false;
        starting.head_prompt = None;
        if std::mem::take(&mut starting.targeted) {
            self.editor.set_review_target(None);
        }
        let starting = &self.reviews.starting;
        if starting.finding.is_none() && starting.head.is_none() {
            self.forget_review_start();
        }
    }

    /// `review-layout`'s menu in a space whose orchestrator is recorded: asks for the
    /// repository among its trusted ones, or takes the one there is. False, doing nothing,
    /// outside such a space or with no trusted repository, where the review menu opens as
    /// before.
    pub(super) fn start_review_in_space(&mut self) -> bool {
        let spaces = self.watch.spaces();
        let Some(at) = spaces.current() else {
            return false;
        };
        let Some(stored) = spaces.stored(at).filter(|s| s.orchestrator.is_some()) else {
            return false;
        };
        let trusted: Vec<(String, Member)> = (spaces.member_names(at).into_iter())
            .filter_map(|(name, root)| Some((name, stored.member(&root)?.clone())))
            .filter(|(_, member)| self.trusts(member))
            .collect();
        let used = self.hosting.spaces.used.get(&stored.id).cloned();
        if trusted.is_empty() {
            return false;
        }
        self.forget_review_start();
        self.reviews.starting.space = self.front_space_id();
        if let [(_, member)] = trusted.as_slice() {
            let member = member.clone();
            self.review_in_repository(member);
            return true;
        }
        let selected = (selected_project(&self.editor))
            .and_then(|id| self.editor.project(id))
            .map(|project| project.root().to_path_buf());
        let named: Vec<(String, PathBuf)> = (trusted.iter())
            .map(|(name, member)| (name.clone(), member.root.clone()))
            .collect();
        let default = default_repository(&named, selected.as_deref(), used.as_deref(), |p| {
            self.listed_repository_of(p)
        });
        let candidates: Arc<[Candidate]> = (named.iter())
            .map(|(name, root)| Candidate::new(name.as_str(), "", shown(root, self.editor.home())))
            .collect();
        self.editor.set_listed(REVIEW_REPOSITORIES_LIST, candidates);
        let Some(prompt) = self.open_host_prompt("review-repository") else {
            self.editor.message("Cannot ask for the repository now");
            return true;
        };
        // The repository's prompt reads no target, but its answer goes on with the start.
        self.reviews.starting.head_prompt = Some(prompt);
        if let Some(name) = default {
            self.editor.offer_prompt_default(prompt, name);
            let _ = self.editor.walk_history(false);
        }
        true
    }

    /// What a step of a start asked.
    pub(super) fn review_start(&mut self, step: ReviewStart) {
        match step {
            ReviewStart::Repository(name) => self.start_repository(&name),
            ReviewStart::Worktree(path) => self.start_worktree(&path),
            ReviewStart::Checkout => self.start_checkout(),
        }
    }

    fn start_repository(&mut self, name: &str) {
        let shown = one_line(name, MAX_NAME);
        let spaces = self.watch.spaces();
        let member = (spaces.current())
            .and_then(|at| spaces.stored(at))
            .filter(|stored| stored.orchestrator.is_some())
            .zip(spaces.member_named(name))
            .and_then(|(stored, root)| stored.member(&root).cloned());
        let Some(member) = member else {
            return (self.editor).message(format!("The space has no repository {shown}"));
        };
        if !self.trusts(&member) {
            return (self.editor).message(format!(
                "{shown} is not trusted in oso: trust-project first"
            ));
        }
        self.forget_review_start();
        self.reviews.starting.space = self.front_space_id();
        self.review_in_repository(member);
    }

    /// The repository `member` is chosen: its worktrees are listed, and its project found.
    fn review_in_repository(&mut self, member: Member) {
        let worktrees: Vec<PathBuf> = (self.repository_worktrees(&member.root).into_iter())
            .filter(|path| path.to_str().is_some_and(|text| text.chars().all(is_plain)))
            .collect();
        let candidates: Arc<[Candidate]> = (worktrees.iter())
            .filter_map(|path| Some(Candidate::new(path.to_str()?, "", "")))
            .collect();
        self.editor.set_listed(REVIEW_WORKTREES_LIST, candidates);
        let starting = &mut self.reviews.starting;
        starting.worktrees = worktrees;
        starting.chosen = Some(Chosen {
            root: member.root.clone(),
            identity: member.identity,
            project: None,
        });
        self.find_review_root(member.root, member.identity, Then::StartMenu);
    }

    /// The chosen repository, while it is still a trusted repository of the space in front
    /// the start began in.
    fn still_chosen(&self) -> Option<&Chosen> {
        let starting = &self.reviews.starting;
        let chosen = starting.chosen.as_ref()?;
        if starting.space.is_none() || starting.space != self.front_space_id() {
            return None;
        }
        let spaces = self.watch.spaces();
        let stored = spaces.stored(spaces.current()?)?;
        let member = stored.member(&chosen.root)?;
        self.trusts(member).then_some(chosen)
    }

    fn start_worktree(&mut self, text: &str) {
        let path = PathBuf::from(text);
        let Some(chosen) = self.still_chosen() else {
            return self.editor.message(NOT_CHOSEN);
        };
        let (main, project) = (chosen.root.clone(), chosen.project);
        let worktrees = &self.reviews.starting.worktrees;
        if !worktrees.iter().any(|w| same_place(w, &path)) {
            let shown = one_line(text, MAX_NAME);
            return (self.editor).message(format!("No worktree {shown} is listed"));
        }
        match project {
            Some(project) if same_place(&main, &path) => self.review_menu_for(project),
            _ => self.find_review_root(path, None, Then::ReviewMenu),
        }
    }

    /// `.`: the selected buffer's worktree of the repository chosen, else its main checkout.
    fn start_checkout(&mut self) {
        let Some(main) = self.still_chosen().and_then(|chosen| chosen.project) else {
            return self.editor.message(NOT_CHOSEN);
        };
        let worktrees = &self.reviews.starting.worktrees;
        let selected = selected_project(&self.editor).filter(|&id| {
            self.editor.project_trusted(id)
                && (self.editor.project(id))
                    .is_some_and(|p| worktrees.iter().any(|w| same_place(w, p.root())))
        });
        self.review_menu_for(selected.unwrap_or(main));
    }

    /// The review menu, for `project`, if it opens.
    fn review_menu_for(&mut self, project: ProjectId) {
        if self.editor.prompt().is_some() {
            return self.editor.message("A prompt is open: C-x v r asks again");
        }
        self.editor.set_review_target(Some(project));
        let starting = &mut self.reviews.starting;
        starting.targeted = true;
        starting.from_start = false;
        self.open_review_menu_now();
    }

    /// `review-start`'s menu, for the repository's project, if it opens.
    fn start_menu_for(&mut self, project: ProjectId) {
        if self.editor.prompt().is_some() {
            return self.editor.message("A prompt is open: C-x v r asks again");
        }
        if let Some(chosen) = &mut self.reviews.starting.chosen {
            chosen.project = Some(project);
        }
        self.editor.set_review_target(Some(project));
        let starting = &mut self.reviews.starting;
        starting.targeted = true;
        starting.from_start = true;
        let effects = (self.dispatcher).open_menu_named(&mut self.editor, REVIEW_START_MENU);
        if effects.redraw {
            self.request_redraw();
        }
    }

    /// Goes on from `root`'s project: at once if a start found it before, it is still trusted
    /// and its root is still the directory `identity` names, else once the detector finds it.
    fn find_review_root(&mut self, root: PathBuf, identity: Option<(u64, u64)>, then: Then) {
        let editor = &self.editor;
        let known = (self.reviews.starting.known.iter())
            .find(|(known, _)| *known == root)
            .map(|(_, id)| *id)
            .filter(|&id| {
                let project = editor.project(id);
                editor.project_trusted(id)
                    && identity.is_some()
                    && project.is_some_and(|p| p.identity() == identity)
            });
        if let Some(project) = known {
            return self.review_root_found(&root, then, Some(project));
        }
        let trusted = self.trusted_list();
        self.projects.find_root(root.clone(), trusted);
        self.reviews.starting.finding = Some(Finding { root, then });
    }

    /// The detector found the roots `found`: the start's goes on from its project, which must
    /// be the chosen repository's or one of its worktrees, by its root or the main repository
    /// its claim was checked against.
    pub(in crate::app) fn review_roots_found(&mut self, found: Vec<RootAnswer>) {
        for (root, found) in found {
            let front = self.front_space_id();
            let starting = &mut self.reviews.starting;
            let Some(finding) = starting.finding.take_if(|f| f.root == root) else {
                continue;
            };
            if starting.space.is_none() || starting.space != front {
                continue;
            }
            let Some(chosen) = self.reviews.starting.chosen.as_ref() else {
                continue;
            };
            let (main, identity) = (chosen.root.clone(), chosen.identity);
            let found = found.filter(|found| belongs(found, &main, identity));
            let project = found.map(|found| self.editor.add_project(found));
            if let (Some(project), Then::StartMenu) = (project, &finding.then) {
                let known = &mut self.reviews.starting.known;
                known.retain(|(known, _)| *known != root);
                known.push((root.clone(), project));
            }
            self.review_root_found(&root, finding.then, project);
        }
    }

    /// What follows once `root`'s project is known to be `project`, or none.
    fn review_root_found(&mut self, root: &Path, then: Then, project: Option<ProjectId>) {
        let main = (self.reviews.starting.chosen.as_ref()).and_then(|chosen| chosen.project);
        let trusted = project.filter(|&id| self.editor.project_trusted(id));
        match (then, trusted) {
            (Then::StartMenu, Some(project)) => self.start_menu_for(project),
            (Then::ReviewMenu, Some(project)) => self.review_menu_for(project),
            (Then::PullRequest(number), Some(project)) => {
                self.open_review(project, ScopeKind::PullRequest, Some(number.to_string()));
            }
            // The head's worktree is no project oso trusts: the main checkout shows its tree.
            (Then::PullRequest(number), None) => {
                if let Some(main) = main {
                    self.open_review(main, ScopeKind::PullRequest, Some(number.to_string()));
                }
            }
            (_, None) if project.is_some() => self.editor.message(RESTRICTED),
            (_, None) => {
                let shown = shown(root, self.editor.home());
                (self.editor).message(format!(
                    "{shown} is no repository or worktree of it oso finds"
                ));
            }
        }
    }

    /// A pull request asked for from `review-start`'s menu: its head's worktree is found
    /// first, and the review opens on it. False for any other open, which goes as before.
    pub(super) fn open_started_pull_request(
        &mut self,
        project: ProjectId,
        number: Option<&str>,
    ) -> bool {
        let starting = &self.reviews.starting;
        let ours = starting.targeted
            && starting.from_start
            && self.editor.review_target() == Some(project)
            && starting.chosen.as_ref().and_then(|c| c.project) == Some(project);
        let Some(number) = number.and_then(pull_number).filter(|_| ours) else {
            return false;
        };
        let (Some(root), Some(state)) = (TrustedRoot::of(&self.editor, project), state()) else {
            return false;
        };
        self.stop_head_look();
        let starting = &mut self.reviews.starting;
        starting.serial += 1;
        let serial = starting.serial;
        let stop = Arc::new(AtomicBool::new(false));
        let waker = self.waker.clone();
        let (stopped, looked) = (Arc::clone(&stop), root.clone());
        let started = std::thread::Builder::new()
            .name("oso-review-head".into())
            .spawn(move || {
                let worktree = find_head(&looked, &state, number, &stopped);
                let found = HeadFound { serial, worktree };
                waker.send(UserEvent::Review(ReviewEvent::Head(found)));
            });
        if let Err(e) = started {
            log::warn!("cannot look for a pull request's head: {e}");
            return false;
        }
        starting.head = Some(Head {
            serial,
            project,
            number,
            root,
            stop,
        });
        self.editor
            .message(format!("Finding #{number}'s worktree\u{2026}"));
        true
    }

    /// `oso-review-head` found where a pull request's head is checked out: the review opens on
    /// that worktree's project, else on the repository's.
    pub(super) fn review_head_found(&mut self, found: HeadFound) {
        let front = self.front_space_id();
        let starting = &mut self.reviews.starting;
        let Some(head) = starting.head.take_if(|head| head.serial == found.serial) else {
            return;
        };
        if starting.space.is_none() || starting.space != front {
            return;
        }
        let main = (self.editor.project(head.project)).map(|p| p.root().to_path_buf());
        let number = Some(head.number.to_string());
        match found.worktree {
            Some(worktree)
                if main
                    .as_deref()
                    .is_none_or(|main| !same_place(main, &worktree)) =>
            {
                self.find_review_root(worktree, None, Then::PullRequest(head.number));
            }
            _ => self.open_review(head.project, ScopeKind::PullRequest, number),
        }
    }

    /// Revokes the root of a head's look whose project is no longer trusted, so that it runs no
    /// more git, before anything else runs (ADR 0122, "Restricted projects").
    pub(super) fn revoke_untrusted_start(&mut self) {
        let untrusted = (self.reviews.starting.head.as_ref())
            .is_some_and(|head| !self.editor.project_trusted(head.project));
        if untrusted {
            self.stop_head_look();
        }
    }

    fn stop_head_look(&mut self) {
        if let Some(head) = self.reviews.starting.head.take() {
            head.root.revoke();
            head.stop.store(true, Ordering::SeqCst);
        }
    }

    /// Drops what a start kept and its lists, as another begins or it ends; the projects found
    /// stay, and the serial goes on, so that an answer of a look just stopped is dropped.
    fn forget_review_start(&mut self) {
        self.stop_head_look();
        let starting = std::mem::take(&mut self.reviews.starting);
        if starting.targeted {
            self.editor.set_review_target(None);
        }
        self.reviews.starting.serial = starting.serial;
        self.reviews.starting.known = starting.known;
        self.editor
            .set_listed(REVIEW_REPOSITORIES_LIST, Arc::from([]));
        self.editor.set_listed(REVIEW_WORKTREES_LIST, Arc::from([]));
    }

    /// The worktrees of the repository at `root` a start lists: its main checkout, those git
    /// listed, and those the open spaces hold in it.
    fn repository_worktrees(&self, root: &Path) -> Vec<PathBuf> {
        let listed = self.listed_worktrees_of(root);
        let held = self.held_worktrees_in(root);
        worktrees_of(root, &listed, &held)
    }
}

/// Whether `found` is the repository at `root` with `identity`, or a worktree whose claim was
/// checked against it: by device and inode where both are known, else by path.
fn belongs(found: &Found, root: &Path, identity: Option<(u64, u64)>) -> bool {
    let same = |known: &RootIdentity| match (known.identity, identity) {
        (Some(a), Some(b)) => a == b,
        _ => same_place(&known.path, root),
    };
    same(&found.root) || found.checked_main.as_ref().is_some_and(same)
}

/// `path` as a candidate's annotation or a message shows it: under the home as `~`, its last
/// 60 characters, its control and invisible characters escaped.
fn shown(path: &Path, home: Option<&Path>) -> String {
    crate::sessions::recap::clean(&crate::commands::shown_place(path, home, 60))
}

/// Why a step after the repository's has nothing to go on.
const NOT_CHOSEN: &str = "No repository was chosen: C-x v r asks";

/// Where pull request `number`'s head is checked out in the repository at `root`, on
/// `oso-review-head`; `None` on any failure, which the review then says itself.
fn find_head(root: &TrustedRoot, state: &Path, number: u64, stop: &AtomicBool) -> Option<PathBuf> {
    let look = || -> Result<Option<PathBuf>, String> {
        root.check()?;
        let hooks = empty_hooks(state, "review-head")?;
        let git = Git::new(root.path(), &hooks).map_err(|e| e.to_string())?;
        let stopped = || stop.load(Ordering::SeqCst) || root.revoked();
        crate::review::pr::head_worktree(&git, state, number, &stopped)
            .map_err(|e| format!("{e:?}"))
    };
    match look() {
        Ok(found) => found,
        Err(why) => {
            log::debug!("a pull request's head was not looked for: {why}");
            None
        }
    }
}

/// The repository `review-repository` offers first, of `trusted` by name: the one the
/// selected buffer is in, as its root or a worktree git listed under it, else the one
/// `space-use-repo` chose.
fn default_repository(
    trusted: &[(String, PathBuf)],
    selected: Option<&Path>,
    used: Option<&Path>,
    listed_under: impl Fn(&Path) -> Option<PathBuf>,
) -> Option<String> {
    let named = |root: &Path| {
        (trusted.iter())
            .find(|(_, member)| same_place(member, root))
            .map(|(name, _)| name.clone())
    };
    let from_selected = selected.and_then(|selected| {
        named(selected).or_else(|| listed_under(selected).and_then(|repo| named(&repo)))
    });
    from_selected.or_else(|| used.and_then(named))
}

/// The worktrees of the repository at `root`: the main checkout first, then those git listed,
/// then those held in it, each once.
fn worktrees_of(root: &Path, listed: &[PathBuf], held: &[PathBuf]) -> Vec<PathBuf> {
    let mut all: Vec<PathBuf> = vec![root.to_path_buf()];
    for path in listed.iter().chain(held) {
        if !all.iter().any(|seen| same_place(seen, path)) {
            all.push(path.clone());
        }
    }
    all
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use oso_core::project::{Found, RootIdentity};

    use super::{belongs, default_repository, worktrees_of};

    fn trusted() -> Vec<(String, PathBuf)> {
        vec![
            ("front".into(), PathBuf::from("/w/front")),
            ("back".into(), PathBuf::from("/w/back")),
        ]
    }

    #[test]
    fn the_repository_offered_is_the_selected_buffer_s_else_the_one_in_use() {
        let none = |_: &Path| None;
        let trusted = trusted();
        let pick = |selected: Option<&str>, used: Option<&str>| {
            default_repository(
                &trusted,
                selected.map(Path::new),
                used.map(Path::new),
                |path: &Path| (path == Path::new("/w/back-wt")).then(|| PathBuf::from("/w/back")),
            )
        };
        assert_eq!(
            pick(Some("/w/back"), Some("/w/front")).as_deref(),
            Some("back")
        );
        assert_eq!(pick(Some("/w/back-wt"), None).as_deref(), Some("back"));
        assert_eq!(
            pick(Some("/elsewhere"), Some("/w/front")).as_deref(),
            Some("front")
        );
        assert_eq!(pick(None, None), None);
        assert_eq!(
            default_repository(&trusted, None, Some(Path::new("/gone")), none),
            None
        );
    }

    #[test]
    fn the_worktrees_listed_are_the_main_checkout_then_each_other_once() {
        let root = Path::new("/w/front");
        let listed = [PathBuf::from("/w/front"), PathBuf::from("/w/front-a")];
        let held = [PathBuf::from("/w/front-a"), PathBuf::from("/w/front/.wt/b")];
        assert_eq!(
            worktrees_of(root, &listed, &held),
            [
                PathBuf::from("/w/front"),
                PathBuf::from("/w/front-a"),
                PathBuf::from("/w/front/.wt/b"),
            ]
        );
        assert_eq!(worktrees_of(root, &[], &[]), [PathBuf::from("/w/front")]);
    }

    fn root(path: &str, identity: Option<(u64, u64)>) -> RootIdentity {
        RootIdentity {
            path: PathBuf::from(path),
            identity,
        }
    }

    #[test]
    fn a_root_found_belongs_to_the_repository_as_itself_or_its_checked_worktree() {
        let main = Path::new("/w/front");
        let at = |path: &str, inode: u64, checked: Option<RootIdentity>| Found {
            root: root(path, Some((1, inode))),
            worktree_of: checked.as_ref().map(|main| main.path.clone()),
            checked_main: checked,
        };
        // By device and inode where both are known.
        assert!(belongs(
            &at("/private/w/front", 7, None),
            main,
            Some((1, 7))
        ));
        assert!(!belongs(&at("/w/front", 8, None), main, Some((1, 7))));
        let checked = Some(root("/w/front", Some((1, 7))));
        assert!(belongs(&at("/w/front-wt", 9, checked), main, Some((1, 7))));
        // A worktree of another repository, or one whose claim was not checked, is not its.
        let other = Some(root("/w/back", Some((1, 3))));
        assert!(!belongs(&at("/w/back-wt", 9, other), main, Some((1, 7))));
        assert!(!belongs(&at("/w/front-wt", 9, None), main, Some((1, 7))));
        // By path where an identity is missing.
        assert!(belongs(&at("/w/front", 8, None), main, None));
    }
}
