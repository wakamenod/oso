//! The `claude` command line of a hosted session (ADR 0121, "A session"), built only from parts
//! oso controls, never from text an agent gave.
//!
//! Must not depend on: threads, the file system or `app`; these are plain functions over what
//! the caller found.
//!
//! - Every option is given as `--flag=value`, so that a value can never be taken for an option,
//!   and `--mcp-config`, which takes any number of values, takes exactly one.
//! - A brief, when one is given, comes after `--`, which ends the options.
//! - No permission option is ever given: not `--permission-mode`, not
//!   `--dangerously-skip-permissions`, not `--allowedTools`. A session starts in the mode the
//!   person's own settings give it, whoever asked for it.

use std::ffi::OsString;
use std::path::Path;

use serde_json::json;

use super::identity::SessionId;

/// The variable that names a hosted session to its bridge, `oso mcp` (ADR 0121, "The bridge and
/// the channel"). Set in the session's environment and in the MCP server's own.
pub const SESSION_VARIABLE: &str = "OSO_SESSION";

/// How a session's `claude` starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Start<'a> {
    /// A new conversation under `id`.
    New { id: &'a SessionId },
    /// The conversation `id`, resumed.
    Resume { id: &'a SessionId },
    /// The conversation `from`, copied under the new id `id`.
    Fork {
        from: &'a SessionId,
        id: &'a SessionId,
    },
}

impl Start<'_> {
    /// The id the session runs under once started.
    pub fn id(&self) -> &SessionId {
        match self {
            Self::New { id } | Self::Resume { id } | Self::Fork { id, .. } => id,
        }
    }
}

/// What a session's command line is made of.
#[derive(Debug, Clone)]
pub struct Line<'a> {
    pub start: Start<'a>,
    /// `--name`, cleaned (see `identity`).
    pub name: &'a str,
    /// oso's own executable, by absolute path, for the `oso` MCP server.
    pub oso: &'a Path,
    /// oso's process id, which `oso mcp --pid` connects to.
    pub oso_pid: u32,
    /// `--append-system-prompt`, the project's instructions (ADR 0121 step 7).
    pub system_prompt: Option<&'a str>,
    /// The first prompt, after `--` (ADR 0121 steps 4 and 6).
    pub brief: Option<&'a str>,
}

/// Why a command line cannot be built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LineError {
    /// oso's own path is not UTF-8, so the MCP configuration, which is JSON, cannot name it.
    NotUtf8,
}

impl std::fmt::Display for LineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotUtf8 => {
                f.write_str("oso's own path is not UTF-8, so a session cannot name it")
            }
        }
    }
}

/// The MCP configuration given with `--mcp-config`: one server, `oso`, which is oso's own
/// executable as `mcp --pid PID`, with [`SESSION_VARIABLE`] set. The person's other MCP servers
/// still load.
pub fn mcp_config(oso: &Path, oso_pid: u32, id: &SessionId) -> Result<String, LineError> {
    let command = oso.to_str().ok_or(LineError::NotUtf8)?;
    let config = json!({
        "mcpServers": {
            "oso": {
                "type": "stdio",
                "command": command,
                "args": ["mcp", "--pid", oso_pid.to_string()],
                "env": { SESSION_VARIABLE: id.as_str() },
            }
        }
    });
    Ok(config.to_string())
}

/// The arguments after `claude`.
pub fn arguments(line: &Line<'_>) -> Result<Vec<OsString>, LineError> {
    let mut args: Vec<OsString> = Vec::new();
    let mut flag = |name: &str, value: &str| args.push(format!("--{name}={value}").into());
    match &line.start {
        Start::New { id } => flag("session-id", id.as_str()),
        Start::Resume { id } => flag("resume", id.as_str()),
        Start::Fork { from, id } => {
            flag("resume", from.as_str());
            // The CLI takes `--session-id` with `--resume` only with `--fork-session`; the copy
            // then runs under oso's id.
            args.push("--fork-session".into());
            args.push(format!("--session-id={}", id.as_str()).into());
        }
    }
    let config = mcp_config(line.oso, line.oso_pid, line.start.id())?;
    args.push(format!("--name={}", line.name).into());
    args.push(format!("--mcp-config={config}").into());
    if let Some(prompt) = line.system_prompt {
        args.push(format!("--append-system-prompt={prompt}").into());
    }
    if let Some(brief) = line.brief {
        args.push("--".into());
        args.push(brief.into());
    }
    Ok(args)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::Path;

    use super::{Line, SESSION_VARIABLE, Start, arguments};
    use crate::sessions::identity::SessionId;

    fn id(n: u8) -> SessionId {
        SessionId::from_random([n; 16])
    }

    fn line<'a>(start: Start<'a>) -> Line<'a> {
        Line {
            start,
            name: "oso-2",
            oso: Path::new("/opt/oso/bin/oso"),
            oso_pid: 4242,
            system_prompt: None,
            brief: None,
        }
    }

    fn strings(args: &[OsString]) -> Vec<String> {
        args.iter()
            .map(|arg| arg.to_str().unwrap().to_owned())
            .collect()
    }

    #[test]
    fn a_new_session_names_its_id_its_name_and_oso() {
        let a = id(1);
        let args = strings(&arguments(&line(Start::New { id: &a })).unwrap());
        assert_eq!(args[0], format!("--session-id={a}"));
        assert_eq!(args[1], "--name=oso-2");
        let config = args[2].strip_prefix("--mcp-config=").unwrap();
        let config: serde_json::Value = serde_json::from_str(config).unwrap();
        let oso = &config["mcpServers"]["oso"];
        assert_eq!(oso["command"], "/opt/oso/bin/oso");
        assert_eq!(oso["args"], serde_json::json!(["mcp", "--pid", "4242"]));
        assert_eq!(oso["env"][SESSION_VARIABLE], a.as_str());
        assert_eq!(config["mcpServers"].as_object().unwrap().len(), 1);
        assert_eq!(args.len(), 3);
    }

    #[test]
    fn a_resume_and_a_fork() {
        let (old, new) = (id(1), id(2));
        let args = strings(&arguments(&line(Start::Resume { id: &old })).unwrap());
        assert_eq!(args[0], format!("--resume={old}"));
        assert!(args.iter().all(|arg| !arg.starts_with("--session-id")));
        let args = strings(
            &arguments(&line(Start::Fork {
                from: &old,
                id: &new,
            }))
            .unwrap(),
        );
        assert_eq!(
            args[..3],
            [
                format!("--resume={old}"),
                "--fork-session".to_owned(),
                format!("--session-id={new}")
            ]
        );
        // The bridge of the copy names the copy.
        assert!(args[4].contains(new.as_str()) && !args[4].contains(old.as_str()));
    }

    #[test]
    fn a_brief_comes_after_the_end_of_the_options() {
        let a = id(1);
        let mut l = line(Start::New { id: &a });
        l.brief = Some("--dangerously-skip-permissions");
        l.system_prompt = Some("--x");
        let args = strings(&arguments(&l).unwrap());
        let end = args.iter().position(|arg| arg == "--").unwrap();
        assert_eq!(end, args.len() - 2);
        assert_eq!(args[end + 1], "--dangerously-skip-permissions");
        assert!(args.contains(&"--append-system-prompt=--x".to_owned()));
    }

    #[test]
    fn only_these_options_are_ever_given() {
        let (old, new) = (id(1), id(2));
        let allowed = [
            "--session-id=",
            "--resume=",
            "--fork-session",
            "--name=",
            "--mcp-config=",
            "--append-system-prompt=",
        ];
        for start in [
            Start::New { id: &new },
            Start::Resume { id: &old },
            Start::Fork {
                from: &old,
                id: &new,
            },
        ] {
            let mut l = line(start);
            l.system_prompt = Some("p");
            let args = strings(&arguments(&l).unwrap());
            for arg in &args {
                assert!(
                    allowed.iter().any(|ok| arg.starts_with(ok)),
                    "an option oso never gives: {arg}"
                );
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_path_that_is_not_utf8_is_refused() {
        use std::os::unix::ffi::OsStrExt;
        let path = Path::new(std::ffi::OsStr::from_bytes(b"/opt/\xff/oso"));
        assert!(super::mcp_config(path, 1, &id(1)).is_err());
    }
}
