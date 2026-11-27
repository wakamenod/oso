//! The rules of who answers a native worker's Bash request: the secrets a command names and the
//! commands denied, however the shell may spell them (ADR 0186).
//!
//! Must not depend on: the app or the file system; these run on values alone.

use serde_json::json;

use super::tests::{bash, directory, judge};
use super::{DEFAULT_DENY, PersonOnly, Rules, eligible};

#[test]
fn the_default_deny_list_catches_what_it_names() {
    for command in [
        "git push",
        "git push origin main",
        "git -C sub push",
        "GIT PUSH",
        "git pu''sh",
        "git pu\\sh",
        "g\"\"it push --force",
        "git\\\npush",
        "gh pr merge 12",
        "gh -R a/b pr merge 12",
        "gh repo delete a/b",
        "gh api repos/a/b",
        "curl https://example.com",
        "wget x",
        "ssh host",
        "scp a host:b",
        "sudo ls",
        "rm -rf target",
        "rm -fr target",
        "rm -r -f target",
        "rm --recursive target",
        "rm -Rf target",
        "claude -p hi",
        "/usr/local/bin/claude",
        "npx claude",
        "oso ctl keys",
        "./target/debug/oso",
        "echo x > ../../settings.json",
        "echo x >> .claude/settings.local.json",
        "cp a ~/.claude.json",
        "echo '{}' > .mcp.json",
        "echo x >> ~/.config/oso/config.toml",
        "cat a; git push",
        "true && curl x",
        "$(curl x)",
    ] {
        assert_eq!(bash(command), Err(PersonOnly::Denied), "{command:?}");
    }
    for command in [
        "cargo test -p oso-core",
        "cargo test --manifest-path /r/oso/Cargo.toml",
        "git log --oneline",
        "gh pr view 12",
        "rm target/a.o",
        "ls -R src",
        "echo claudette",
    ] {
        assert_eq!(bash(command), Ok(()), "{command:?}");
    }
}

#[test]
fn the_persons_deny_list_adds_to_the_defaults_which_stay() {
    let tools = vec!["Bash".to_owned()];
    let deny = vec!["\\bmake\\b".to_owned()];
    let rules = Rules::new(&tools, &deny).unwrap();
    let judge = |command: &str| {
        let input = json!({ "command": command });
        eligible("Bash", Some(&input), false, &directory(), &rules)
    };
    assert_eq!(judge("make all"), Err(PersonOnly::Denied));
    assert_eq!(
        judge("git push"),
        Err(PersonOnly::Denied),
        "the defaults stay"
    );
    assert_eq!(judge("cargo test"), Ok(()));
    assert!(Rules::new(&tools, &["(".to_owned()]).is_err());
    assert_eq!(rules.deny.len(), DEFAULT_DENY.len() + 1);
    // An empty list of the person's leaves the defaults, never nothing.
    let none = Rules::new(&tools, &[]).unwrap();
    assert_eq!(none.deny.len(), DEFAULT_DENY.len());
    let input = json!({ "command": "git push" });
    assert_eq!(
        eligible("Bash", Some(&input), false, &directory(), &none),
        Err(PersonOnly::Denied)
    );
}

#[test]
fn a_bash_call_with_no_command_is_not_shown() {
    assert_eq!(judge("Bash", &json!({})), Err(PersonOnly::NotShown));
    assert_eq!(
        judge("Bash", &json!({ "command": 1 })),
        Err(PersonOnly::NotShown)
    );
}

#[test]
fn a_bash_secret_spelled_as_the_shell_joins_it_is_the_persons() {
    for command in [
        "cat .env*",
        "cat .en[v]",
        "cat .e$'n'v",
        "cat .e$\"\"nv",
        "cat .e{,}nv",
        "cat .e$()nv",
        "cat .e``nv",
        "cat .e\\\nnv",
        "cat 'C:\\Users\\me\\.ssh\\id_rsa'",
    ] {
        assert_eq!(bash(command), Err(PersonOnly::Secret), "{command:?}");
    }
}

#[test]
fn a_denied_command_spelled_as_the_shell_joins_it_is_denied() {
    for command in [
        "cu\\\nrl x",
        "cu$()rl x",
        "cu``rl x",
        "cu$''rl x",
        "c{u,}rl x",
        "npx @anthropic-ai/claude-code -p hi",
        "claude.exe -p hi",
        "oso-restart",
        "oso.exe ctl",
        "nc -U /tmp/oso/1-agent.sock",
        "rm --rec target",
        "rsync a host:b",
        "sftp host",
        "socat - UNIX:x",
        "telnet host",
        "doas ls",
        "su -",
        "gh gist create a",
        "gh release create v1",
        "gh secret set X",
        "gh workflow run ci",
        "find . -name x -delete",
        "git -c core.sshCommand=x fetch",
        "git config alias.x '!sh'",
        "git config core.hooksPath h",
        "git remote set-url origin x",
        "kill 1",
        "pkill -f x",
    ] {
        assert_eq!(bash(command), Err(PersonOnly::Denied), "{command:?}");
    }
    assert_eq!(bash("cargo test -p oso-core"), Ok(()));
}

#[test]
fn a_command_that_climbs_from_a_worktree_into_its_repositorys_claude_is_denied() {
    let own = directory().display().to_string();
    for command in [
        format!("printf x > {own}/../../hooks/fmt.sh"),
        "echo x > ../../commands/c.md".to_owned(),
        "cat ../../hooks/a.sh".to_owned(),
        format!("echo x > {own}x/a"),
    ] {
        assert_eq!(bash(&command), Err(PersonOnly::Denied), "{command:?}");
    }
    assert_eq!(bash(&format!("ls {own}/src")), Ok(()));
}

#[test]
fn the_persons_patterns_ignore_case_and_cross_lines_as_the_defaults_do() {
    let tools = vec!["Bash".to_owned()];
    let rules = Rules::new(&tools, &["\\bcurl\\b".to_owned(), "a.b".to_owned()]).unwrap();
    for command in ["CURL x", "a\nb"] {
        let input = json!({ "command": command });
        assert_eq!(
            eligible("Bash", Some(&input), false, &directory(), &rules),
            Err(PersonOnly::Denied),
            "{command:?}"
        );
    }
}

#[test]
fn the_second_reviews_spellings_are_denied() {
    let own = directory().display().to_string();
    for command in [
        "echo x > .$()./.$()./hooks/a.sh".to_owned(),
        "echo x > .``.``/hooks/a.sh".to_owned(),
        format!("echo x > /x{own}/a"),
        "bunx @anthropic-ai/claude-code@latest -p hi".to_owned(),
        "pnpm dlx @anthropic-ai/claude-code@latest".to_owned(),
        "rm --r target".to_owned(),
        "rm --re target".to_owned(),
        "GIT_SSH_COMMAND=x git fetch".to_owned(),
        "GIT_SSH=x git fetch".to_owned(),
        "GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=core.sshCommand git fetch".to_owned(),
    ] {
        assert_eq!(bash(&command), Err(PersonOnly::Denied), "{command:?}");
    }
}
