//! Naming a space with `claude` (ADR 0159, step 2): the command line gives no tool, no
//! permission option and no description; the answer is read from `structured_output` or
//! `result`, each name kept only if it holds; and a run against stand-in `claude`s takes the
//! description on its input, and is refused when it fails, says too much or takes too long.
//!
//! Must not depend on: `app` or a real `claude`; the stand-ins are shell scripts.

use super::{Named, answer, arguments, input};

#[test]
fn the_command_line_gives_no_tool_no_permission_and_no_description() {
    let args: Vec<String> = arguments()
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    for wanted in [
        "--print",
        "--safe-mode",
        "--tools=",
        "--strict-mcp-config",
        "--disable-slash-commands",
        "--no-session-persistence",
        "--output-format=json",
        "--model=haiku",
    ] {
        assert!(args.iter().any(|arg| arg == wanted), "{wanted}: {args:?}");
    }
    for refused in [
        "--permission",
        "--dangerously",
        "--allowed",
        "--bare",
        "--add-dir",
    ] {
        assert!(!args.iter().any(|arg| arg.starts_with(refused)), "{args:?}");
    }
    assert!(
        args.iter().all(|arg| arg.starts_with("--")),
        "options alone"
    );
    let given = String::from_utf8(input("Fix\nthe login")).unwrap();
    assert!(given.contains("===\nFix the login\n===\n"), "{given}");
}

#[test]
fn an_answer_gives_the_names_that_hold() {
    let display = " ログイン\u{202e} 修正 ";
    let structured = serde_json::json!({
        "type": "result",
        "is_error": false,
        "result": "ignored",
        "structured_output": { "display_name": display, "slug": "login-fix" },
    })
    .to_string();
    let structured = structured.as_bytes();
    assert_eq!(
        answer(structured),
        Ok(Named {
            display: Some("ログイン 修正".into()),
            slug: Some("login-fix".into()),
        })
    );
    let in_result = br#"{"is_error":false,"result":"{\"display_name\":\"Fix\",\"slug\":\"fix\"}"}"#;
    assert_eq!(
        answer(in_result),
        Ok(Named {
            display: Some("Fix".into()),
            slug: Some("fix".into()),
        })
    );
    // A slug is taken as it is or not at all: nothing makes one of what is not.
    for slug in ["../../x", "Fix", "ログイン", "fix--it", "-fix", "123", ""] {
        let text = format!(
            r#"{{"structured_output":{{"display_name":"Fix","slug":{}}}}}"#,
            serde_json::json!(slug)
        );
        let named = answer(text.as_bytes()).unwrap();
        assert_eq!(named.slug, None, "{slug}");
        assert_eq!(named.display.as_deref(), Some("Fix"));
    }
    let long = "a".repeat(49);
    let text = format!(r#"{{"structured_output":{{"display_name":"","slug":"{long}"}}}}"#);
    assert!(answer(text.as_bytes()).is_err(), "no name holds");
    assert!(answer(b"not json").is_err());
    assert!(answer(br#"{"is_error":true,"result":"{\"slug\":\"x\"}"}"#).is_err());
    assert!(answer(br#"{"result":"no json here"}"#).is_err());
}

/// Runs against stand-in `claude`s, shell scripts.
#[cfg(unix)]
mod runs {
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    use super::super::{Ask, DIRECTORY, run_within};

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "oso-naming-{name}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        /// A stand-in `claude` that runs `body`, executable.
        fn claude(&self, body: &str) -> PathBuf {
            use std::os::unix::fs::PermissionsExt;
            let path = self.0.join("claude");
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path
        }

        fn ask(&self, claude: PathBuf) -> Ask {
            let state = self.0.join("state");
            Ask {
                description: "Fix the login".into(),
                terminal: crate::terminal::shell::Settings {
                    shell: Some(PathBuf::from("/bin/sh")),
                    login: false,
                    ..Default::default()
                },
                claude: Some(claude),
                inherit: Vec::new(),
                state: oso_platform::privacy::PrivateDir::open(state)
                    .unwrap()
                    .path()
                    .to_path_buf(),
            }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_stand_in_claude_gets_the_description_on_its_input_in_an_empty_directory() {
        let scratch = Scratch::new("ok");
        let body = format!(
            "pwd -P > '{dir}/cwd'; printf '%s\\n' \"$@\" > '{dir}/args'; cat > '{dir}/in'\n\
             printf '{{\"structured_output\":{{\"display_name\":\"Login fix\",\"slug\":\"login-fix\"}}}}'",
            dir = scratch.0.display()
        );
        let ask = scratch.ask(scratch.claude(&body));
        let named = run_within(&ask, Duration::from_secs(10)).unwrap();
        assert_eq!(named.slug.as_deref(), Some("login-fix"));
        let given = std::fs::read_to_string(scratch.0.join("in")).unwrap();
        assert!(given.contains("Fix the login"), "{given}");
        let args = std::fs::read_to_string(scratch.0.join("args")).unwrap();
        assert!(!args.contains("Fix the login"), "{args}");
        let cwd = std::fs::read_to_string(scratch.0.join("cwd")).unwrap();
        let expected = std::fs::canonicalize(ask.state.join(DIRECTORY)).unwrap();
        assert_eq!(Path::new(cwd.trim()), expected);
    }

    #[test]
    fn a_failed_long_or_slow_claude_gives_no_names() {
        let scratch = Scratch::new("bad");
        let failed = scratch.ask(scratch.claude("cat > /dev/null; exit 1"));
        assert_eq!(
            run_within(&failed, Duration::from_secs(10)),
            Err("claude did not answer".into())
        );
        let long = scratch.ask(scratch.claude("cat > /dev/null; head -c 70000 /dev/zero"));
        assert_eq!(
            run_within(&long, Duration::from_secs(10)),
            Err("its answer was too long".into())
        );
        let slow = scratch.ask(scratch.claude("cat > /dev/null; sleep 30"));
        let started = Instant::now();
        assert_eq!(
            run_within(&slow, Duration::from_secs(1)),
            Err("no answer in 1 s".into())
        );
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "killed at its limit"
        );
        // A file in its directory, which `claude` could read as a setting: it is not run there.
        std::fs::write(slow.state.join(DIRECTORY).join("CLAUDE.md"), "x").unwrap();
        assert_eq!(
            run_within(&slow, Duration::from_secs(1)),
            Err("its directory is not empty".into())
        );
    }
}
