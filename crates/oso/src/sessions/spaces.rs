//! Spaces (ADR 0121, Q8, step 3): a project root and its sessions, a worktree's nested under its
//! repository's. Each keeps its own window tree, swapped whole into the frame as a tab would be,
//! its zoom, and the session and the file buffer it showed last, for its first entry.
//!
//! Must not depend on: `app`, `dispatch` or `config`; it acts on the editor through
//! oso-core's window trees alone, and is given each space's root and repository: whether a
//! worktree belongs to a repository is ADR 0114's claim, which the app checks off this thread.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use oso_core::buffer::BufferId;
use oso_core::complete::Candidate;
use oso_core::editor::{Editor, WindowTree};

/// One space.
#[derive(Debug)]
pub struct Space {
    pub root: PathBuf,
    /// `repo`, or `repo/slug` for a worktree.
    pub name: String,
    /// The repository a worktree's space is nested under.
    pub repo: Option<PathBuf>,
    /// Its window tree while another space's is in the frame.
    tree: Option<WindowTree>,
    /// Its tree from before `space-zoom`, while zoomed.
    zoom: Option<WindowTree>,
    /// Whether it has had a tree: a space never entered gets one from its last buffers.
    entered: bool,
    pub last_session: Option<BufferId>,
    pub last_file: Option<BufferId>,
}

/// The editor has a prompt open, so the frame's tree stays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Refused;

/// What `space-zoom` did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Zoom {
    In,
    Out,
}

/// Every space oso knows, and the one whose tree is in the frame.
#[derive(Debug, Default)]
pub struct Spaces {
    spaces: Vec<Space>,
    current: Option<usize>,
    /// The tree from before `space-zoom` while no space is current.
    zoom: Option<WindowTree>,
}

impl Spaces {
    /// The space of `root`, added if new, nested under `repo`'s if a worktree of it.
    pub fn add(&mut self, root: &Path, repo: Option<&Path>) -> usize {
        if let Some(at) = self.find(root) {
            return at;
        }
        let base = |path: &Path| {
            path.file_name().map_or_else(
                || path.display().to_string(),
                |name| name.to_string_lossy().into_owned(),
            )
        };
        let mut name = match repo {
            Some(repo) => format!("{}/{}", base(repo), base(root)),
            None => base(root),
        };
        // Two projects of one name: the later is told apart, as a buffer's name is.
        let first = name.clone();
        let mut n = 2;
        while self.named(&name).is_some() {
            name = format!("{first}<{n}>");
            n += 1;
        }
        self.spaces.push(Space {
            root: root.to_owned(),
            name,
            repo: repo.map(Path::to_owned),
            tree: None,
            zoom: None,
            entered: false,
            last_session: None,
            last_file: None,
        });
        self.spaces.len() - 1
    }

    pub fn find(&self, root: &Path) -> Option<usize> {
        self.spaces.iter().position(|space| space.root == root)
    }

    /// The space whose root holds `path`, the deepest if several do.
    pub fn of_path(&self, path: &Path) -> Option<usize> {
        (0..self.spaces.len())
            .filter(|&at| path.starts_with(&self.spaces[at].root))
            .max_by_key(|&at| self.spaces[at].root.components().count())
    }

    pub fn get(&self, at: usize) -> Option<&Space> {
        self.spaces.get(at)
    }

    pub fn get_mut(&mut self, at: usize) -> Option<&mut Space> {
        self.spaces.get_mut(at)
    }

    pub fn current(&self) -> Option<usize> {
        self.current
    }

    /// Space `at` is the one whose tree is in the frame, as it is: oso's first space. A zoom
    /// made before there was a space is the space's.
    pub fn adopt(&mut self, at: usize) {
        if let Some(space) = self.spaces.get_mut(at) {
            space.entered = true;
            if self.current.is_none() && space.zoom.is_none() {
                space.zoom = self.zoom.take();
            }
            self.current = Some(at);
        }
    }

    /// Shows space `at`'s tree, keeping the frame's in the space it leaves. A space entered for
    /// the first time shows its last file buffer and its last session side by side, or the one
    /// of them it has; with neither, the frame's tree becomes its own, with its zoom, and the
    /// space left gets one from its last buffers when it is entered again. With no space yet,
    /// the frame's tree becomes `at`'s.
    ///
    /// # Errors
    ///
    /// [`Refused`] while a prompt is open: nothing changed.
    pub fn enter(&mut self, editor: &mut Editor, at: usize) -> Result<(), Refused> {
        let Some(from) = self.current else {
            self.adopt(at);
            return Ok(());
        };
        if from == at || at >= self.spaces.len() {
            return Ok(());
        }
        if editor.prompt().is_some() {
            return Err(Refused);
        }
        let target = &mut self.spaces[at];
        let tree = match target.tree.take() {
            Some(tree) => Some(tree),
            None if target.entered => None,
            None => {
                let shown: Vec<BufferId> = [target.last_file, target.last_session]
                    .into_iter()
                    .flatten()
                    .filter(|&id| editor.buffer(id).is_some())
                    .collect();
                (!shown.is_empty()).then(|| editor.window_tree_showing(&shown))
            }
        };
        match tree {
            Some(tree) => match editor.swap_window_tree(tree) {
                Ok(old) => self.spaces[from].tree = Some(old),
                Err(tree) => {
                    let target = &mut self.spaces[at];
                    if target.entered {
                        target.tree = Some(tree);
                    }
                    return Err(Refused);
                }
            },
            None => {
                let from = &mut self.spaces[from];
                from.entered = false;
                let zoom = from.zoom.take();
                self.spaces[at].zoom = zoom;
            }
        }
        self.spaces[at].entered = true;
        self.current = Some(at);
        Ok(())
    }

    /// Closes space `at`, as `worktree-remove` does for the worktree it removed (ADR 0121). If its
    /// tree is in the frame, its repository's space is entered first, else the first other space;
    /// with no other space, no space is current and the frame keeps its tree. Each later space
    /// moves down one place, and the current one with it.
    ///
    /// # Errors
    ///
    /// [`Refused`] while a prompt is open and the space to close is in the frame: nothing
    /// changed.
    pub(crate) fn remove(&mut self, editor: &mut Editor, at: usize) -> Result<(), Refused> {
        let Some(space) = self.spaces.get(at) else {
            return Ok(());
        };
        if self.current == Some(at) {
            let parent = space.repo.as_deref().and_then(|repo| self.find(repo));
            let fallback = parent
                .filter(|&to| to != at)
                .or_else(|| (0..self.spaces.len()).find(|&to| to != at));
            match fallback {
                Some(to) => self.enter(editor, to)?,
                None => {
                    self.current = None;
                    self.zoom = self.spaces[at].zoom.take();
                }
            }
        }
        self.spaces.remove(at);
        if let Some(current) = self.current.filter(|&current| current > at) {
            self.current = Some(current - 1);
        }
        Ok(())
    }

    /// `space-zoom`: the tree's selected window alone, and back, with that window's buffer and
    /// places put in the tree's window it was zoomed from. With the side window selected, the
    /// tree's window is selected first.
    ///
    /// # Errors
    ///
    /// [`Refused`] while a prompt is open.
    pub fn zoom(&mut self, editor: &mut Editor) -> Result<Zoom, Refused> {
        if editor.prompt().is_some() {
            return Err(Refused);
        }
        let side = editor.side_window();
        if side == Some(editor.window().id()) {
            editor.other_window(1);
        }
        let slot = match self.current {
            Some(at) => &mut self.spaces[at].zoom,
            None => &mut self.zoom,
        };
        let Some(saved) = slot.take() else {
            let lone = editor.lone_window_tree();
            let saved = editor.swap_window_tree(lone).map_err(|_| Refused)?;
            *slot = Some(saved);
            return Ok(Zoom::In);
        };
        let window = editor.window();
        let (buffer, selections, scroll) =
            (window.buffer(), window.selections.clone(), window.scroll());
        if let Err(saved) = editor.swap_window_tree(saved) {
            *slot = Some(saved);
            return Err(Refused);
        }
        let back = editor.window().id();
        editor.show_in(back, buffer);
        editor.restore_positions_in(back, selections, scroll);
        Ok(Zoom::Out)
    }

    /// The candidates of `space-switch`: each space's name, a worktree's after its repository's,
    /// with its root as the annotation.
    pub fn candidates(&self) -> Arc<[Candidate]> {
        let mut order: Vec<&Space> = self.spaces.iter().collect();
        order.sort_by(|a, b| a.name.cmp(&b.name));
        order
            .into_iter()
            .map(|space| Candidate::new(space.name.as_str(), "", space.root.display().to_string()))
            .collect()
    }

    /// The space a `space-switch` answer names.
    pub fn named(&self, name: &str) -> Option<usize> {
        self.spaces.iter().position(|space| space.name == name)
    }
}

#[cfg(test)]
mod tests;
