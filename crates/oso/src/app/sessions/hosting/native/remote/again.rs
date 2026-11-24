//! A session connected to claude.ai across `oso-restart` (ADR 0181): what the state carries of it,
//! and whether the new instance connects it again, which asks nothing, as it is no turning on.
//!
//! Must not depend on: key presses; `restarting` carries it and resumes the session.
//!
//! - Carried only where the person has it on and an answer gave the session's id there, with the
//!   directory and the identities its start thread read; not of a conversation that moved. Windows
//!   gives no identities, so nothing is carried there.
//! - Again only of the parent's state, for the conversation it names, in the directory the record
//!   has, which must be the one carried; as a session that may be connected at all; under the
//!   trust judged by its start thread, which compares the identities. The answer must give the
//!   same id. Any of these otherwise, it resumes off, and says so.

use crate::app::App;
use crate::carry::natives::{Agents, RemoteResume};
use crate::sessions::identity::SessionId;
use crate::sessions::native::wire::remote::RemoteId;
use crate::sessions::start::connect::{Back, Connect, Place};
use crate::sessions::worker::SessionKey;

impl App {
    /// The conversations forked from an agent's session that oso still has, for a restart's state.
    pub(in crate::app) fn natives_of_agents(&self) -> Agents {
        let natives = &self.hosting.natives;
        if natives.remotes.any_may_be_agents {
            return Agents::Unknown;
        }
        let named: Vec<String> = (self.hosting.table.iter())
            .filter(|session| natives.remotes.from_agents.contains(&session.id))
            .map(|session| session.id.as_str().to_owned())
            .collect();
        match named.is_empty() {
            true => Agents::None,
            false => Agents::Named(named),
        }
    }

    /// The instance before named `agents` so.
    pub(in crate::app) fn natives_of_agents_carried(&mut self, agents: Agents) {
        let natives = &mut self.hosting.natives;
        match agents {
            Agents::None => {}
            Agents::Named(ids) => {
                let ids = ids.iter().filter_map(|id| SessionId::parse(id));
                natives.remotes.from_agents.extend(ids);
            }
            Agents::Unknown => natives.remotes.any_may_be_agents = true,
        }
    }

    /// How native session `key` is connected, for a restart's state.
    pub(in crate::app::sessions::hosting::native) fn remote_carried(
        &self,
        key: SessionKey,
    ) -> Option<RemoteResume> {
        let native = (self.hosting.natives.map.get(&key)).filter(|native| !native.moved)?;
        if native.remote_control.respawns() || self.remote_never(key) {
            return None;
        }
        let place = native.remote_control.place?;
        let directory = self.hosting.table.get(key)?.directory.to_str()?;
        Some(RemoteResume {
            id: native.remote.carried()?.as_str().to_owned(),
            directory: directory.to_owned(),
            place: place.directory,
            repository: place.repository,
        })
    }

    /// Leave to connect native session `key` again as `carried` says, if it may be; else it stays
    /// off, and says so.
    pub(in crate::app::sessions::hosting::native) fn remote_again(
        &mut self,
        key: SessionKey,
        carried: Option<&RemoteResume>,
    ) -> Option<Connect> {
        let carried = carried?;
        let here = (self.hosting.table.get(key)).map(|session| session.directory.clone());
        let refused = match RemoteId::read(&carried.id) {
            None => Some("what the restart carried of it does not read"),
            Some(_) if here.as_deref() != Some(carried.directory.as_ref()) => {
                Some("its directory is not the one it was connected in")
            }
            Some(_) if self.remote_never(key) => Some("it is a session that is never connected"),
            Some(_) => None,
        };
        if let Some(why) = refused {
            self.remote_stays_off(key, why);
            return None;
        }
        let id = RemoteId::read(&carried.id)?;
        let name = self.remote_name(key);
        self.native_note_row(
            key,
            "Remote Control was on before oso-restart: connecting to claude.ai again.",
        );
        self.editor
            .message(format!("Connecting {name} to claude.ai again…"));
        Some(Connect::again(Back {
            id,
            place: Place {
                directory: carried.place,
                repository: carried.repository,
            },
        }))
    }
}
