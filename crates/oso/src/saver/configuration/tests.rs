//! The check that keeps the agent channel's saves out of oso's configuration directory.
//!
//! Must not depend on: the editor; these make files and ask the check.

use std::fs;

use super::{MAX_DEPTH, is_inside};
use crate::saver::tests::Scratch;

#[test]
fn a_file_in_the_directory_or_below_it_is_inside_and_one_beside_it_is_not() {
    let scratch = Scratch::new("config-inside");
    let config = scratch.path().join("oso");
    fs::create_dir_all(config.join("themes")).unwrap();
    let file = config.join("config.toml");
    fs::write(&file, b"").unwrap();
    assert!(is_inside(&file, &config).unwrap());
    assert!(is_inside(&config.join("themes").join("new.toml"), &config).unwrap());
    assert!(
        is_inside(&config.join("not-yet.toml"), &config).unwrap(),
        "a file not there yet"
    );
    let outside = scratch.file("notes.md", b"");
    assert!(!is_inside(&outside, &config).unwrap());
    assert!(!is_inside(&scratch.path().join("oso-other.toml"), &config).unwrap());
}

#[test]
fn a_directory_that_does_not_exist_holds_nothing() {
    let scratch = Scratch::new("config-absent");
    let file = scratch.file("notes.md", b"");
    assert!(!is_inside(&file, &scratch.path().join("oso")).unwrap());
}

#[cfg(unix)]
#[test]
fn a_symbolic_link_to_the_directory_or_a_file_in_it_is_inside() {
    let scratch = Scratch::new("config-symlink");
    let config = scratch.path().join("oso");
    fs::create_dir(&config).unwrap();
    fs::write(config.join("config.toml"), b"").unwrap();
    let to_file = scratch.path().join("config-link.toml");
    std::os::unix::fs::symlink(config.join("config.toml"), &to_file).unwrap();
    assert!(is_inside(&to_file, &config).unwrap());
    let to_dir = scratch.path().join("dotfiles");
    std::os::unix::fs::symlink(&config, &to_dir).unwrap();
    assert!(is_inside(&to_dir.join("config.toml"), &config).unwrap());
    // The directory named through a link is the same directory.
    assert!(is_inside(&config.join("config.toml"), &to_dir).unwrap());
}

#[cfg(unix)]
#[test]
fn a_hard_link_elsewhere_to_a_file_in_the_directory_is_inside() {
    let scratch = Scratch::new("config-hard-link");
    let config = scratch.path().join("oso");
    fs::create_dir(&config).unwrap();
    fs::write(config.join("config.toml"), b"").unwrap();
    let other = scratch.path().join("settings.toml");
    fs::hard_link(config.join("config.toml"), &other).unwrap();
    assert!(is_inside(&other, &config).unwrap());
    // A file of its own name only, beside it, is not.
    let alone = scratch.file("alone.toml", b"");
    assert!(!is_inside(&alone, &config).unwrap());
}

/// A job saving `text` over the file at `path`, read just now, as a command queues it.
fn save_of(path: &std::path::Path, text: &str) -> oso_core::editor::SaveJob {
    let mut editor = crate::saver::tests::visit(path);
    let id = editor.window().buffer();
    let edit = oso_core::buffer::Replacement {
        range: 0..usize::MAX,
        text,
    };
    editor.apply_edits(id, &[edit]).unwrap();
    editor.start_save().unwrap();
    editor.take_save_jobs().pop().unwrap()
}

#[test]
fn an_agents_save_into_the_directory_is_refused_and_writes_nothing() {
    use super::{SaveError, run_for_agent};
    let scratch = Scratch::new("config-agent-save");
    let config = scratch.path().join("oso");
    fs::create_dir(&config).unwrap();
    let file = config.join("config.toml");
    fs::write(&file, b"theme = \"dusk\"\n").unwrap();
    let report = run_for_agent(save_of(&file, "agent = 1\n"), std::slice::from_ref(&config));
    assert!(matches!(report.outcome, Err(SaveError::IntoConfiguration)));
    assert!(report.agent_failure().unwrap().starts_with("refused:"));
    assert_eq!(
        fs::read(&file).unwrap(),
        b"theme = \"dusk\"\n",
        "nothing written"
    );
    // Each directory counts: the person's, and the one `--config` names.
    let other = scratch.path().join("e2e");
    fs::create_dir(&other).unwrap();
    let both = [other, config.clone()];
    let report = run_for_agent(save_of(&file, "agent = 1\n"), &both);
    assert!(matches!(report.outcome, Err(SaveError::IntoConfiguration)));
    // Outside it, the save goes on as any other, and the agent hears no failure.
    let notes = scratch.file("notes.md", b"old\n");
    let report = run_for_agent(save_of(&notes, "new\n"), std::slice::from_ref(&config));
    assert!(report.outcome.is_ok(), "{:?}", report.outcome);
    assert_eq!(report.agent_failure(), None);
    assert_eq!(fs::read(&notes).unwrap(), b"new\n");
}

#[cfg(unix)]
#[test]
fn a_file_kept_elsewhere_and_linked_into_the_directory_is_refused_by_either_name() {
    use super::{SaveError, run_for_agent};
    let scratch = Scratch::new("config-dotfiles");
    let config = scratch.path().join("oso");
    fs::create_dir(&config).unwrap();
    let dotfiles = scratch.path().join("dotfiles");
    fs::create_dir(&dotfiles).unwrap();
    let real = dotfiles.join("config.toml");
    fs::write(&real, b"theme = \"dusk\"\n").unwrap();
    let link = config.join("config.toml");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    for opened in [&link, &real] {
        let report = run_for_agent(
            save_of(opened, "agent = 1\n"),
            std::slice::from_ref(&config),
        );
        assert!(
            matches!(report.outcome, Err(SaveError::IntoConfiguration)),
            "{}: {:?}",
            opened.display(),
            report.outcome
        );
    }
    assert_eq!(
        fs::read(&real).unwrap(),
        b"theme = \"dusk\"\n",
        "nothing written"
    );
    // A file beside it in the same place, which no entry leads to, is not refused.
    let other = dotfiles.join("notes.md");
    fs::write(&other, b"old\n").unwrap();
    assert!(!is_inside(&other, &config).unwrap());
}

#[cfg(unix)]
#[test]
fn a_link_in_the_directory_to_what_is_not_there_yet_is_where_it_leads() {
    let scratch = Scratch::new("config-dangling");
    let config = scratch.path().join("oso");
    fs::create_dir(&config).unwrap();
    let missing = scratch.path().join("dotfiles-config.toml");
    std::os::unix::fs::symlink(&missing, config.join("config.toml")).unwrap();
    assert!(
        is_inside(&missing, &config).unwrap(),
        "the file the link waits for"
    );
    assert!(is_inside(&config.join("config.toml"), &config).unwrap());
}

#[test]
fn a_write_to_a_new_place_is_judged_where_it_would_go() {
    use super::{SaveError, run_for_agent};
    use oso_core::editor::Elsewhere;
    let scratch = Scratch::new("config-write-file");
    let config = scratch.path().join("oso");
    fs::create_dir(&config).unwrap();
    let notes = scratch.file("config.toml", b"mine\n");
    // `C-x C-w` into the directory itself writes the buffer's file's name there.
    let mut job = save_of(&notes, "agent = 1\n");
    job.path = config.clone();
    job.baseline = None;
    job.elsewhere = Some(Elsewhere {
        over: None,
        open: Vec::new(),
        name: "config.toml".into(),
    });
    let report = run_for_agent(job, std::slice::from_ref(&config));
    assert!(matches!(report.outcome, Err(SaveError::IntoConfiguration)));
    assert!(!config.join("config.toml").exists());
}

#[cfg(unix)]
#[test]
fn a_directory_in_it_that_cannot_be_listed_refuses_what_it_might_lead_to() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = Scratch::new("config-unlisted");
    let config = scratch.path().join("oso");
    let themes = config.join("themes");
    fs::create_dir_all(&themes).unwrap();
    let kept = scratch.file("dark.toml", b"");
    std::os::unix::fs::symlink(&kept, themes.join("dark.toml")).unwrap();
    let other = scratch.file("notes.md", b"");
    let mode = |path: &std::path::Path, mode| {
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    };
    // Searched but not listed: what is in it can be read by name, and leads anywhere.
    mode(&themes, 0o111);
    let refused = (
        is_inside(&kept, &config),
        is_inside(&themes.join("new.toml"), &config),
        is_inside(&other, &config),
    );
    mode(&themes, 0o755);
    assert!(refused.0.unwrap(), "what an entry in it leads to");
    assert!(refused.1.unwrap(), "a file under it");
    assert!(
        refused.2.unwrap(),
        "what it might lead to cannot be ruled out"
    );
    assert!(
        !is_inside(&other, &config).unwrap(),
        "once it can be listed again"
    );
}

#[test]
fn a_directory_deeper_than_the_look_goes_refuses_what_it_might_lead_to() {
    let scratch = Scratch::new("config-deep");
    let config = scratch.path().join("oso");
    let mut deepest = config.clone();
    for level in 1..=MAX_DEPTH {
        deepest.push(format!("d{level}"));
    }
    fs::create_dir_all(&deepest).unwrap();
    let other = scratch.file("notes.md", b"");
    assert!(
        !is_inside(&other, &config).unwrap(),
        "every directory down to the depth is looked through"
    );
    // One level more: what is in it, as a link out, is not looked at.
    fs::create_dir(deepest.join("deeper")).unwrap();
    assert!(
        is_inside(&other, &config).unwrap(),
        "what it might lead to cannot be ruled out"
    );
}
