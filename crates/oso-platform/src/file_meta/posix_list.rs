//! The bytes of a POSIX access list as Linux keeps them in `system.posix_acl_access`: a version,
//! then a tag, rights and an ID for each entry. Only plain arithmetic, so it is tested everywhere.
//!
//! Must not depend on: `control`, `privacy`, `process` or `text`.

#![cfg_attr(
    not(target_os = "linux"),
    allow(dead_code, reason = "used only on Linux, and tested everywhere")
)]

use std::io;

const VERSION: u32 = 2;
const HEADER: usize = 4;
const ENTRY: usize = 8;

const USER_OBJ: u16 = 0x01;
const USER: u16 = 0x02;
const GROUP_OBJ: u16 = 0x04;
const GROUP: u16 = 0x08;
const MASK: u16 = 0x10;
const OTHER: u16 = 0x20;

/// Read and write.
const OWNER_RIGHTS: u16 = 0o6;

/// `list` with nothing open but to its owner, so that setting it on a file opens nothing to anyone
/// else: the owner reads and writes, and the mask and others get nothing. A named user or group counts
/// only through the mask, and the file's group too if there is one. The mode's group bits are the mask,
/// so setting the mode afterwards opens all of it in one step. With no mask, the group's rights go as well,
/// and the mode's group bits are those.
///
/// # Errors
///
/// If this is not such a list. What it would mean cannot then be told, so it must not be carried.
pub(super) fn shut(list: &[u8]) -> io::Result<Vec<u8>> {
    let malformed = || io::Error::other("an access list has a form this does not know");
    let version = list.first_chunk::<HEADER>().ok_or_else(malformed)?;
    if u32::from_le_bytes(*version) != VERSION || !(list.len() - HEADER).is_multiple_of(ENTRY) {
        return Err(malformed());
    }
    let entries = list[HEADER..].chunks_exact(ENTRY);
    let has_mask = entries.clone().any(|entry| tag_of(entry) == MASK);
    let mut out = list[..HEADER].to_vec();
    for entry in entries {
        let tag = tag_of(entry);
        let rights = match tag {
            USER_OBJ => OWNER_RIGHTS,
            MASK | OTHER => 0,
            GROUP_OBJ if !has_mask => 0,
            GROUP_OBJ | USER | GROUP => u16::from_le_bytes([entry[2], entry[3]]),
            _ => return Err(malformed()),
        };
        out.extend_from_slice(&tag.to_le_bytes());
        out.extend_from_slice(&rights.to_le_bytes());
        out.extend_from_slice(&entry[4..ENTRY]);
    }
    Ok(out)
}

fn tag_of(entry: &[u8]) -> u16 {
    u16::from_le_bytes([entry[0], entry[1]])
}

#[cfg(test)]
pub(super) fn bytes(entries: &[(u16, u16, u32)]) -> Vec<u8> {
    let mut list = VERSION.to_le_bytes().to_vec();
    for (tag, rights, id) in entries {
        list.extend_from_slice(&tag.to_le_bytes());
        list.extend_from_slice(&rights.to_le_bytes());
        list.extend_from_slice(&id.to_le_bytes());
    }
    list
}

#[cfg(test)]
mod tests {
    use super::{GROUP, GROUP_OBJ, MASK, OTHER, USER, USER_OBJ, bytes, shut};

    const NO_ID: u32 = u32::MAX;

    #[test]
    fn only_the_owner_is_left_open_and_the_rest_waits_for_the_mask() {
        let list = bytes(&[
            (USER_OBJ, 7, NO_ID),
            (USER, 6, 1000),
            (GROUP_OBJ, 0, NO_ID),
            (GROUP, 4, 2000),
            (MASK, 6, NO_ID),
            (OTHER, 4, NO_ID),
        ]);
        let wanted = bytes(&[
            (USER_OBJ, 6, NO_ID),
            (USER, 6, 1000),
            (GROUP_OBJ, 0, NO_ID),
            (GROUP, 4, 2000),
            (MASK, 0, NO_ID),
            (OTHER, 0, NO_ID),
        ]);
        assert_eq!(shut(&list).unwrap(), wanted);
    }

    #[test]
    fn with_no_mask_the_group_goes_too() {
        let list = bytes(&[
            (USER_OBJ, 6, NO_ID),
            (GROUP_OBJ, 6, NO_ID),
            (OTHER, 4, NO_ID),
        ]);
        let wanted = bytes(&[
            (USER_OBJ, 6, NO_ID),
            (GROUP_OBJ, 0, NO_ID),
            (OTHER, 0, NO_ID),
        ]);
        assert_eq!(shut(&list).unwrap(), wanted);
    }

    #[test]
    fn with_a_mask_the_groups_rights_stay_for_when_it_opens() {
        let list = bytes(&[(GROUP_OBJ, 6, NO_ID), (MASK, 6, NO_ID)]);
        let wanted = bytes(&[(GROUP_OBJ, 6, NO_ID), (MASK, 0, NO_ID)]);
        assert_eq!(shut(&list).unwrap(), wanted);
    }

    #[test]
    fn what_this_does_not_know_is_refused() {
        let good = bytes(&[(USER_OBJ, 6, NO_ID)]);
        assert!(shut(&good).is_ok());
        assert!(shut(&[]).is_err(), "no version");
        assert!(shut(&good[..2]).is_err(), "half a version");
        assert!(shut(&good[..good.len() - 1]).is_err(), "half an entry");
        let mut later = good.clone();
        later[0] = 3;
        assert!(shut(&later).is_err(), "another version");
        assert!(shut(&bytes(&[(0x40, 6, NO_ID)])).is_err(), "another tag");
    }
}
