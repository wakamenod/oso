//! Which repository a session tool's call reaches in an issue space (ADR 0136, step 4): a
//! space's orchestrator names one of the space's repositories with `repo`, as `space_status`
//! gives their names; any other session may leave `repo` out or name its own repository only.
//!
//! Must not depend on: `app`, threads or the disk; these are plain functions over names and
//! paths the app hands in.

use std::path::Path;

use super::quoted;
use crate::sessions::spaces::stored::same;

/// What a call's `repo` names.
#[derive(Debug, PartialEq, Eq)]
pub enum Named<'a> {
    /// The caller's own repository, as without a space.
    Own,
    /// A repository of the orchestrator's space, by its root.
    Member(&'a Path),
}

/// What `repo` names for a caller that is its space's orchestrator or not, with `members` its
/// space's repositories by name and root (none outside a space) and `own` its own root.
///
/// # Errors
///
/// What the agent is told: the names it may give.
pub fn named<'a>(
    orchestrator: bool,
    repo: Option<&str>,
    members: &'a [(String, std::path::PathBuf)],
    own: Option<&Path>,
) -> Result<Named<'a>, String> {
    let names = || {
        let names: Vec<String> = members.iter().map(|(name, _)| quoted(name)).collect();
        names.join(", ")
    };
    match (orchestrator, repo) {
        (true, None) => Err(format!(
            "name a repository of your space with \"repo\": {}",
            names()
        )),
        (true, Some(name)) => members
            .iter()
            .find(|(member, _)| member == name)
            .map(|(_, root)| Named::Member(root.as_path()))
            .ok_or_else(|| {
                format!(
                    "{} is no repository of your space; its repositories: {}",
                    quoted(name),
                    names()
                )
            }),
        (false, None) => Ok(Named::Own),
        (false, Some(name)) => {
            let own_name = own.and_then(|own| {
                (members.iter().find(|(_, root)| same(root, own)))
                    .map(|(name, _)| name.clone())
                    .or_else(|| own.file_name().map(|n| n.to_string_lossy().into_owned()))
            });
            match own_name {
                Some(own_name) if own_name == name => Ok(Named::Own),
                Some(own_name) => Err(format!(
                    "you may name only your own repository, {}",
                    quoted(&own_name)
                )),
                None => Err("your session is in no repository to name".to_owned()),
            }
        }
    }
}

/// The repository of `members` that `path` is in: the deepest whose root holds it.
pub fn holding<'a>(members: &'a [(String, std::path::PathBuf)], path: &Path) -> Option<&'a Path> {
    members
        .iter()
        .map(|(_, root)| root.as_path())
        .filter(|root| path.starts_with(root))
        .max_by_key(|root| root.components().count())
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{Named, holding, named};

    fn members() -> Vec<(String, PathBuf)> {
        vec![
            ("api".to_owned(), PathBuf::from("/src/api")),
            ("web".to_owned(), PathBuf::from("/src/web")),
        ]
    }

    #[test]
    fn an_orchestrator_must_name_a_repository_of_its_space() {
        let members = members();
        assert_eq!(
            named(true, Some("web"), &members, None),
            Ok(Named::Member(Path::new("/src/web")))
        );
        let none = named(true, None, &members, None).unwrap_err();
        assert!(none.contains("\"api\", \"web\""), "{none}");
        let other = named(true, Some("db"), &members, None).unwrap_err();
        assert!(other.contains("\"db\" is no repository"), "{other}");
    }

    #[test]
    fn another_session_may_name_only_its_own_repository() {
        let members = members();
        let own = Some(Path::new("/src/api"));
        assert_eq!(named(false, None, &members, own), Ok(Named::Own));
        assert_eq!(named(false, Some("api"), &members, own), Ok(Named::Own));
        let other = named(false, Some("web"), &members, own).unwrap_err();
        assert!(
            other.contains("only your own repository, \"api\""),
            "{other}"
        );
        // Outside a space, its own is its root's name.
        let alone = Some(Path::new("/src/tool"));
        assert_eq!(named(false, Some("tool"), &[], alone), Ok(Named::Own));
        assert!(named(false, Some("tool"), &[], None).is_err());
    }

    #[test]
    fn a_path_is_in_the_deepest_repository_holding_it() {
        let mut members = members();
        members.push(("inner".to_owned(), PathBuf::from("/src/api/vendor/inner")));
        let worktree = Path::new("/src/api/.claude/worktrees/fix");
        assert_eq!(holding(&members, worktree), Some(Path::new("/src/api")));
        let inner = Path::new("/src/api/vendor/inner/x");
        assert_eq!(
            holding(&members, inner),
            Some(Path::new("/src/api/vendor/inner"))
        );
        assert_eq!(holding(&members, Path::new("/src/apix")), None);
    }
}
