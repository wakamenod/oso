//! The `claude` command line of a hosted session (ADR 0121, "A session"), built only from parts
//! oso controls, never from text an agent gave.
//!
//! Must not depend on: threads, the file system or `app`; these are plain functions over what
//! the caller found.
//!
//! - Every option is given as `--flag=value`, so that a value can never be taken for an option,
//!   and `--mcp-config`, which takes any number of values, takes exactly one.
//! - A brief, when one is given, comes after `--`, which ends the options.
//! - A native session's (ADR 0165) starts with [`NATIVE`]: `claude -p` reading and writing
//!   stream-json. It takes no brief on its line: its first message goes on standard input.
//! - No permission option is given to a terminal's new session: not `--permission-mode`, not
//!   `--dangerously-skip-permissions`, not `--allowedTools`. A session starts in the mode the
//!   person's own settings give it, whoever asked for it. A native session's later processes
//!   (a resume, an interrupt's resume, a fork) are given `--permission-mode`, the mode it was
//!   in or the person switched it to, as `--resume` does not keep one (ADR 0165, part 3b):
//!   only one of [`PermissionMode`]'s, which names neither mode that skips asking. So is the
//!   TUI a native session is handed to (part 4d), with the model and effort it held, and only
//!   it: the caller gives a terminal's line none otherwise. A native session's line names
//!   [`PERMISSION_TOOL`] as its `--permission-prompt-tool`, so that a call that would ask is
//!   asked of the person in oso (ADR 0165, step 2), and its bridge is started with
//!   `--permission`, which lists that tool.

use std::ffi::OsString;
use std::path::Path;

use serde_json::json;

use super::identity::SessionId;
use crate::channel::protocol::mode::PermissionMode;

pub(crate) use crate::channel::mcp::SESSION_VARIABLE;

/// What a native session's command line starts with (ADR 0165): print mode, stream-json in
/// and out, every event, the parts of messages as they stream, the messages it took, and a
/// suggestion of the next message after each turn (part 3f, Q-F1 (a)).
pub const NATIVE: [&str; 7] = [
    "-p",
    "--input-format=stream-json",
    "--output-format=stream-json",
    "--verbose",
    "--include-partial-messages",
    "--replay-user-messages",
    "--prompt-suggestions=true",
];

/// The tool a native session's `claude` asks permission through: the `oso` server's
/// `permission`, which only a bridge started with `--permission` lists (ADR 0165, step 2).
pub const PERMISSION_TOOL: &str = "mcp__oso__permission";

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
    /// `--settings`, the JSON of a session's own settings: the hook that steers Claude away
    /// from its worktree tools (ADR 0121, Q15), when the person turns it on.
    pub settings: Option<&'a str>,
    /// `--add-dir`, one for each: an orchestrator's trusted repositories (ADR 0136, step 3).
    pub add_dirs: &'a [std::path::PathBuf],
    /// A native session's line (ADR 0165), which starts with [`NATIVE`] and takes no brief.
    pub native: bool,
    /// `--setting-sources=user`, a native session's in an issue space's own directory
    /// (ADR 0176): the person's own settings alone, none of the directory's or its parents'
    /// (hooks, MCP servers, commands, `CLAUDE.md`); `--settings` and `--mcp-config` still
    /// load. Never a terminal's.
    pub user_settings_only: bool,
    /// `--permission-mode`, a native session's mode once it ran (part 3b), and that of the
    /// TUI it is handed to (part 4d); never another terminal's.
    pub permission_mode: Option<PermissionMode>,
    /// `--model` and `--effort`, what the person chose in a native session with `/model` and
    /// `/effort` once the CLI took it (part 3c), for its processes and the TUI it is handed
    /// to; never another terminal's.
    pub model: Option<&'a str>,
    pub effort: Option<&'a str>,
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
/// executable as `mcp --pid PID`, and `--permission` for a `native` session's, with
/// [`SESSION_VARIABLE`] set. The person's other MCP servers still load.
pub fn mcp_config(
    oso: &Path,
    oso_pid: u32,
    id: &SessionId,
    native: bool,
) -> Result<String, LineError> {
    let command = oso.to_str().ok_or(LineError::NotUtf8)?;
    let mut args = vec!["mcp".to_owned(), "--pid".to_owned(), oso_pid.to_string()];
    if native {
        args.push("--permission".to_owned());
    }
    let config = json!({
        "mcpServers": {
            "oso": {
                "type": "stdio",
                "command": command,
                "args": args,
                "env": { SESSION_VARIABLE: id.as_str() },
            }
        }
    });
    Ok(config.to_string())
}

/// The arguments after `claude`.
pub fn arguments(line: &Line<'_>) -> Result<Vec<OsString>, LineError> {
    let mut args: Vec<OsString> = Vec::new();
    if line.native {
        args.extend(NATIVE.iter().map(OsString::from));
    }
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
    let config = mcp_config(line.oso, line.oso_pid, line.start.id(), line.native)?;
    // One `--add-dir=` each: the option takes any number of values, and so takes exactly one.
    for dir in line.add_dirs {
        let mut arg = OsString::from("--add-dir=");
        arg.push(dir);
        args.push(arg);
    }
    args.push(format!("--name={}", line.name).into());
    args.push(format!("--mcp-config={config}").into());
    if line.native {
        args.push(format!("--permission-prompt-tool={PERMISSION_TOOL}").into());
    }
    if line.native && line.user_settings_only {
        args.push("--setting-sources=user".into());
    }
    // A native session's, or the TUI's it is handed to (part 4d): the caller gives a
    // terminal's line none otherwise.
    if let Some(mode) = line.permission_mode {
        args.push(format!("--permission-mode={}", mode.name()).into());
    }
    // One argument each, `=` joined, as the values are the person's.
    if let Some(model) = line.model {
        args.push(format!("--model={model}").into());
    }
    if let Some(effort) = line.effort {
        args.push(format!("--effort={effort}").into());
    }
    if let Some(settings) = line.settings {
        args.push(format!("--settings={settings}").into());
    }
    if let Some(prompt) = line.system_prompt {
        args.push(format!("--append-system-prompt={prompt}").into());
    }
    if let Some(brief) = line.brief.filter(|_| !line.native) {
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
            settings: None,
            add_dirs: &[],
            native: false,
            user_settings_only: false,
            permission_mode: None,
            model: None,
            effort: None,
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
    fn settings_go_before_the_end_of_the_options() {
        let a = id(1);
        let mut l = line(Start::New { id: &a });
        l.settings = Some(r#"{"hooks":{}}"#);
        l.brief = Some("go");
        let args = strings(&arguments(&l).unwrap());
        let settings = args
            .iter()
            .position(|arg| arg == r#"--settings={"hooks":{}}"#);
        let end = args.iter().position(|arg| arg == "--").unwrap();
        assert!(settings.is_some_and(|at| at < end));
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
            "--settings=",
            "--add-dir=",
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
            l.settings = Some("{}");
            let dirs = [std::path::PathBuf::from("/src/back")];
            l.add_dirs = &dirs;
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
        assert!(super::mcp_config(path, 1, &id(1), false).is_err());
    }

    #[test]
    fn an_orchestrator_is_given_each_repository_as_one_add_dir() {
        let id = id(4);
        let dirs = [
            std::path::PathBuf::from("/src/back"),
            std::path::PathBuf::from("/src/front"),
        ];
        let line = Line {
            start: Start::New { id: &id },
            name: "fix-login",
            oso: Path::new("/bin/oso"),
            oso_pid: 1,
            system_prompt: Some("You are the orchestrator"),
            brief: None,
            settings: None,
            add_dirs: &dirs,
            native: false,
            user_settings_only: false,
            permission_mode: None,
            model: None,
            effort: None,
        };
        let args = arguments(&line).unwrap();
        let at = |text: &str| {
            args.iter()
                .position(|a| a.as_os_str() == std::ffi::OsStr::new(text))
                .unwrap()
        };
        let (back, front) = (at("--add-dir=/src/back"), at("--add-dir=/src/front"));
        assert!(back < front && front < at("--name=fix-login"));
        assert!(args.contains(&OsString::from(
            "--append-system-prompt=You are the orchestrator"
        )));
    }

    #[test]
    fn a_native_line_reads_stream_json_and_takes_no_brief() {
        let a = id(1);
        let native = Line {
            brief: Some("go on"),
            native: true,
            ..line(Start::New { id: &a })
        };
        let args = strings(&arguments(&native).unwrap());
        assert_eq!(args[..7], super::NATIVE.map(str::to_owned));
        assert_eq!(args[7], format!("--session-id={a}"));
        assert!(args.iter().all(|arg| arg != "--" && arg != "go on"));
        let asks = format!("--permission-prompt-tool={}", super::PERMISSION_TOOL);
        assert_eq!(args.iter().filter(|arg| **arg == asks).count(), 1);
        let config = args
            .iter()
            .find_map(|arg| arg.strip_prefix("--mcp-config="))
            .unwrap();
        let config: serde_json::Value = serde_json::from_str(config).unwrap();
        let oso_args = &config["mcpServers"]["oso"]["args"];
        assert_eq!(
            *oso_args,
            serde_json::json!(["mcp", "--pid", "4242", "--permission"])
        );
    }

    #[test]
    fn a_line_takes_the_mode_model_and_effort_it_is_given() {
        use crate::channel::protocol::mode::PermissionMode;
        let a = id(1);
        let modes = |native: bool, mode: Option<PermissionMode>| {
            let line = Line {
                native,
                permission_mode: mode,
                ..line(Start::Resume { id: &a })
            };
            let args = strings(&arguments(&line).unwrap());
            (args.iter())
                .filter(|arg| arg.starts_with("--permission-mode"))
                .cloned()
                .collect::<Vec<_>>()
        };
        assert_eq!(
            modes(true, Some(PermissionMode::AcceptEdits)),
            ["--permission-mode=acceptEdits"]
        );
        // The model and effort chosen go beside it (part 3c).
        let chosen = |native: bool| {
            let line = Line {
                native,
                model: Some("claude-opus-5-5[1m]"),
                effort: Some("high"),
                ..line(Start::Resume { id: &a })
            };
            let args = strings(&arguments(&line).unwrap());
            (args.iter())
                .filter(|arg| arg.starts_with("--model") || arg.starts_with("--effort"))
                .cloned()
                .collect::<Vec<_>>()
        };
        assert_eq!(
            chosen(true),
            ["--model=claude-opus-5-5[1m]", "--effort=high"]
        );
        // The TUI a native session is handed to takes what it held (part 4d); the caller
        // gives another terminal's line none.
        assert_eq!(chosen(false), chosen(true));
        assert_eq!(
            modes(true, Some(PermissionMode::Auto)),
            ["--permission-mode=auto"]
        );
        assert!(modes(true, None).is_empty(), "the settings' mode");
        assert!(modes(false, None).is_empty(), "the settings' mode");
        assert_eq!(
            modes(false, Some(PermissionMode::Plan)),
            ["--permission-mode=plan"]
        );
    }

    /// The CLI names the tool as `mcp__SERVER__TOOL`: the server is the one `--mcp-config`
    /// names, and the tool the one the bridge lists.
    /// A native session in a space's directory reads the person's own settings alone
    /// (ADR 0176); a terminal's line never says so.
    #[test]
    fn only_a_native_line_in_a_space_reads_the_persons_settings_alone() {
        let a = id(1);
        let sources = |native: bool, user_settings_only: bool| {
            let line = Line {
                native,
                user_settings_only,
                ..line(Start::Resume { id: &a })
            };
            let args = strings(&arguments(&line).unwrap());
            (args.iter())
                .filter(|arg| arg.starts_with("--setting-sources"))
                .count()
        };
        assert_eq!(sources(true, true), 1);
        assert_eq!(sources(true, false), 0);
        assert_eq!(sources(false, true), 0);
    }

    #[test]
    fn the_prompt_tool_is_the_one_the_bridge_lists() {
        let config = super::mcp_config(Path::new("/o"), 1, &id(1), true).unwrap();
        let config: serde_json::Value = serde_json::from_str(&config).unwrap();
        let server = config["mcpServers"]
            .as_object()
            .unwrap()
            .keys()
            .next()
            .unwrap()
            .clone();
        let tool = crate::channel::mcp::PERMISSION_TOOL;
        assert_eq!(super::PERMISSION_TOOL, format!("mcp__{server}__{tool}"));
    }

    #[test]
    fn a_terminal_line_names_no_permission_tool() {
        let a = id(1);
        let args = strings(&arguments(&line(Start::New { id: &a })).unwrap());
        assert!(args.iter().all(|arg| !arg.contains("permission")));
    }
}
