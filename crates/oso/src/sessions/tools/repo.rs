//! Which repository a session tool's call reaches in an issue space (ADR 0136, step 4): a
//! space's orchestrator names one of the space's repositories with `repo`, as `space_status`
//! gives their names; any other session may leave `repo` out or name its own repository only.
//! And which repository the project commands use from an orchestrator's buffer.
//!
//! Must not depend on: `app`, threads or the disk; these are plain functions over names and
//! paths the app hands in.

use std::path::{Path, PathBuf};

use super::quoted;
use crate::sessions::spaces::stored::{last_component, same};

/// What a call's `repo` names.
#[derive(Debug, PartialEq, Eq)]
pub enum Named<'a> {
    /// The caller's own repository, as without a space.
    Own,
    /// A repository of the orchestrator's space, by its root.
    Member(&'a Path),
}

/// The names of `members`, quoted, for a refusal.
fn names(members: &[(String, PathBuf)]) -> String {
    let names: Vec<String> = members.iter().map(|(name, _)| quoted(name)).collect();
    names.join(", ")
}

/// What `repo` names for a space's orchestrator, whose space has `members` by name and root:
/// it must name one.
///
/// # Errors
///
/// What the agent is told: the names it may give.
pub fn for_orchestrator<'a>(
    repo: Option<&str>,
    members: &'a [(String, PathBuf)],
) -> Result<Named<'a>, String> {
    let Some(name) = repo else {
        return Err(format!(
            "name a repository of your space with \"repo\": {}",
            names(members)
        ));
    };
    members
        .iter()
        .find(|(member, _)| member == name)
        .map(|(_, root)| Named::Member(root.as_path()))
        .ok_or_else(|| {
            format!(
                "{} is no repository of your space; its repositories: {}",
                quoted(name),
                names(members)
            )
        })
}

/// What `repo` names for any other session, at `own` (its project's root: a main checkout or
/// a worktree), whose space has `members` (none outside a space): its own repository only, by
/// the name its space gives the repository holding `own`, else that root's last component.
///
/// # Errors
///
/// What the agent is told: the name it may give.
pub fn for_other(
    repo: Option<&str>,
    members: &[(String, PathBuf)],
    own: Option<&Path>,
) -> Result<Named<'static>, String> {
    let Some(name) = repo else {
        return Ok(Named::Own);
    };
    let own_name = own.map(|own| {
        let holder = holding(members, own);
        let named = holder.and_then(|root| members.iter().find(|(_, r)| same(r, root)));
        named.map_or_else(|| last_component(own), |(name, _)| name.clone())
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

/// The repository of `members` that `path` is in: the deepest whose root holds it.
pub fn holding<'a>(members: &'a [(String, PathBuf)], path: &Path) -> Option<&'a Path> {
    members
        .iter()
        .map(|(_, root)| root.as_path())
        .filter(|root| path.starts_with(root))
        .max_by_key(|root| root.components().count())
}

/// The repository the project commands use from an orchestrator's buffer: the space's one
/// trusted repository, else the one `space-use-repo` chose, `chosen`, while it is still one
/// of the `trusted`.
///
/// # Errors
///
/// What the echo area says.
pub fn for_commands(trusted: &[&Path], chosen: Option<&Path>) -> Result<PathBuf, String> {
    match (trusted, chosen) {
        ([], _) => Err("No repository of the space is trusted: trust-project first".to_owned()),
        ([one], _) => Ok(one.to_path_buf()),
        (several, Some(chosen)) if several.iter().any(|root| same(root, chosen)) => {
            Ok(chosen.to_path_buf())
        }
        _ => Err(
            "The space has several trusted repositories: choose one with space-use-repo \
             (C-x a S u)"
                .to_owned(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{Named, for_commands, for_orchestrator, for_other, holding};

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
            for_orchestrator(Some("web"), &members),
            Ok(Named::Member(Path::new("/src/web")))
        );
        let none = for_orchestrator(None, &members).unwrap_err();
        assert!(none.contains("\"api\", \"web\""), "{none}");
        let other = for_orchestrator(Some("db"), &members).unwrap_err();
        assert!(other.contains("\"db\" is no repository"), "{other}");
    }

    #[test]
    fn another_session_may_name_only_its_own_repository() {
        let members = members();
        let own = Some(Path::new("/src/api"));
        assert_eq!(for_other(None, &members, own), Ok(Named::Own));
        assert_eq!(for_other(Some("api"), &members, own), Ok(Named::Own));
        let other = for_other(Some("web"), &members, own).unwrap_err();
        assert!(
            other.contains("only your own repository, \"api\""),
            "{other}"
        );
        // A worker in a worktree of it names the repository as its space does.
        let worktree = Some(Path::new("/src/api/.claude/worktrees/fix"));
        assert_eq!(for_other(Some("api"), &members, worktree), Ok(Named::Own));
        assert!(for_other(Some("fix"), &members, worktree).is_err());
        // Outside a space, its own is its root's name.
        let alone = Some(Path::new("/src/tool"));
        assert_eq!(for_other(Some("tool"), &[], alone), Ok(Named::Own));
        assert!(for_other(Some("tool"), &[], None).is_err());
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

    #[test]
    fn the_commands_use_the_one_trusted_repository_else_the_one_chosen() {
        let (api, web) = (Path::new("/src/api"), Path::new("/src/web"));
        assert!(for_commands(&[], Some(api)).is_err());
        assert_eq!(for_commands(&[api], None), Ok(api.to_path_buf()));
        assert_eq!(for_commands(&[api], Some(web)), Ok(api.to_path_buf()));
        assert!(
            for_commands(&[api, web], None)
                .unwrap_err()
                .contains("space-use-repo")
        );
        assert_eq!(for_commands(&[api, web], Some(web)), Ok(web.to_path_buf()));
        // A choice no longer trusted is no choice.
        let gone = Path::new("/src/db");
        assert!(for_commands(&[api, web], Some(gone)).is_err());
    }
}
