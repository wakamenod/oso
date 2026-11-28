//! The native sessions a restart resumes (ADR 0165, part 4c): each one whose `claude -p` ran in
//! the instance going, by its session id, with the permission mode, model and effort level its
//! next process starts with (Q4-2). The state carries nothing else of it: the new instance
//! finds the session in the record it reads itself, with its name, directory and root, and
//! resumes only a native one it finds there.
//!
//! Must not depend on: `app` or `sessions`; these are values. The app reads each field again
//! as a session's id, a mode, a model and a level, and leaves out what does not read so.
//!
//! An entry that is not one this writes is left out, never refusing the restart: its session
//! comes back stopped, as before part 4c. Past [`MAX_NATIVE_RESUMES`], the rest are left out
//! too. A reader from before part 4c knows no such field, and brings every session back
//! stopped.

use serde_json::{Map, Value, json};

use crate::channel::protocol::mode::PermissionMode;

/// The most native sessions a state resumes.
pub const MAX_NATIVE_RESUMES: usize = 64;

/// The longest model a state carries, as a session's choice keeps it (`wire::MAX_NAME`).
const MAX_MODEL: usize = 256;

/// The longest effort level a state carries: `--effort`'s are shorter.
const MAX_EFFORT: usize = 16;

/// The longest permission mode a state carries: the modes oso sends are shorter.
const MAX_MODE: usize = 16;

/// A native session the new instance resumes, once the instance going has gone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeResume {
    /// Its session id, which the app reads as one: 36 hexadecimal digits and dashes here.
    pub id: String,
    /// The permission mode its next process starts in, if it is given one (part 3b).
    pub mode: Option<PermissionMode>,
    /// The model and effort level the person chose (part 3c).
    pub model: Option<String>,
    pub effort: Option<String>,
    /// How it is connected to claude.ai, if the person has it on (ADR 0181). One that does not read
    /// is none: the session resumes off.
    pub remote: Option<RemoteResume>,
}

/// How a session is connected to claude.ai (ADR 0181): what must be as it was for the new instance
/// to connect it again.
#[derive(Clone, PartialEq, Eq)]
pub struct RemoteResume {
    /// The session's id there.
    pub id: String,
    pub directory: String,
    /// The directory's device and inode, and its repository's.
    pub place: (u64, u64),
    pub repository: Option<(u64, u64)>,
}

impl std::fmt::Debug for RemoteResume {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RemoteResume(..)")
    }
}

/// The longest id there and directory a state carries.
const MAX_REMOTE_ID: usize = 128;
const MAX_REMOTE_DIRECTORY: usize = 4096;

/// The state's field for `resumes`, the first [`MAX_NATIVE_RESUMES`].
fn state(resumes: &[NativeResume]) -> Value {
    resumes
        .iter()
        .take(MAX_NATIVE_RESUMES)
        .map(|resume| {
            json!({
                "id": resume.id,
                "mode": resume.mode.map(PermissionMode::name),
                "model": resume.model,
                "effort": resume.effort,
                "remote": resume.remote.as_ref().map(|remote| json!({
                    "id": remote.id,
                    "directory": remote.directory,
                    "place": remote.place,
                    "repository": remote.repository,
                })),
            })
        })
        .collect()
}

/// Adds `resumes` to the exported `state`, unless they would not fit: they never refuse the
/// restart, and their sessions then come back stopped.
pub fn add(state: &mut Value, resumes: &[NativeResume]) {
    if resumes.is_empty() {
        return;
    }
    state["native_resumes"] = self::state(resumes);
    if super::fits(state).is_err() {
        log::info!("the native sessions to resume are left out of the restart's state");
        if let Some(state) = state.as_object_mut() {
            state.remove("native_resumes");
        }
    }
}

/// The resumes `state` carries, each once, at most [`MAX_NATIVE_RESUMES`]; none if it says
/// none. An entry that does not read is left out.
pub(super) fn parse(state: &Map<String, Value>) -> Vec<NativeResume> {
    let Some(entries) = state.get("native_resumes").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut resumes: Vec<NativeResume> = Vec::new();
    for entry in entries.iter().take(MAX_NATIVE_RESUMES) {
        let Some(resume) = entry.as_object().and_then(parse_entry) else {
            continue;
        };
        if resumes.iter().all(|kept| kept.id != resume.id) {
            resumes.push(resume);
        }
    }
    resumes
}

/// The most conversations turned off that a state names.
const MAX_TURNED_OFF: usize = 1024;

/// Adds the conversations the person turned off (ADR 0189) to the exported `state`, after the
/// resumes; where they do not fit, all are off (`true`).
pub fn add_turned_off(state: &mut Value, ids: &[String]) {
    if ids.is_empty() {
        return;
    }
    state["native_remote_off"] = match ids.len() <= MAX_TURNED_OFF {
        true => json!(ids),
        false => json!(true),
    };
    if super::fits(state).is_err() {
        state["native_remote_off"] = json!(true);
    }
}

/// The conversations `state` says were turned off; `Err` where it says all, or does not read, so
/// that every one is off: what it says only turns off.
///
/// # Errors
///
/// Where every conversation is off.
pub(super) fn parse_turned_off(state: &Map<String, Value>) -> Result<Vec<String>, ()> {
    match state.get("native_remote_off") {
        None => Ok(Vec::new()),
        Some(Value::Array(ids)) if ids.len() <= MAX_TURNED_OFF => {
            ids.iter().map(|id| session_id(id).ok_or(())).collect()
        }
        Some(_) => Err(()),
    }
}

/// A session's id, 36 hexadecimal digits and dashes (ADR 0121), which the app reads as one.
pub(super) fn session_id(value: &Value) -> Option<String> {
    let id = value.as_str()?;
    (id.len() == 36 && id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-'))
        .then(|| id.to_owned())
}

fn parse_entry(entry: &Map<String, Value>) -> Option<NativeResume> {
    let id = session_id(entry.get("id")?)?;
    // A field that is there is a string that reads, or null.
    let text = |name: &str, longest: usize| match entry.get(name) {
        None | Some(Value::Null) => Some(None),
        Some(Value::String(text)) if !text.is_empty() && text.len() <= longest => {
            Some(Some(text.clone()))
        }
        Some(_) => None,
    };
    let mode = match text("mode", MAX_MODE)? {
        Some(name) => Some(PermissionMode::named(&name)?),
        None => None,
    };
    Some(NativeResume {
        id,
        mode,
        model: text("model", MAX_MODEL)?,
        effort: text("effort", MAX_EFFORT)?,
        remote: entry.get("remote").and_then(parse_remote),
    })
}

/// How a session is connected, if `value` reads whole.
fn parse_remote(value: &Value) -> Option<RemoteResume> {
    let remote = value.as_object()?;
    let text = |name: &str, longest: usize| {
        let text = remote.get(name)?.as_str()?;
        (!text.is_empty() && text.len() <= longest).then(|| text.to_owned())
    };
    let pair = |value: &Value| match value.as_array()?.as_slice() {
        [device, inode] => Some((device.as_u64()?, inode.as_u64()?)),
        _ => None,
    };
    Some(RemoteResume {
        id: text("id", MAX_REMOTE_ID)?,
        directory: text("directory", MAX_REMOTE_DIRECTORY)?,
        place: pair(remote.get("place")?)?,
        repository: match remote.get("repository")? {
            Value::Null => None,
            value => Some(pair(value)?),
        },
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::carry::tests::{absolute, visiting};
    use crate::carry::{self, export_value};

    const ID: &str = "0123abcd-0000-4000-8000-000000000001";
    const OTHER: &str = "0123abcd-0000-4000-8000-000000000002";

    fn parsed(field: Value) -> Vec<NativeResume> {
        let state = json!({ "native_resumes": field });
        parse(state.as_object().unwrap())
    }

    fn resume(id: &str) -> NativeResume {
        NativeResume {
            id: id.to_owned(),
            mode: None,
            model: None,
            effort: None,
            remote: None,
        }
    }

    fn remote() -> RemoteResume {
        RemoteResume {
            id: "cse_abc".into(),
            directory: "/w/oso".into(),
            place: (1, u64::MAX),
            repository: Some((3, 4)),
        }
    }

    #[test]
    fn a_connection_goes_and_comes_back_whole_or_not_at_all() {
        for remote in [
            remote(),
            RemoteResume {
                repository: None,
                ..remote()
            },
        ] {
            let resumes = vec![NativeResume {
                remote: Some(remote),
                ..resume(ID)
            }];
            assert_eq!(parsed(state(&resumes)), resumes);
        }
        let whole = json!({
            "id": "cse_abc", "directory": "/w", "place": [1, 2], "repository": null,
        });
        let off = |change: &dyn Fn(&mut Map<String, Value>)| {
            let mut remote = whole.clone();
            change(remote.as_object_mut().unwrap());
            let read = parsed(json!([{ "id": ID, "remote": remote }]));
            assert_eq!(read, vec![resume(ID)], "off, and resumed: {remote}");
        };
        for field in ["id", "directory", "place", "repository"] {
            off(&|remote| drop(remote.remove(field)));
            off(&|remote| drop(remote.insert(field.into(), json!(7))));
            off(&|remote| drop(remote.insert(field.into(), json!([1]))));
            off(&|remote| drop(remote.insert(field.into(), json!([1, -2]))));
        }
        off(&|remote| drop(remote.insert("id".into(), json!(""))));
        off(&|remote| drop(remote.insert("id".into(), json!("x".repeat(129)))));
        off(&|remote| drop(remote.insert("directory".into(), json!("x".repeat(4097)))));
        off(&|remote| drop(remote.insert("place".into(), json!(null))));
        // Its id there is in no `Debug`, and so in no log.
        assert!(!format!("{:?}", remote()).contains("cse_"));
    }

    #[test]
    fn the_conversations_turned_off_go_and_come_back_and_what_does_not_read_is_all() {
        let ids = vec![ID.to_owned(), OTHER.to_owned()];
        let mut state = json!({});
        add_turned_off(&mut state, &ids);
        assert_eq!(parse_turned_off(state.as_object().unwrap()), Ok(ids));
        assert_eq!(
            parse_turned_off(json!({}).as_object().unwrap()),
            Ok(Vec::new())
        );
        for odd in [json!(7), json!(["x"]), json!([ID, 7]), json!(null)] {
            let state = json!({ "native_remote_off": odd });
            assert_eq!(
                parse_turned_off(state.as_object().unwrap()),
                Err(()),
                "{odd}"
            );
        }
    }

    #[test]
    fn resumes_go_and_come_back_as_they_were() {
        let resumes = vec![
            NativeResume {
                id: ID.to_owned(),
                mode: Some(PermissionMode::AcceptEdits),
                model: Some("sonnet".into()),
                effort: Some("low".into()),
                remote: None,
            },
            resume(OTHER),
        ];
        assert_eq!(parsed(state(&resumes)), resumes);
    }

    #[test]
    fn a_state_with_no_resumes_resumes_none() {
        assert_eq!(parse(json!({}).as_object().unwrap()), Vec::new());
        assert_eq!(parsed(json!(null)), Vec::new());
        assert_eq!(parsed(json!({ "id": ID })), Vec::new(), "not a list");
    }

    #[test]
    fn an_entry_that_does_not_read_is_left_out_and_the_rest_stay() {
        let good = json!({ "id": OTHER });
        for bad in [
            json!("an id"),
            json!({}),
            json!({ "id": "short" }),
            json!({ "id": 7 }),
            json!({ "id": "é123abcd-0000-4000-8000-00000000001" }),
            json!({ "id": "0123abcd-0000-4000-8000-00000000000\n" }),
            json!({ "id": "0123abcd-0000-4000-8000-00000000000g" }),
            // Modes oso never sends, and what is not a mode.
            json!({ "id": ID, "mode": "bypassPermissions" }),
            json!({ "id": ID, "mode": "dontAsk" }),
            json!({ "id": ID, "mode": 1 }),
            json!({ "id": ID, "mode": "" }),
            json!({ "id": ID, "model": "m".repeat(MAX_MODEL + 1) }),
            json!({ "id": ID, "model": ["sonnet"] }),
            json!({ "id": ID, "effort": "e".repeat(MAX_EFFORT + 1) }),
        ] {
            assert_eq!(
                parsed(json!([bad, good])),
                vec![resume(OTHER)],
                "{bad} is left out"
            );
        }
    }

    #[test]
    fn a_session_resumes_once_and_no_more_than_the_cap_are_read() {
        assert_eq!(
            parsed(json!([{ "id": ID }, { "id": ID, "model": "opus" }])),
            vec![resume(ID)],
            "the first of an id"
        );
        let many: Vec<NativeResume> = (0..=MAX_NATIVE_RESUMES)
            .map(|n| resume(&format!("0123abcd-0000-4000-8000-{n:012}")))
            .collect();
        assert_eq!(parsed(state(&many)).len(), MAX_NATIVE_RESUMES);
        let written = json!(
            many.iter()
                .map(|r| json!({ "id": r.id }))
                .collect::<Vec<_>>()
        );
        assert_eq!(parsed(written), many[..MAX_NATIVE_RESUMES]);
    }

    #[test]
    fn the_native_sessions_to_resume_go_with_the_state_and_an_older_state_names_none() {
        let editor = visiting(&absolute("/p/a.txt"), "text\n");
        let mut exported = export_value(&editor, &[], None, None).unwrap();
        assert_eq!(carry::parse(&exported).unwrap().native_resumes, Vec::new());
        let resume = NativeResume {
            id: "0123abcd-0000-4000-8000-000000000001".into(),
            mode: Some(PermissionMode::Plan),
            model: Some("opus".into()),
            effort: None,
            remote: None,
        };
        add(&mut exported, std::slice::from_ref(&resume));
        assert_eq!(
            carry::parse(&exported).unwrap().native_resumes,
            vec![resume]
        );
        // One too long for the line is left out, and the restart goes on.
        let long = NativeResume {
            id: "0123abcd-0000-4000-8000-000000000002".into(),
            mode: None,
            model: Some("m".repeat(200)),
            effort: None,
            remote: None,
        };
        let mut state = export_value(&editor, &[], None, None).unwrap();
        // Within the line by 100 bytes, which the entry is not.
        let room = carry::MAX_STATE - state.to_string().len() - 100 - r#","filler":"""#.len();
        state["filler"] = json!("x".repeat(room));
        assert!(carry::fits(&state).is_ok());
        add(&mut state, &[long]);
        assert!(state.get("native_resumes").is_none());
        assert!(carry::fits(&state).is_ok());
    }
}
