//! What the app keeps for renaming sessions (ADR 0155): the rename the person was asked
//! about, and the names Claude Code's registry gives oso's own running sessions, which its
//! `SendMessage` knows them by; and every name a session answers to, which a rename and a new
//! space's slug may not take (ADR 0159).
//!
//! Must not depend on: key presses or how a frame is drawn.

use std::collections::HashMap;

use oso_core::minibuffer::PromptId;

use crate::sessions::hosted::{Run, Table};
use crate::sessions::identity::{SessionId, cleaned};
use crate::sessions::worker::SessionKey;
use crate::sessions::worker::externals::External;

/// The rename asked, and the registry's names.
#[derive(Debug, Default)]
pub(in crate::app) struct Naming {
    /// A rename's prompt, and the session it renames on its answer.
    asked: Option<(PromptId, SessionKey)>,
    /// The registry's name of each of oso's running sessions, by key, a clean one.
    messaging: HashMap<SessionKey, String>,
}

impl Naming {
    /// The rename asked last: its prompt, and the session it renames on its answer.
    pub(in crate::app) fn asked(&self) -> Option<(PromptId, SessionKey)> {
        self.asked
    }

    /// Notes the rename asked: its prompt, and the session it renames on its answer.
    pub(in crate::app) fn set_asked(&mut self, prompt: PromptId, key: SessionKey) {
        self.asked = Some((prompt, key));
    }

    /// Takes the rename asked, which is then forgotten.
    pub(in crate::app) fn take_asked(&mut self) -> Option<(PromptId, SessionKey)> {
        self.asked.take()
    }

    /// The name Claude Code's registry gives session `key`, if the poll saw one.
    pub(in crate::app) fn messaging_name(&self, key: SessionKey) -> Option<&str> {
        self.messaging.get(&key).map(String::as_str)
    }

    /// Keeps what the registry's `live` sessions call those of `table` that run, by the id oso
    /// gave each. Any process of the person's may write the registry, so a name counts only
    /// from the entry of the session's own `claude`, oso's child by its process id, and only
    /// when it is a name as oso keeps one.
    pub(in crate::app) fn read_registry(&mut self, live: &[External], table: &Table) {
        let running: HashMap<&SessionId, (SessionKey, u32)> = (table.iter())
            .filter_map(|session| match session.run {
                Run::Running { child: Some(child) } => Some((&session.id, (session.key, child))),
                _ => None,
            })
            .collect();
        self.messaging = (live.iter())
            .filter_map(|external| {
                let (key, child) = *running.get(external.session_id.as_ref()?)?;
                let name = external.name.as_deref()?;
                let clean = cleaned(name).is_some_and(|kept| kept == name);
                (external.pid == child && clean).then(|| (key, name.to_owned()))
            })
            .collect();
    }

    /// The poll stopped: what it saw goes.
    pub(in crate::app) fn forget_registry(&mut self) {
        self.messaging.clear();
    }
}

impl crate::app::App {
    /// Every name a session oso knows of answers to, but session `except`'s: each hosted
    /// one's, being started or not, the name each running one was started with or the
    /// registry gives it, and other programs' (ADR 0155). A new space's slug is none of them
    /// either (ADR 0159).
    pub(in crate::app) fn session_names(&self, except: Option<SessionKey>) -> Vec<&str> {
        let table = self.hosting.table();
        let others = || {
            table
                .iter()
                .filter(move |session| Some(session.key) != except)
        };
        let answered = others()
            .filter(|session| session.runs())
            .flat_map(|session| {
                let registry = self.hosting.naming.messaging_name(session.key);
                registry.into_iter().chain(session.launched_as.as_deref())
            });
        (others().map(|session| session.name.as_str()))
            .chain(answered)
            .chain(table.held_names())
            .chain(self.watch.external_names())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::Naming;
    use crate::sessions::hosted::{New, Run, Table};
    use crate::sessions::identity::{Origin, SessionId};
    use crate::sessions::worker::externals::External;

    fn entry(n: u8, pid: u32, name: &str) -> External {
        External {
            pid,
            session_id: Some(SessionId::from_random([n; 16])),
            name: Some(name.to_owned()),
            directory: Some(PathBuf::from("/p")),
            working: false,
            status_changed: None,
            place: None,
            repo: None,
        }
    }

    /// A name counts only from the session's own `claude`, and only a clean one (ADR 0155):
    /// another process's entry with the session's id, or a name oso would not keep, is not
    /// taken, and a stopped session has none.
    #[test]
    fn a_registry_name_counts_only_from_the_sessions_own_clean_entry() {
        let mut table = Table::default();
        let mut add = |n: u8, run: Run| {
            let (key, _) = table.reserve(Path::new("/p"), &format!("s{n}"));
            let new = New {
                native: false,
                id: SessionId::from_random([n; 16]),
                root: None,
                parent: None,
                origin: Origin::Person,
                unasked_brief: None,
                changed: 1,
            };
            table.add(key, new).unwrap().run = run;
            key
        };
        let own = add(1, Run::Running { child: Some(100) });
        let forged = add(2, Run::Running { child: Some(200) });
        let unclean = add(3, Run::Running { child: Some(300) });
        let stopped = add(4, Run::Stopped);
        let live = [
            entry(1, 100, "api"),
            entry(2, 999, "lead"),
            entry(3, 300, "two words"),
            entry(4, 400, "old"),
        ];
        let mut naming = Naming::default();
        naming.read_registry(&live, &table);
        assert_eq!(naming.messaging_name(own), Some("api"));
        for none in [forged, unclean, stopped] {
            assert_eq!(naming.messaging_name(none), None, "{none}");
        }
        naming.forget_registry();
        assert_eq!(naming.messaging_name(own), None);
    }
}
