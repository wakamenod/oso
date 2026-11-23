//! What a native session's to-do tools say (ADR 0165, part 3e): a `TaskUpdate` call's input,
//! and what `TaskCreate`, `TaskUpdate` and `TodoWrite` gave back, from the result message's
//! `tool_use_result` (measured, CLI 2.1.330).
//!
//! Must not depend on: `todo`, `rows` or `render`; plain functions over a parsed value.
//!
//! - Only the fields named here are read, each cut: an id or a status to [`super::MAX_NAME`], an
//!   item's text to [`MAX_ITEM`], and at most [`MAX_ITEMS`] of a `TodoWrite`'s items.
//! - Which tool a result is of is not read here: the caller matches it with its call by id.

use serde_json::{Map, Value};

use super::{capped, string};

/// The most items of a list kept.
pub const MAX_ITEMS: usize = 100;
/// The most of an item's text kept, in bytes.
pub const MAX_ITEM: usize = 256;

/// What a `TaskUpdate` call asks to change: the task, and its new subject, if it gives one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Update {
    pub task: String,
    pub subject: Option<String>,
}

/// What a to-do tool gave back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Done {
    /// `TaskCreate`: `{task: {id, subject}}`.
    Created { id: String, subject: String },
    /// `TaskUpdate`: `{success, taskId, updatedFields, statusChange: {from, to}}`; a task
    /// deleted says `deleted` in both.
    Updated {
        id: String,
        /// Its subject was one of the fields changed.
        subject: bool,
        /// The status it went to, if it changed.
        to: Option<String>,
    },
    /// `TodoWrite`: `{oldTodos, newTodos}`, the whole list as it is now.
    Written(Vec<Written>),
}

/// An item of a `TodoWrite`'s list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Written {
    pub content: String,
    pub status: String,
}

/// What a `TaskUpdate` call's `input` asks.
pub fn update(input: Option<&Value>) -> Option<Update> {
    let input = input?.as_object()?;
    let task = string(input, "taskId");
    if task.is_empty() {
        return None;
    }
    Some(Update {
        task,
        subject: input.get("subject").and_then(Value::as_str).map(item),
    })
}

/// What `result`, a message's `tool_use_result`, says a to-do tool did, if it is one of
/// theirs. A `TaskUpdate` that did not succeed says nothing.
pub fn done(result: Option<&Value>) -> Option<Done> {
    let result = result?.as_object()?;
    if let Some(task) = result.get("task").and_then(Value::as_object) {
        let id = string(task, "id");
        let subject = task.get("subject").and_then(Value::as_str)?;
        return (!id.is_empty()).then(|| Done::Created {
            id,
            subject: item(subject),
        });
    }
    if let Some(todos) = result.get("newTodos").and_then(Value::as_array) {
        let written = (todos.iter().filter_map(Value::as_object).take(MAX_ITEMS))
            .map(|todo| Written {
                content: todo
                    .get("content")
                    .and_then(Value::as_str)
                    .map_or_else(String::new, item),
                status: string(todo, "status"),
            })
            .collect();
        return Some(Done::Written(written));
    }
    if result.get("success").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    let id = string(result, "taskId");
    if id.is_empty() {
        return None;
    }
    let fields = field_names(result);
    let to = (result.get("statusChange").and_then(Value::as_object))
        .map(|change| string(change, "to"))
        .filter(|to| !to.is_empty())
        .or_else(|| fields.contains(&"deleted").then(|| "deleted".to_owned()));
    Some(Done::Updated {
        id,
        subject: fields.contains(&"subject"),
        to,
    })
}

/// The names in `updatedFields`, at most a few.
fn field_names(result: &Map<String, Value>) -> Vec<&str> {
    (result.get("updatedFields").and_then(Value::as_array))
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .take(16)
        .collect()
}

/// An item's text, cut to [`MAX_ITEM`].
fn item(text: &str) -> String {
    capped(text, MAX_ITEM).0.to_owned()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_created_task_gives_its_id_and_subject() {
        let result = json!({"task": {"id": "1", "subject": "Alpha", "description": "x"}});
        assert_eq!(
            done(Some(&result)),
            Some(Done::Created {
                id: "1".into(),
                subject: "Alpha".into()
            })
        );
    }

    #[test]
    fn an_update_gives_its_status_and_whether_the_subject_changed() {
        let status = json!({
            "success": true,
            "taskId": "1",
            "updatedFields": ["status"],
            "statusChange": {"from": "pending", "to": "in_progress"}
        });
        assert_eq!(
            done(Some(&status)),
            Some(Done::Updated {
                id: "1".into(),
                subject: false,
                to: Some("in_progress".into())
            })
        );
        let renamed = json!({"success": true, "taskId": "2", "updatedFields": ["subject"]});
        assert_eq!(
            done(Some(&renamed)),
            Some(Done::Updated {
                id: "2".into(),
                subject: true,
                to: None
            })
        );
        let deleted = json!({"success": true, "taskId": "3", "updatedFields": ["deleted"]});
        assert!(matches!(
            done(Some(&deleted)),
            Some(Done::Updated { to: Some(to), .. }) if to == "deleted"
        ));
    }

    #[test]
    fn a_failed_update_or_another_tools_result_says_nothing() {
        let failed = json!({"success": false, "taskId": "1", "updatedFields": ["status"]});
        assert_eq!(done(Some(&failed)), None);
        let bash = json!({"stdout": "hi", "stderr": "", "interrupted": false});
        assert_eq!(done(Some(&bash)), None);
        assert_eq!(done(Some(&json!("Error: no"))), None);
        assert_eq!(done(None), None);
    }

    #[test]
    fn a_written_list_is_cut_to_its_limits() {
        let long = "x".repeat(MAX_ITEM * 2);
        let todos: Vec<Value> = (0..MAX_ITEMS + 5)
            .map(|_| json!({"content": long, "status": "pending", "activeForm": "y"}))
            .collect();
        let Some(Done::Written(written)) = done(Some(&json!({"oldTodos": [], "newTodos": todos})))
        else {
            panic!("not a list");
        };
        assert_eq!(written.len(), MAX_ITEMS);
        assert_eq!(written[0].content.len(), MAX_ITEM);
        assert_eq!(written[0].status, "pending");
    }

    #[test]
    fn an_update_call_names_its_task_and_subject() {
        let input = json!({"taskId": "2", "subject": "Beta2"});
        assert_eq!(
            update(Some(&input)),
            Some(Update {
                task: "2".into(),
                subject: Some("Beta2".into())
            })
        );
        assert_eq!(update(Some(&json!({"status": "completed"}))), None);
    }
}
