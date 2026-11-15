//! The record an instance leaves in the control directory of the instances it took over from,
//! over `oso dev`'s restart (ADR 0120, step 11) or `oso-restart` (ADR 0133), and finding in it
//! the instance that replaced one a bridge's `--pid` named.
//!
//! Must not depend on: the protocol; the connection is checked as any other.

use oso_platform::control::ControlDir;

/// The instance that took over from `pid` in a restart, as the record each one leaves beside
/// its socket says: the newest of those that list it (ADR 0120, step 11). The connection is
/// then checked as any other.
pub(super) fn follower(pid: u32) -> Option<u32> {
    let dir = ControlDir::open_existing().ok()?;
    let suffix = format!("-{FOLLOWS}");
    std::fs::read_dir(dir.path())
        .ok()?
        .take(MAX_LOOKED)
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let follower: u32 = name.strip_suffix(&suffix)?.parse().ok()?;
            // A plain file only, not followed through a link: a FIFO would block the read.
            if !entry.file_type().ok()?.is_file() {
                return None;
            }
            let file = std::fs::File::open(entry.path()).ok()?;
            let mut text = String::new();
            std::io::Read::read_to_string(&mut std::io::Read::take(file, 4096), &mut text).ok()?;
            let followed = parse_follows(&text);
            // The newest follower lists the most instances before it.
            followed
                .contains(&pid)
                .then_some((followed.len(), follower))
        })
        .max()
        .map(|(_, follower)| follower)
}

/// Writes `<pid>-follows` on a thread of its own: `pids`, the instances this one took over
/// from, for a session's bridge pinned to one of them (ADR 0120, step 11; ADR 0133). It goes
/// with this instance's files when its socket is cleaned up.
pub fn write_follows(pids: &[u32]) {
    let text = follows_text(pids);
    let spawned = std::thread::Builder::new()
        .name("oso-follows".into())
        .spawn(move || {
            let written =
                ControlDir::open().and_then(|dir| dir.write_private_file(FOLLOWS, text.as_bytes()));
            if let Err(e) = written {
                log::warn!("the instances this one follows were not written: {e}");
            }
        });
    if let Err(e) = spawned {
        log::warn!("cannot start the follows thread: {e}");
    }
}

/// The record of the instances `pids` an instance took over from, one id a line, newest last.
pub fn follows_text(pids: &[u32]) -> String {
    pids.iter().map(|pid| format!("{pid}\n")).collect()
}

/// The instances a record names, each once, in its order.
pub(super) fn parse_follows(text: &str) -> Vec<u32> {
    let mut pids: Vec<u32> = Vec::new();
    for pid in text.lines().filter_map(|line| line.trim().parse().ok()) {
        if !pids.contains(&pid) {
            pids.push(pid);
        }
    }
    pids
}

/// The name of the file, beside an instance's socket, that lists the instances it took over
/// from in `oso dev`'s restart, which a dev build writes and this reads.
pub const FOLLOWS: &str = "follows";

/// The most names in the control directory looked at for a follower.
const MAX_LOOKED: usize = 1024;
