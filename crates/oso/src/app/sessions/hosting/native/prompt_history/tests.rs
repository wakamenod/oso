//! Whether the history is kept, and the thread that keeps it.
//!
//! Must not depend on: the window.

use super::*;

#[test]
fn it_is_kept_once_the_configuration_says_so_and_stays_off_once_off() {
    assert_eq!(Kept::Unknown.after(None), Kept::Unknown);
    assert_eq!(Kept::Unknown.after(Some(true)), Kept::Yes);
    assert_eq!(Kept::Unknown.after(Some(false)), Kept::No);
    assert_eq!(Kept::Yes.after(Some(false)), Kept::No);
    assert_eq!(Kept::No.after(Some(true)), Kept::No);
    assert_eq!(Kept::Yes.after(None), Kept::Yes);
}

/// The thread over a directory of its own.
#[cfg(unix)]
struct Thread {
    directory: std::path::PathBuf,
    jobs: Option<Sender<Job>>,
    events: Receiver<PromptHistoryEvent>,
    thread: Option<thread::JoinHandle<()>>,
}

#[cfg(unix)]
impl Thread {
    fn start() -> Self {
        use std::os::unix::fs::PermissionsExt as _;
        let test = thread::current();
        let test = test.name().unwrap_or("test").replace("::", "-");
        let name = format!("oso-prompts-{}-{test}", std::process::id());
        let directory = std::fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(name);
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir(&directory).unwrap();
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
        let (jobs, waiting) = mpsc::channel();
        let (sender, events) = mpsc::channel();
        let at = directory.clone();
        let thread = thread::spawn(move || {
            run(&at, &waiting, |event| {
                let _ = sender.send(event);
            });
        });
        Self {
            directory,
            jobs: Some(jobs),
            events,
            thread: Some(thread),
        }
    }

    fn send(&self, job: Job) {
        self.jobs.as_ref().unwrap().send(job).unwrap();
    }

    fn write(&self, sent: &[&str], clears: bool) {
        let mut history = History::default();
        for text in sent {
            history.note(text, 1);
        }
        if clears {
            history.clear(now());
        }
        self.send(Job::Write { history, clears });
    }

    /// Whether all it was handed is written.
    fn flush(&self) {
        let (done, answer) = mpsc::sync_channel(1);
        self.send(Job::Flush(done));
        assert!(answer.recv_timeout(Duration::from_secs(5)).unwrap());
    }

    fn texts(&self) -> Vec<String> {
        (file::read(&self.directory).unwrap().entries().iter())
            .map(|entry| entry.text.to_string())
            .collect()
    }
}

#[cfg(unix)]
impl Drop for Thread {
    fn drop(&mut self) {
        self.jobs = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

#[cfg(unix)]
#[test]
fn a_flush_writes_at_once_the_newest_and_what_is_read_is_told() {
    let thread = Thread::start();
    thread.write(&["a"], false);
    thread.write(&["a", "b"], false);
    thread.flush();
    assert_eq!(thread.texts(), ["b", "a"]);
    thread.send(Job::Read);
    for _ in 0..2 {
        let event = thread.events.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(event, PromptHistoryEvent::Known(h) if h.entries().len() == 2));
    }
}

#[cfg(unix)]
#[test]
fn what_waits_is_dropped_as_it_is_turned_off_and_a_clear_is_not() {
    let thread = Thread::start();
    thread.write(&["a"], false);
    thread.send(Job::Drop);
    thread.flush();
    assert!(thread.texts().is_empty());
    thread.write(&["b"], false);
    thread.flush();
    thread.write(&[], true);
    thread.send(Job::Drop);
    thread.flush();
    assert!(thread.texts().is_empty());
    assert!(thread.directory.join("prompt-history").exists());
}

#[cfg(unix)]
#[test]
fn a_file_that_may_not_be_read_is_told_and_a_clear_starts_it_again() {
    let thread = Thread::start();
    let file = thread.directory.join("prompt-history");
    std::fs::write(&file, "{\"version\":9}\n").unwrap();
    thread.send(Job::Read);
    thread.write(&["a"], false);
    thread.flush();
    let events = || thread.events.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(matches!(events(), PromptHistoryEvent::NotRead(_)));
    assert!(matches!(events(), PromptHistoryEvent::NotWritten(_)));
    thread.write(&[], true);
    assert!(matches!(events(), PromptHistoryEvent::Cleared(_)));
    thread.write(&["b"], false);
    thread.flush();
    assert!(thread.texts().is_empty(), "sent before the clear");
}

#[cfg(unix)]
#[test]
fn a_clear_another_instance_holds_up_is_told_and_takes_no_message_with_it_once_off() {
    let thread = Thread::start();
    let lock = thread.directory.join("prompt-history.lock");
    let held = std::fs::File::create(&lock).unwrap();
    held.lock().unwrap();
    thread.write(&[], true);
    let event = thread.events.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(matches!(event, PromptHistoryEvent::NotCleared(_)));
    thread.send(Job::Write {
        history: {
            let mut history = History::default();
            history.note("b", now() + 9999);
            history
        },
        clears: false,
    });
    thread.send(Job::Drop);
    drop(held);
    thread.flush();
    assert!(thread.texts().is_empty());
}
