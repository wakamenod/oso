//! The tests of listing directories, on scratch directories.
//!
//! Must not depend on: `app`, winit or the GPU.

use std::ffi::OsString;
use std::fs;
use std::sync::mpsc;
use std::time::Duration;

use super::*;

/// A scratch directory, removed at the end of the test.
struct Scratch(PathBuf);

impl Scratch {
    fn new(test: &str) -> Self {
        let name = format!("oso-lister-test-{}-{test}", std::process::id());
        let path = std::env::temp_dir().join(name);
        let _ = fs::remove_dir_all(&path);
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn file(&self, name: &str, content: &[u8]) {
        fs::write(self.0.join(name), content).unwrap();
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A lister and what tells the test an answer came.
fn lister() -> (Lister, mpsc::Receiver<()>) {
    let (tx, rx) = mpsc::channel();
    let tx = Mutex::new(tx);
    (Lister::new(move || lock(&tx).send(()).is_ok()), rx)
}

/// Waits for `count` answers.
fn answers(lister: &Lister, rx: &mpsc::Receiver<()>, count: usize) -> Vec<Answer> {
    let mut answers = Vec::new();
    while answers.len() < count {
        rx.recv_timeout(Duration::from_secs(10)).expect("an answer");
        answers.extend(lister.take());
    }
    answers
}

#[cfg(unix)]
fn names(entries: &[Entry]) -> Vec<(String, Kind)> {
    let mut names: Vec<_> = entries
        .iter()
        .map(|entry| (entry.name.to_string_lossy().into_owned(), entry.kind))
        .collect();
    names.sort_by(|a, b| a.0.cmp(&b.0));
    names
}

#[cfg(unix)]
#[test]
fn a_directory_is_listed_with_each_names_kind_and_its_generation() {
    let scratch = Scratch::new("lister-kinds");
    scratch.file("file.md", b"x");
    fs::create_dir(scratch.path().join("dir")).unwrap();
    std::os::unix::fs::symlink("file.md", scratch.path().join("link")).unwrap();
    let (lister, rx) = lister();
    lister.list(scratch.path().to_path_buf(), 7);
    let [
        Answer::Listed {
            directory,
            generation,
            entries,
            truncated,
            partial,
        },
    ] = &answers(&lister, &rx, 1)[..]
    else {
        panic!("one listing");
    };
    assert_eq!(directory, scratch.path());
    assert_eq!(*generation, 7);
    assert!(!truncated && !partial);
    assert_eq!(
        names(entries.as_ref().unwrap()),
        [
            ("dir".to_owned(), Kind::Directory),
            ("file.md".to_owned(), Kind::File),
            ("link".to_owned(), Kind::Link),
        ]
    );
}

#[test]
fn a_directory_that_cannot_be_read_says_why() {
    let (lister, rx) = lister();
    let missing = PathBuf::from("/no/such/directory/here");
    lister.list(missing.clone(), 1);
    let [
        Answer::Listed {
            directory,
            entries,
            truncated,
            partial,
            ..
        },
    ] = &answers(&lister, &rx, 1)[..]
    else {
        panic!("one listing");
    };
    assert_eq!(directory, &missing);
    assert!(entries.is_err());
    assert!(!truncated && !partial);
}

#[test]
fn a_listing_keeps_at_most_its_limit_and_says_there_were_more() {
    let scratch = Scratch::new("lister-limit");
    for n in 0..5 {
        scratch.file(&format!("{n}"), b"");
    }
    let listed = list(scratch.path(), 3).unwrap();
    assert_eq!(listed.entries.len(), 3);
    assert!(listed.truncated);
    let listed = list(scratch.path(), 5).unwrap();
    assert_eq!(listed.entries.len(), 5);
    assert!(!listed.truncated);
}

#[cfg(unix)]
#[test]
fn a_device_is_looked_at_without_being_opened() {
    let looked = look(Path::new("/dev"), OsString::from("null")).unwrap();
    assert_eq!(looked.mode & 0o170_000, 0o020_000, "a character device");
}

#[cfg(unix)]
#[test]
fn a_look_says_size_time_and_where_a_link_leads() {
    let scratch = Scratch::new("lister-look");
    scratch.file("file.md", b"made up");
    fs::create_dir(scratch.path().join("dir")).unwrap();
    std::os::unix::fs::symlink("dir", scratch.path().join("to-dir")).unwrap();
    std::os::unix::fs::symlink("gone", scratch.path().join("dangling")).unwrap();
    let (lister, rx) = lister();
    let asked = ["file.md", "to-dir", "dangling", "missing"].map(OsString::from);
    lister.look(scratch.path().to_path_buf(), asked.to_vec(), 2);
    let [
        Answer::Looked {
            looks,
            generation,
            directory,
        },
    ] = &answers(&lister, &rx, 1)[..]
    else {
        panic!("one look");
    };
    assert_eq!(*generation, 2);
    assert_eq!(directory, scratch.path());
    assert_eq!(looks.len(), 3, "a name that has gone is left out");
    let file = &looks[0];
    assert_eq!(
        (file.len, file.others, file.target.as_ref()),
        (7, false, None)
    );
    assert!(file.modified.is_some());
    assert_eq!(file.mode & 0o170_000, 0o100_000, "a regular file");
    let to_dir = &looks[1];
    assert_eq!(to_dir.target.as_deref(), Some(Path::new("dir")));
    assert_eq!(to_dir.leads_to, Some(LeadsTo::Directory));
    let dangling = &looks[2];
    assert_eq!(dangling.target.as_deref(), Some(Path::new("gone")));
    assert_eq!(dangling.leads_to, Some(LeadsTo::Missing));
    assert_eq!(file.leads_to, None, "no link");
}

#[test]
fn a_look_covers_at_most_the_rows_shown() {
    let scratch = Scratch::new("lister-looks");
    let names: Vec<OsString> = (0..40).map(|n| OsString::from(format!("{n}"))).collect();
    for name in &names {
        scratch.file(&name.to_string_lossy(), b"");
    }
    let (lister, rx) = lister();
    lister.look(scratch.path().to_path_buf(), names, 0);
    let [
        Answer::Looked {
            looks, directory, ..
        },
    ] = &answers(&lister, &rx, 1)[..]
    else {
        panic!("one look");
    };
    assert_eq!(directory, scratch.path());
    assert_eq!(looks.len(), MAX_LOOKS);
}

#[test]
fn many_requests_use_at_most_four_threads_and_all_are_answered() {
    let scratch = Scratch::new("lister-many");
    let (lister, rx) = lister();
    for generation in 0..20 {
        let directory = scratch.path().join(generation.to_string());
        fs::create_dir(&directory).unwrap();
        lister.list(directory, generation);
        assert!(lock(&lister.shared.queue).alive <= MAX_THREADS);
    }
    let mut generations: Vec<u64> = answers(&lister, &rx, 20)
        .into_iter()
        .map(|answer| match answer {
            Answer::Listed { generation, .. } | Answer::Looked { generation, .. } => generation,
        })
        .collect();
    generations.sort_unstable();
    assert_eq!(generations, (0..20).collect::<Vec<_>>());
}

#[cfg(unix)]
#[test]
fn a_device_is_listed_as_neither_file_nor_directory_without_being_opened() {
    let listed = list(Path::new("/dev"), MAX_NAMES).unwrap();
    let null = listed.entries.iter().find(|entry| entry.name == "null");
    assert_eq!(null.map(|entry| entry.kind), Some(Kind::Other));
}

#[cfg(unix)]
#[test]
fn a_link_in_a_loop_is_not_called_missing() {
    let scratch = Scratch::new("lister-loop");
    std::os::unix::fs::symlink("b", scratch.path().join("a")).unwrap();
    std::os::unix::fs::symlink("a", scratch.path().join("b")).unwrap();
    let looked = look(scratch.path(), OsString::from("a")).unwrap();
    assert_eq!(looked.leads_to, Some(LeadsTo::Unknown));
}

fn list_of(directory: &str, generation: u64) -> Request {
    Request::List(PathBuf::from(directory), generation)
}

#[test]
fn a_newer_request_for_a_directory_replaces_one_still_waiting() {
    let mut queue = Queue::default();
    queue.push(list_of("/a", 1));
    queue.push(list_of("/b", 2));
    queue.push(list_of("/a", 3));
    queue.push(Request::Look(PathBuf::from("/a"), Vec::new(), 4));
    let waiting: Vec<_> = queue.waiting.iter().collect();
    assert_eq!(
        waiting,
        [
            &list_of("/b", 2),
            &list_of("/a", 3),
            &Request::Look(PathBuf::from("/a"), Vec::new(), 4)
        ],
        "a late generation never waits behind the newest"
    );
}

#[test]
fn a_busy_directory_holds_up_only_its_own_requests() {
    let mut queue = Queue::default();
    queue.push(list_of("/hung", 1));
    assert_eq!(queue.next(), Some(list_of("/hung", 1)));
    queue.push(Request::Look(PathBuf::from("/hung"), Vec::new(), 2));
    queue.push(list_of("/fine", 3));
    assert_eq!(queue.next(), Some(list_of("/fine", 3)), "past the busy one");
    assert_eq!(queue.next(), None, "the busy directory's next waits");
    queue.done(Path::new("/hung"));
    assert_eq!(
        queue.next(),
        Some(Request::Look(PathBuf::from("/hung"), Vec::new(), 2))
    );
}

#[test]
fn a_thread_is_started_only_for_work_no_free_thread_will_take() {
    let mut queue = Queue::default();
    queue.push(list_of("/a", 1));
    assert!(queue.wants_a_thread());
    queue.alive = 1;
    assert!(!queue.wants_a_thread(), "the one alive will take it");
    queue.next();
    queue.push(list_of("/a", 2));
    assert!(!queue.wants_a_thread(), "its directory is busy");
    queue.push(list_of("/b", 3));
    assert!(queue.wants_a_thread());
    queue.alive = MAX_THREADS;
    assert!(!queue.wants_a_thread(), "no more than the limit");
}

#[test]
fn forgetting_drops_what_waits_and_what_came() {
    let scratch = Scratch::new("lister-forget");
    let (lister, rx) = lister();
    lister.list(scratch.path().to_path_buf(), 1);
    rx.recv_timeout(Duration::from_secs(10)).expect("an answer");
    lister.forget();
    assert!(lister.take().is_empty());
    lock(&lister.shared.queue).push(list_of("/x", 2));
    lister.forget();
    assert!(lock(&lister.shared.queue).waiting.is_empty());
}
