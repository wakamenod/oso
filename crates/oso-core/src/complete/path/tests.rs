//! Tests of completing a path by its segments, with listings made up.
//!
//! Must not depend on: the file system.

use std::collections::HashMap;

use super::*;

fn names(entries: &[(&str, NameKind)]) -> Vec<Name> {
    entries
        .iter()
        .map(|(name, kind)| Name {
            name: (*name).to_owned(),
            kind: *kind,
        })
        .collect()
}

/// A tree: home has Projects, Pictures and .config; Projects has SideProjects and Work; each of
/// those has an oso directory and a notes file.
fn tree() -> HashMap<PathBuf, Vec<Name>> {
    use NameKind::{Directory as D, File as F};
    let mut tree = HashMap::new();
    tree.insert(
        PathBuf::from("/home/me"),
        names(&[
            ("Projects", D),
            ("Pictures", D),
            (".config", D),
            ("todo.md", F),
        ]),
    );
    tree.insert(PathBuf::from("/home/me/Pictures"), Vec::new());
    tree.insert(
        PathBuf::from("/home/me/Projects"),
        names(&[("SideProjects", D), ("Shared", D)]),
    );
    for side in ["SideProjects", "Shared"] {
        tree.insert(
            PathBuf::from(format!("/home/me/Projects/{side}")),
            names(&[("oso", D), ("notes.md", F)]),
        );
    }
    tree
}

fn home() -> &'static Path {
    Path::new("/home/me")
}

fn expand_in(tree: &HashMap<PathBuf, Vec<Name>>, input: &str) -> Expanded {
    let plan = plan(input, Some(home()), Path::new("/work"));
    expand(&plan, |dir| tree.get(dir).map(Vec::as_slice), |_| false)
}

/// Each row as it reads: its context, then its name.
fn texts(expanded: &Expanded) -> Vec<String> {
    match expanded {
        Expanded::Ready(found, _) => found
            .iter()
            .map(|found| format!("{}{}", found.context, found.name))
            .collect(),
        Expanded::Need(_) => panic!("{expanded:?}"),
    }
}

#[test]
fn a_tilde_or_a_double_slash_starts_the_path_over() {
    assert_eq!(start("/a/b//c"), 5);
    assert_eq!(start("/a/b/~/c"), 5);
    assert_eq!(start("~/a"), 0);
    assert_eq!(start("/a/b~/c"), 0, "only right after a slash");
    let over = plan("/tmp/x/~/notes", Some(home()), Path::new("/work"));
    assert_eq!(over.root, home());
    assert_eq!(over.last, "notes");
    let root = plan("~/Projects//etc/ho", Some(home()), Path::new("/work"));
    assert_eq!(
        (root.root.as_path(), root.segments.as_slice()),
        (Path::new("/"), &["etc".to_owned()][..])
    );
}

#[test]
fn a_tilde_with_no_home_a_tilde_user_and_a_dollar_are_plain_names() {
    let no_home = plan("~/a", None, Path::new("/work"));
    assert_eq!(no_home.root, Path::new("/work"));
    assert_eq!(no_home.segments, ["~"]);
    let user = plan("~me/a", Some(home()), Path::new("/work"));
    assert_eq!(user.segments, ["~me"]);
    let dollar = plan("$HOME/a", Some(home()), Path::new("/work"));
    assert_eq!(
        (dollar.root.as_path(), dollar.segments.as_slice()),
        (Path::new("/work"), &["$HOME".to_owned()][..])
    );
}

#[test]
fn segments_are_prefixes_and_the_last_lists_the_directory_reached() {
    let tree = tree();
    assert_eq!(texts(&expand_in(&tree, "~/Pr/Si/")), ["oso/", "notes.md"]);
    let plan = plan("~/Pr/Si/o", Some(home()), Path::new("/work"));
    assert_eq!(plan.last, "o", "the matcher's");
}

#[test]
fn a_segment_that_matches_several_goes_into_each_and_rows_say_which() {
    let tree = tree();
    let expanded = expand_in(&tree, "~/Pr/S/");
    let rows = texts(&expanded);
    assert_eq!(
        rows,
        [
            "Shared/oso/",
            "Shared/notes.md",
            "SideProjects/oso/",
            "SideProjects/notes.md"
        ]
    );
}

#[test]
fn a_whole_name_matches_only_itself_and_case_is_smart() {
    use NameKind::Directory as D;
    let mut tree = tree();
    tree.insert(PathBuf::from("/home/me/Pro"), names(&[("a", D)]));
    tree.get_mut(Path::new("/home/me")).unwrap().push(Name {
        name: "Pro".to_owned(),
        kind: D,
    });
    assert_eq!(
        texts(&expand_in(&tree, "~/Pro/")),
        ["a/"],
        "not also Projects"
    );
    assert_eq!(
        texts(&expand_in(&tree, "~/pi/")),
        Vec::<&str>::new(),
        "Pictures is empty"
    );
    let upper = expand_in(&tree, "~/PI/");
    assert_eq!(
        upper,
        Expanded::Ready(Vec::new(), false),
        "upper case is exact"
    );
}

#[test]
fn hidden_names_show_only_when_the_last_segment_starts_with_a_dot() {
    let tree = tree();
    assert!(!texts(&expand_in(&tree, "~/")).contains(&".config/".to_owned()));
    assert!(texts(&expand_in(&tree, "~/.")).contains(&".config/".to_owned()));
}

#[test]
fn a_directory_not_listed_yet_is_asked_for() {
    let tree: HashMap<PathBuf, Vec<Name>> = HashMap::new();
    assert_eq!(
        expand_in(&tree, "~/Pr/"),
        Expanded::Need(vec![PathBuf::from("/home/me")])
    );
}

#[test]
fn expanding_keeps_at_most_its_limit_and_says_so() {
    let many: Vec<(String, NameKind)> = (0..40)
        .map(|n| (format!("d{n:02}"), NameKind::Directory))
        .collect();
    let many: Vec<Name> = many
        .iter()
        .map(|(name, kind)| Name {
            name: name.clone(),
            kind: *kind,
        })
        .collect();
    let mut tree = HashMap::new();
    tree.insert(PathBuf::from("/"), names(&[("r", NameKind::Directory)]));
    tree.insert(PathBuf::from("/r"), many);
    for n in 0..40 {
        tree.insert(PathBuf::from(format!("/r/d{n:02}")), Vec::new());
    }
    let plan = plan("/r/d/", None, Path::new("/"));
    let expanded = expand(&plan, |dir| tree.get(dir).map(Vec::as_slice), |_| false);
    assert_eq!(expanded, Expanded::Ready(Vec::new(), true));
}

#[test]
fn a_path_as_typed_and_the_typed_form_of_a_candidate() {
    let plan = plan("~/notes/new.md", Some(home()), Path::new("/work"));
    assert_eq!(as_typed(&plan), Path::new("/home/me/notes/new.md"));
    let relative = plan_relative();
    assert_eq!(as_typed(&relative), Path::new("/work/a/b"));
    assert_eq!(
        typed_form(Path::new("/home/me/x"), true, Some(home())),
        "~/x/"
    );
    assert_eq!(
        typed_form(Path::new("/etc/hosts"), false, Some(home())),
        "/etc/hosts"
    );
}

fn plan_relative() -> Plan {
    plan("a/b", Some(home()), Path::new("/work"))
}

#[test]
fn a_directory_known_to_exist_is_gone_into_without_listing_what_holds_it() {
    let tree: HashMap<PathBuf, Vec<Name>> =
        [(PathBuf::from("/a/b"), names(&[("c.md", NameKind::File)]))]
            .into_iter()
            .collect();
    let plan = plan("/a/b/", None, Path::new("/"));
    let known = |path: &Path| path == Path::new("/a") || path == Path::new("/a/b");
    let expanded = expand(&plan, |dir| tree.get(dir).map(Vec::as_slice), known);
    assert_eq!(texts(&expanded), ["c.md"]);
}

#[test]
fn the_root_is_typed_as_one_slash() {
    assert_eq!(typed_form(Path::new("/"), true, None), "/");
}
