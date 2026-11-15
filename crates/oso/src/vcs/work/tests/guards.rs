//! A status's thread kept from what it must not do: run the repository's hooks or diff drivers,
//! let the person's configuration reshape a patch, read a name as a pattern, add untracked files
//! under a tracked path, stage a mode no hunk shows, or read a file that may hold secrets.
//!
//! Must not depend on: `app`, `dispatch` or winit; these drive the thread's state directly, and
//! run git only through the process module.

use std::path::Path;

use super::{
    Repo, Report, Section, TEN, Target, View, cached, open_a, read, run, state, text, two_hunks,
};

/// The hunk header `target` names in the unstaged section's first file.
fn first_hunk() -> Target {
    Target::Hunk {
        section: Section::Unstaged,
        file: 0,
        hunk: 0,
    }
}

fn index_file(repo: &Repo, path: &str) -> Vec<u8> {
    repo.git(&["show", &format!(":{path}")])
}

/// A script at `path` that leaves `marker` when it runs, then prints its first argument's file,
/// as a textconv driver does.
#[cfg(unix)]
fn marking_script(path: &Path, marker: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let script = format!(
        "#!/bin/sh\ntouch '{}'\n[ -f \"$1\" ] && cat \"$1\"\nexit 0\n",
        marker.display()
    );
    std::fs::write(path, script).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(unix)]
#[test]
fn no_hook_of_the_repository_runs_while_staging_and_reading() {
    let repo = two_hunks("vcs-no-hooks");
    let marker = repo.dir.join("hook-ran");
    let own = repo.root.join(".git/hooks");
    std::fs::create_dir_all(&own).unwrap();
    marking_script(&own.join("post-index-change"), &marker);
    let configured = repo.dir.join("configured-hooks");
    std::fs::create_dir_all(&configured).unwrap();
    marking_script(&configured.join("post-index-change"), &marker);
    repo.git(&["config", "core.hooksPath", configured.to_str().unwrap()]);
    let mut state = state(&repo);
    let view = open_a(Section::Unstaged);
    let laid = read(&mut state, &repo.root, &view);
    let (laid, _) = run(&mut state, &repo.root, &laid, &[first_hunk()], true, &view);
    let staged = Target::Section(Section::Staged);
    let (laid, _) = run(&mut state, &repo.root, &laid, &[staged], false, &view);
    let file = Target::File {
        section: Section::Unstaged,
        file: 0,
    };
    let _ = run(&mut state, &repo.root, &laid, &[file], true, &view);
    assert!(!cached(&repo).is_empty());
    assert!(!marker.exists(), "a hook ran");
}

#[cfg(unix)]
#[test]
fn the_persons_configuration_does_not_reshape_what_is_read_or_staged() {
    let repo = Repo::new("vcs-config");
    let marker = repo.dir.join("driver-ran");
    let script = repo.dir.join("driver.sh");
    marking_script(&script, &marker);
    repo.write(".gitattributes", b"*.txt diff=conv\n");
    repo.write("a.txt", TEN.as_bytes());
    repo.commit("first");
    let script = script.to_str().unwrap();
    for (key, value) in [
        ("diff.noprefix", "true"),
        ("diff.mnemonicPrefix", "true"),
        ("color.ui", "always"),
        ("diff.context", "0"),
        ("diff.suppressBlankEmpty", "true"),
        ("diff.relative", "true"),
        ("apply.whitespace", "fix"),
        ("apply.ignoreWhitespace", "change"),
        ("diff.external", script),
        ("diff.conv.textconv", script),
    ] {
        repo.git(&["config", key, value]);
    }
    // A trailing space, which `apply.whitespace=fix` would take away.
    let changed = TEN.replacen("\n2\n", "\ntwo \n", 1);
    repo.write("a.txt", changed.as_bytes());
    let mut state = state(&repo);
    let view = open_a(Section::Unstaged);
    let laid = read(&mut state, &repo.root, &view);
    assert!(text(&laid).contains("@@ -1,5 +1,5 @@"), "{}", text(&laid));
    let (_, reports) = run(&mut state, &repo.root, &laid, &[first_hunk()], true, &view);
    assert!(
        matches!(reports.as_slice(), [Report::Ran { .. }]),
        "{reports:?}"
    );
    assert_eq!(index_file(&repo, "a.txt"), changed.as_bytes());
    assert!(!marker.exists(), "a diff driver ran");
}

#[cfg(unix)]
#[test]
fn a_name_that_reads_as_a_pattern_is_read_as_itself() {
    let repo = Repo::new("vcs-literal-read");
    repo.write("a*.txt", b"star\n");
    repo.write("ab.txt", b"ab\n");
    repo.commit("first");
    repo.write("a*.txt", b"star changed\n");
    repo.write("ab.txt", b"ab changed\n");
    let mut state = state(&repo);
    let mut view = View::default();
    view.open_file(Section::Unstaged, b"a*.txt".to_vec());
    let laid = read(&mut state, &repo.root, &view);
    let shown = text(&laid);
    assert!(shown.contains("+star changed"), "{shown}");
    assert!(!shown.contains("+ab changed"), "{shown}");
}

#[test]
fn staging_a_tracked_path_never_adds_untracked_files_under_it() {
    let repo = Repo::new("vcs-update");
    repo.write("config", b"one\n");
    repo.commit("first");
    std::fs::remove_file(repo.root.join("config")).unwrap();
    repo.write("config/.env", b"TOKEN=x\n");
    repo.write("config/other.txt", b"other\n");
    let mut state = state(&repo);
    let laid = read(&mut state, &repo.root, &View::default());
    let file = Target::File {
        section: Section::Unstaged,
        file: 0,
    };
    assert_eq!(laid.built.files(Section::Unstaged)[0].path, b"config");
    let (laid, reports) = run(
        &mut state,
        &repo.root,
        &laid,
        &[file],
        true,
        &View::default(),
    );
    assert!(
        matches!(reports.as_slice(), [Report::Ran { .. }]),
        "{reports:?}"
    );
    let listed = String::from_utf8(repo.git(&["ls-files", "--cached"])).unwrap();
    assert_eq!(listed, "");
    let all = Target::Section(Section::Unstaged);
    if !laid.built.files(Section::Unstaged).is_empty() {
        let _ = run(
            &mut state,
            &repo.root,
            &laid,
            &[all],
            true,
            &View::default(),
        );
    }
    let listed = String::from_utf8(repo.git(&["ls-files", "--cached"])).unwrap();
    assert_eq!(listed, "");
}

#[cfg(unix)]
#[test]
fn a_hunk_of_a_file_whose_mode_changed_stages_only_with_it() {
    use std::os::unix::fs::PermissionsExt;
    let repo = two_hunks("vcs-mode");
    let path = repo.root.join("a.txt");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut state = state(&repo);
    let view = open_a(Section::Unstaged);
    let laid = read(&mut state, &repo.root, &view);
    assert!(text(&laid).contains("(a change of mode: s and u take it whole)"));
    let (_, reports) = run(&mut state, &repo.root, &laid, &[first_hunk()], true, &view);
    let [Report::Refused { why }, ..] = reports.as_slice() else {
        panic!("refused: {reports:?}");
    };
    assert!(why.contains("a change of mode"), "{why}");
    assert_eq!(cached(&repo), "");
}

#[test]
fn a_file_renamed_from_a_secret_name_is_not_read() {
    let repo = Repo::new("vcs-secret-rename");
    repo.write(".env", b"TOKEN=one\nKEY=two\nMORE=three\n");
    repo.commit("first");
    repo.git(&["mv", ".env", "notes.txt"]);
    repo.write("notes.txt", b"TOKEN=one\nKEY=two\nMORE=four\n");
    repo.git(&["add", "notes.txt"]);
    let mut state = state(&repo);
    let mut view = View::default();
    view.open_file(Section::Staged, b"notes.txt".to_vec());
    let laid = read(&mut state, &repo.root, &view);
    let shown = text(&laid);
    assert!(shown.contains(".env -> notes.txt"), "{shown}");
    assert!(!shown.contains("TOKEN"), "{shown}");
    assert!(state.hunks.is_empty());
}

#[test]
fn a_file_whose_open_buffer_may_hold_secrets_is_not_read() {
    let repo = two_hunks("vcs-withheld");
    let mut state = state(&repo);
    let mut view = open_a(Section::Unstaged);
    view.withheld.insert(b"a.txt".to_vec());
    let laid = read(&mut state, &repo.root, &view);
    assert!(state.hunks.is_empty());
    assert!(!text(&laid).contains("fourteen"));
}
