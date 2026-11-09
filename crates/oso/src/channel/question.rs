//! What the agent channel's connection question says (ADR 0085): who reports itself as
//! connecting, as the system names it, with names escaped and quoted so that no name can
//! change how the question reads.
//!
//! Must not depend on: sockets, threads or editor state; it only words what it is given.

use oso_platform::control::{Peer, PeerProcess};

/// What the connection's question says: who reports itself as connecting, as the system
/// names them. `oso mcp` is named so only when the process runs this program; its parent
/// counts only then, and not when it is the system's first process, which adopts orphans.
pub fn connection(peer: &Peer) -> String {
    let named = |process: &PeerProcess| match process.name() {
        Some(name) => format!("\"{name}\" (pid {})", process.pid()),
        None => format!("an unnamed process (pid {})", process.pid()),
    };
    match (&peer.process, peer.runs_this_program) {
        (Some(process), true) => {
            let parent = peer
                .parent
                .as_ref()
                .filter(|parent| parent.pid() > 1)
                .map_or_else(|| "an unknown process".to_owned(), named);
            format!(
                "let a process that reports itself as {parent} control oso through oso mcp (pid {})?",
                process.pid()
            )
        }
        (Some(process), false) => {
            format!(
                "let a process that reports itself as {} control oso?",
                named(process)
            )
        }
        (None, _) => "let a process the system does not name control oso?".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::connection;
    use oso_platform::control::{Peer, PeerProcess};

    fn process(pid: u32, name: &str) -> Option<PeerProcess> {
        Some(PeerProcess::new(pid, Some(name)))
    }

    #[test]
    fn names_the_bridge_and_who_started_it_only_when_it_is_this_program() {
        let peer = Peer {
            process: process(815, "oso"),
            parent: process(812, "claude"),
            runs_this_program: true,
        };
        assert_eq!(
            connection(&peer),
            "let a process that reports itself as \"claude\" (pid 812) control oso through oso mcp (pid 815)?"
        );
        let impostor = Peer {
            runs_this_program: false,
            ..peer.clone()
        };
        assert_eq!(
            connection(&impostor),
            "let a process that reports itself as \"oso\" (pid 815) control oso?"
        );
    }

    #[test]
    fn an_adopted_or_unknown_parent_is_unknown() {
        for parent in [process(1, "launchd"), None] {
            let peer = Peer {
                process: process(815, "oso"),
                parent,
                runs_this_program: true,
            };
            assert!(connection(&peer).contains("an unknown process control oso"));
        }
        let unnamed = Peer {
            process: Some(PeerProcess::new(9, None)),
            parent: None,
            runs_this_program: false,
        };
        assert!(connection(&unnamed).contains("an unnamed process (pid 9)"));
        assert!(connection(&Peer::default()).contains("does not name"));
    }

    #[test]
    fn a_name_cannot_rewrite_the_question() {
        let peer = Peer {
            process: process(815, "oso\u{202e}?n ro y"),
            parent: None,
            runs_this_program: false,
        };
        let question = connection(&peer);
        assert!(question.is_ascii(), "{question}");
        assert!(question.contains("\\u{202e}"), "{question}");
    }
}
