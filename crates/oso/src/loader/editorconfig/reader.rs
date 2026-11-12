//! The thread that reads EditorConfig again after `C-x C-w` names a file anew (ADR 0115):
//! started with the first request, it answers each in turn, as the thread that finds
//! projects does.
//!
//! Must not depend on: `app`, `config` or the UI thread; the caller supplies the wake-up.

use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;

use oso_core::editor::ProjectRequest;

use super::{EditorConfigFound, Home, read};

/// A request answered: what the files above its file give it.
type Answer = (ProjectRequest, EditorConfigFound);

/// The thread that reads EditorConfig after `C-x C-w`.
pub struct Reader {
    home: Home,
    wake: Arc<dyn Fn() -> bool + Send + Sync>,
    requests: Option<Sender<ProjectRequest>>,
    answers: Arc<Mutex<Vec<Answer>>>,
}

impl Reader {
    /// A reader whose walks end after `home`, and which calls `wake` when answers wait after
    /// none did. `wake` returns false once the UI is gone, which ends the thread.
    pub fn new(home: Home, wake: impl Fn() -> bool + Send + Sync + 'static) -> Self {
        Self {
            home,
            wake: Arc::new(wake),
            requests: None,
            answers: Arc::default(),
        }
    }

    /// Reads the EditorConfig of `request`'s file, on the thread.
    pub fn read(&mut self, request: ProjectRequest) {
        if self.requests.is_none() {
            self.requests = self.start();
        }
        if let Some(requests) = &self.requests
            && requests.send(request).is_err()
        {
            self.requests = None;
        }
    }

    /// The answers that came since the last call.
    pub fn take(&self) -> Vec<Answer> {
        std::mem::take(&mut *lock(&self.answers))
    }

    fn start(&self) -> Option<Sender<ProjectRequest>> {
        let (requests, received) = mpsc::channel::<ProjectRequest>();
        let answers = Arc::clone(&self.answers);
        let wake = Arc::clone(&self.wake);
        let home = self.home.clone();
        let started = thread::Builder::new()
            .name("oso-editorconfig".into())
            .spawn(move || {
                for request in received {
                    // A panic reads nothing, and the buffer keeps what it has.
                    let Ok(found) =
                        std::panic::catch_unwind(|| read(&request.location, &home, &|| false))
                    else {
                        continue;
                    };
                    let was_empty = {
                        let mut answers = lock(&answers);
                        answers.push((request, found));
                        answers.len() == 1
                    };
                    if was_empty && !wake() {
                        return;
                    }
                }
            });
        match started {
            Ok(_) => Some(requests),
            Err(e) => {
                log::warn!("cannot start reading EditorConfig: {e}");
                None
            }
        }
    }
}

fn lock(answers: &Mutex<Vec<Answer>>) -> std::sync::MutexGuard<'_, Vec<Answer>> {
    answers.lock().unwrap_or_else(PoisonError::into_inner)
}
