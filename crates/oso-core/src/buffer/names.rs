//! Names that tell buffers of files with one name apart (ADR 0051, ADR 0088).
//!
//! Must not depend on: `editor`.
//!
//! As Emacs's uniquify with `post-forward-angle-brackets`: two files called `notes.md` are
//! named `notes.md<work>` and `notes.md<home>`, each with the fewest directories, nearest
//! first, that no other file of that name has there. The name comes from the path as
//! visited, with no look at the file system.

use std::path::Path;

/// The name of each of `files`, in order, so that no two share one, and none is one of
/// `taken`, the names of buffers with no file.
pub(crate) fn distinct(files: &[&Path], taken: &[&str]) -> Vec<String> {
    let bases: Vec<String> = files.iter().map(|path| base(path)).collect();
    let dirs: Vec<Vec<String>> = files.iter().map(|path| dirs(path)).collect();
    let mut names = Vec::with_capacity(files.len());
    for (index, base) in bases.iter().enumerate() {
        let same: Vec<usize> = (0..files.len())
            .filter(|other| *other != index && bases[*other] == *base)
            .collect();
        if same.is_empty() && !taken.contains(&base.as_str()) {
            names.push(base.clone());
            continue;
        }
        let own = &dirs[index];
        // The fewest directories, at least one, that no other of this name ends with too.
        let fewest = (1..=own.len()).find(|&count| {
            same.iter()
                .all(|other| suffix(&dirs[*other], count) != suffix(own, count))
        });
        let count = fewest.unwrap_or(own.len());
        names.push(format!("{base}<{}>", suffix(own, count).join("/")));
    }
    number_what_is_left(&mut names, taken);
    names
}

/// The file's own name, or the whole path for one with none, such as `/`.
fn base(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.to_string_lossy().into_owned(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// The names of the directories on the way to the file, from the root.
fn dirs(path: &Path) -> Vec<String> {
    let Some(parent) = path.parent() else {
        return Vec::new();
    };
    parent
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .filter(|part| part != "/")
        .collect()
}

/// The last `count` of `dirs`, or all of them if there are fewer.
fn suffix(dirs: &[String], count: usize) -> &[String] {
    &dirs[dirs.len().saturating_sub(count)..]
}

/// Numbers names that are still the same, such as two buffers of one path, from the second
/// on: `notes.md<work><2>`.
fn number_what_is_left(names: &mut [String], taken: &[&str]) {
    for index in 0..names.len() {
        let first = names[index].clone();
        let mut number = 2;
        while names[..index].contains(&names[index]) || taken.contains(&names[index].as_str()) {
            names[index] = format!("{first}<{number}>");
            number += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn names(paths: &[&str], taken: &[&str]) -> Vec<String> {
        let paths: Vec<&Path> = paths.iter().map(Path::new).collect();
        distinct(&paths, taken)
    }

    #[test]
    fn a_file_alone_keeps_its_name() {
        assert_eq!(names(&["/home/me/notes.md"], &[]), ["notes.md"]);
    }

    #[test]
    fn two_files_with_one_name_take_their_nearest_directory() {
        assert_eq!(
            names(&["/home/me/work/notes.md", "/home/me/notes.md"], &[]),
            ["notes.md<work>", "notes.md<me>"]
        );
    }

    #[test]
    fn one_directory_name_in_common_takes_one_more() {
        assert_eq!(
            names(
                &["/a/work/notes.md", "/b/work/notes.md", "/home/notes.md"],
                &[]
            ),
            ["notes.md<a/work>", "notes.md<b/work>", "notes.md<home>"]
        );
    }

    #[test]
    fn a_file_named_as_a_buffer_with_no_file_takes_its_directory() {
        assert_eq!(
            names(&["/tmp/*scratch*"], &["*scratch*"]),
            ["*scratch*<tmp>"]
        );
    }

    #[test]
    fn one_path_twice_is_numbered() {
        assert_eq!(
            names(&["/a/notes.md", "/a/notes.md"], &[]),
            ["notes.md<a>", "notes.md<a><2>"]
        );
    }

    #[test]
    fn a_file_at_the_root_has_no_directory_to_take() {
        assert_eq!(
            names(&["/notes.md", "/a/notes.md"], &[]),
            ["notes.md<>", "notes.md<a>"]
        );
    }

    proptest! {
        #[test]
        fn names_never_repeat(
            paths in proptest::collection::vec("(/[ab]){0,3}/[xy]", 1..8),
            taken in proptest::collection::vec("[xy]", 0..2),
        ) {
            let paths: Vec<&str> = paths.iter().map(String::as_str).collect();
            let taken: Vec<&str> = taken.iter().map(String::as_str).collect();
            let names = names(&paths, &taken);
            for (index, name) in names.iter().enumerate() {
                prop_assert!(!names[..index].contains(name), "{names:?}");
                prop_assert!(!taken.contains(&name.as_str()), "{names:?}");
            }
        }

        #[test]
        fn a_name_alone_is_kept(paths in proptest::collection::vec("(/[ab]){0,3}/[a-z]{1,8}", 1..8)) {
            let paths: Vec<&str> = paths.iter().map(String::as_str).collect();
            let names = names(&paths, &[]);
            for (index, path) in paths.iter().enumerate() {
                let base = base(Path::new(path));
                let alone = paths.iter().filter(|other| base_of(other) == base).count() == 1;
                if alone {
                    prop_assert_eq!(&names[index], &base);
                }
            }
        }
    }

    fn base_of(path: &str) -> String {
        base(Path::new(path))
    }
}
