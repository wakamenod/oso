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
use crate::carry::natives::RemoteResume;
use crate::sessions::native::wire::remote::RemoteId;
use crate::sessions::start::connect::{Back, Connect, Place};
use crate::sessions::worker::SessionKey;

impl App {
    /// How native session `key` is connected, for a restart's state.
    pub(in crate::app::sessions::hosting::native) fn remote_carried(
        &self,
        key: SessionKey,
    ) -> Option<RemoteResume> {
        let native = (self.hosting.natives.map.get(&key)).filter(|native| !native.moved)?;
        if native.remote_control.respawns() {
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
            Some(_) => None,
        };
        if let Some(why) = refused {
            // Nor connected afresh as Claude Code's settings say: it stays off (ADR 0189).
            self.hosting.natives.remotes.turned_off.insert(key);
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
