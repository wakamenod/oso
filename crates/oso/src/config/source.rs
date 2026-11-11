//! Where the user configuration comes from, by the command line: the person's file, another one a
//! dev build is given, or none (ADR 0108). It also says which directories the agent channel never
//! saves into.
//!
//! Must not depend on: `app` or `dispatch`; it asks the environment, and not the disk.

use std::path::{Path, PathBuf};

use oso_platform::privacy;

/// The file's name in oso's configuration directory.
pub const FILE_NAME: &str = "config.toml";

/// Where the configuration comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// `--no-config`: none, the compiled-in defaults.
    Nothing,
    /// `config.toml` in oso's configuration directory.
    Home,
    /// `--config FILE`, in a dev build: the end-to-end flows and benchmarks never read the
    /// person's file.
    File(PathBuf),
}

impl Source {
    /// The file to read, if any.
    pub fn file(&self) -> Option<PathBuf> {
        match self {
            Self::Nothing => None,
            Self::Home => privacy::config_home().map(|directory| directory.join(FILE_NAME)),
            Self::File(file) => Some(file.clone()),
        }
    }

    /// The configuration directories the agent channel never saves into (ADR 0108): the
    /// person's, whatever the command line says, as the next start reads it, and the one
    /// holding the file `--config` names.
    pub fn directories(&self) -> Vec<PathBuf> {
        let mut directories: Vec<PathBuf> = privacy::config_home().into_iter().collect();
        if let Self::File(file) = self
            && let Some(directory) = file.parent().map(Path::to_path_buf)
            && !directories.contains(&directory)
        {
            directories.push(directory);
        }
        directories
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use oso_platform::privacy::config_home;

    use super::Source;

    #[test]
    fn the_persons_directory_is_kept_from_the_agent_whatever_the_command_line_says() {
        let file = PathBuf::from("/tmp/e2e/config/config.toml");
        let mut both: Vec<PathBuf> = config_home().into_iter().collect();
        both.push(PathBuf::from("/tmp/e2e/config"));
        assert_eq!(Source::File(file.clone()).directories(), both);
        assert_eq!(Source::File(file.clone()).file(), Some(file));
        assert_eq!(Source::Nothing.file(), None);
        let persons: Vec<PathBuf> = config_home().into_iter().collect();
        assert_eq!(Source::Nothing.directories(), persons);
        assert_eq!(Source::Home.directories(), persons);
    }
}
