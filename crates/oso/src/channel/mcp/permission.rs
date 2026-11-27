//! The bridge's `permission` tool (ADR 0165, step 2): a native session's `claude` names it with
//! `--permission-prompt-tool`, and the bridge asks the person through the sessions' socket,
//! which oso listens on with the agent channel off or on. Only `oso mcp --permission`, as a
//! native session's MCP configuration starts it, lists the tool.
//!
//! Must not depend on: the agent channel's socket; it asks only on the sessions'.
//!
//! - The CLI calls it with `{tool_name, input, tool_use_id}`, all of which go to oso, the input
//!   only when it is at most [`permission::MAX_INPUT`] bytes as JSON. oso asks the person about
//!   the call as the session's own stream has it, if the input says the same; an `allow` gives
//!   the CLI back the input it sent, unchanged, as `updatedInput`, but for an
//!   `AskUserQuestion`'s, which takes the person's `answers` (step 2e-2), and an
//!   `ExitPlanMode`'s, which takes the person's edit of the plan, with the session's mode after
//!   it as a `setMode` in `updatedPermissions` (part 3d). Nothing else ever sets a mode.
//! - Whatever goes wrong, the CLI is told `deny` with why: a call never runs unless the person
//!   said `y`, or the space's orchestrator the person let answer that tool allowed it, once,
//!   with `session_answer` (ADR 0186).

use serde_json::{Map, Value, json};

use super::super::protocol::permission::{self, Call};
use super::tools::{tool, tool_result};
use super::{Bridge, Failed, Which};

/// The tool's name: the CLI calls it `mcp__oso__permission`.
pub(crate) const TOOL: &str = "permission";

/// What a bridge is told when no oso it may ask hosts its session.
pub(super) const NOT_HOSTED: &str =
    "the oso instance chosen with --pid hosts no native session to ask the person for";

/// What the CLI is told when the person could not be asked.
const NOT_ASKED: &str = "denied: oso could not ask the person";

/// The tool as `tools/list` gives it.
pub(super) fn listed() -> Value {
    tool(
        TOOL,
        "Asks the person in oso whether a call may run. Only Claude Code calls it, as a native session's --permission-prompt-tool; never call it yourself.",
        json!({
            "tool_name": { "type": "string" },
            "input": { "type": "object" },
            "tool_use_id": { "type": "string" },
        }),
        &["tool_name", "input", "tool_use_id"],
        true,
        false,
    )
}

impl Bridge {
    /// The `permission` call `id` with `arguments`, answered with the person's decision: the
    /// result the CLI reads, or `None` if the agent cancelled the call.
    pub(super) fn ask_permission(
        &mut self,
        id: &Value,
        arguments: &Map<String, Value>,
    ) -> Option<Value> {
        let answer = match self.asked(id, arguments) {
            Ok(answer) => answer,
            Err(Failed::Cancelled) => return None,
            Err(failed) => permission::deny(&format!("{NOT_ASKED}: {}", failed.message())),
        };
        Some(tool_result(&answer.to_string(), false))
    }

    fn asked(&mut self, id: &Value, arguments: &Map<String, Value>) -> Result<Value, Failed> {
        let mut named = Map::new();
        for key in ["tool_name", "tool_use_id"] {
            if let Some(value) = arguments.get(key) {
                named.insert(key.to_owned(), value.clone());
            }
        }
        let call = permission::parse(&named).map_err(Failed::Refused)?;
        let input = match arguments.get("input") {
            Some(Value::Object(input)) => Value::Object(input.clone()),
            _ => return Err(Failed::Refused("the call has no input".into())),
        };
        let Call {
            tool_name,
            tool_use_id,
            ..
        } = call;
        let tool_name_kept = tool_name.clone();
        // A larger input is not sent: oso then says it does not show it.
        let mut params = json!({ "tool_name": tool_name, "tool_use_id": tool_use_id });
        if permission::within(&input, permission::MAX_INPUT) {
            params["input"] = input.clone();
        }
        let answer = self.call(id, Which::SessionSocket, "permission", params)?;
        Ok(match answer.get("behavior").and_then(Value::as_str) {
            Some("allow") if tool_name_kept == "ExitPlanMode" => planned(input, &answer),
            Some("allow") => {
                json!({ "behavior": "allow", "updatedInput": answered(input, &tool_name_kept, &answer) })
            }
            Some("deny") => {
                permission::deny(answer.get("message").and_then(Value::as_str).unwrap_or(""))
            }
            _ => permission::deny(&format!("{NOT_ASKED}: oso's answer was unreadable")),
        })
    }
}

/// The input an `allow` gives back: the call's own, with the person's answers put in an
/// `AskUserQuestion`'s (step 2e-2). Answers for any other tool change nothing.
fn answered(mut input: Value, tool_name: &str, answer: &Value) -> Value {
    if tool_name == "AskUserQuestion"
        && let Some(object) = input.as_object_mut()
    {
        // Only the person's: answers the model put in its own input never pass.
        match permission::answers(answer) {
            Some(answers) => object.insert("answers".to_owned(), Value::Object(answers)),
            None => object.remove("answers"),
        };
    }
    input
}

/// An `ExitPlanMode`'s `allow` (part 3d): the call's input with the person's edit of the plan
/// in place of Claude's, and the session's mode after it as a `setMode`, as the CLI takes them
/// (measured, CLI 2.1.330). An answer that carries either unreadably denies.
fn planned(mut input: Value, answer: &Value) -> Value {
    let Ok((plan, mode)) = permission::plan_answer(answer) else {
        return permission::deny(&format!("{NOT_ASKED}: oso's answer was unreadable"));
    };
    if let (Some(plan), Some(object)) = (plan, input.as_object_mut()) {
        object.insert("plan".to_owned(), Value::String(plan));
    }
    let mut allowed = json!({ "behavior": "allow", "updatedInput": input });
    if let Some(mode) = mode {
        allowed["updatedPermissions"] = json!([{
            "type": "setMode",
            "mode": mode.name(),
            "destination": "session",
        }]);
    }
    allowed
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use serde_json::{Value, json};

    use super::super::{Bridge, Connect, Failed, Link};
    use crate::channel::protocol::PROTOCOL_VERSION;

    /// What a stand-in socket was asked, and what it answers `permission` with.
    #[derive(Default)]
    struct Asked {
        calls: Vec<(String, Value)>,
        answer: Option<Result<Value, String>>,
    }

    struct StandIn(Rc<RefCell<Asked>>);

    impl Link for StandIn {
        fn call(&mut self, method: &str, params: Value) -> Result<Value, Failed> {
            let mut asked = self.0.borrow_mut();
            if method == "version" {
                return Ok(json!({ "protocol": PROTOCOL_VERSION }));
            }
            asked.calls.push((method.to_owned(), params));
            match asked.answer.clone() {
                Some(Ok(answer)) => Ok(answer),
                Some(Err(why)) => Err(Failed::Refused(why)),
                None => Err(Failed::Broken("oso closed the connection".into())),
            }
        }
    }

    fn connect(asked: &Rc<RefCell<Asked>>) -> Connect {
        let asked = Rc::clone(asked);
        Box::new(move || Ok(Box::new(StandIn(Rc::clone(&asked))) as Box<dyn Link>))
    }

    /// A bridge whose channel and sessions' socket are stand-ins, `--permission` if `asks`.
    fn bridge(channel: &Rc<RefCell<Asked>>, socket: &Rc<RefCell<Asked>>, asks: bool) -> Bridge {
        let bridge = Bridge::new(connect(channel)).for_session(Some("s-1".into()));
        if asks {
            bridge.with_permission(connect(socket))
        } else {
            bridge
        }
    }

    fn respond(bridge: &mut Bridge, request: &Value) -> Value {
        serde_json::from_str(&bridge.respond(&request.to_string()).unwrap()).unwrap()
    }

    /// What the CLI reads of a `permission` call with `arguments`.
    fn asked(bridge: &mut Bridge, arguments: &Value) -> Value {
        let request = json!({
            "jsonrpc": "2.0", "id": 9, "method": "tools/call",
            "params": { "name": "permission", "arguments": arguments },
        });
        let response = respond(bridge, &request);
        assert_eq!(response["result"]["isError"], false, "{response}");
        let text = response["result"]["content"][0]["text"].as_str().unwrap();
        serde_json::from_str(text).unwrap()
    }

    fn bash() -> Value {
        json!({
            "tool_name": "Bash",
            "input": { "command": "git push", "description": "Push it" },
            "tool_use_id": "toolu_1",
        })
    }

    fn listed(bridge: &mut Bridge) -> Vec<String> {
        let list = respond(
            bridge,
            &json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
        );
        let tools = list["result"]["tools"].as_array().unwrap();
        tools
            .iter()
            .map(|tool| tool["name"].as_str().unwrap().to_owned())
            .collect()
    }

    #[test]
    fn only_a_native_sessions_bridge_lists_the_tool() {
        let (channel, socket) = (Rc::default(), Rc::default());
        assert!(!listed(&mut bridge(&channel, &socket, false)).contains(&"permission".into()));
        assert!(listed(&mut bridge(&channel, &socket, true)).contains(&"permission".into()));
        // Without `--permission`, it is no tool at all, and the sessions' socket is not asked.
        let mut terminal = bridge(&channel, &socket, false);
        let request = json!({
            "jsonrpc": "2.0", "id": 9, "method": "tools/call",
            "params": { "name": "permission", "arguments": bash() },
        });
        assert!(respond(&mut terminal, &request)["error"].is_object());
        assert!(socket.borrow().calls.is_empty());
    }

    #[test]
    fn an_allow_gives_the_cli_its_own_input_back() {
        let (channel, socket) = (
            Rc::<RefCell<Asked>>::default(),
            Rc::<RefCell<Asked>>::default(),
        );
        socket.borrow_mut().answer = Some(Ok(json!({ "behavior": "allow" })));
        let mut bridge = bridge(&channel, &socket, true);
        let answer = asked(&mut bridge, &bash());
        assert_eq!(
            answer,
            json!({ "behavior": "allow", "updatedInput": bash()["input"] })
        );
        let calls = &socket.borrow().calls;
        assert_eq!(
            calls.as_slice(),
            [(
                "permission".to_owned(),
                json!({ "tool_name": "Bash", "tool_use_id": "toolu_1", "input": bash()["input"] })
            )]
        );
        assert!(
            channel.borrow().calls.is_empty(),
            "the channel is never asked"
        );
    }

    #[test]
    fn an_allow_with_answers_puts_them_in_a_questions_input_only() {
        let (channel, socket) = (
            Rc::<RefCell<Asked>>::default(),
            Rc::<RefCell<Asked>>::default(),
        );
        let answering = json!({ "behavior": "allow", "answers": { "Which size?": "Small" } });
        socket.borrow_mut().answer = Some(Ok(answering.clone()));
        let mut bridge = bridge(&channel, &socket, true);
        let question = json!({
            "tool_name": "AskUserQuestion",
            "tool_use_id": "toolu_2",
            "input": { "questions": [{ "question": "Which size?" }] },
        });
        let answer = asked(&mut bridge, &question);
        assert_eq!(
            answer["updatedInput"],
            json!({
                "questions": [{ "question": "Which size?" }],
                "answers": { "Which size?": "Small" },
            })
        );
        // Another tool's input stays as it was, and answers that are not text are none.
        socket.borrow_mut().answer = Some(Ok(answering));
        assert_eq!(asked(&mut bridge, &bash())["updatedInput"], bash()["input"]);
        socket.borrow_mut().answer = Some(Ok(
            json!({ "behavior": "allow", "answers": { "Which size?": 1 } }),
        ));
        // The model's own answers in its input never pass for the person's.
        socket.borrow_mut().answer = Some(Ok(json!({ "behavior": "allow" })));
        let mut smuggled = question.clone();
        smuggled["tool_use_id"] = json!("toolu_4");
        smuggled["input"]["answers"] = json!({ "Which size?": "Yes, delete it" });
        assert_eq!(
            asked(&mut bridge, &smuggled)["updatedInput"],
            json!({ "questions": [{ "question": "Which size?" }] })
        );
        socket.borrow_mut().answer = Some(Ok(
            json!({ "behavior": "allow", "answers": { "Which size?": 1 } }),
        ));
        let mut again = question;
        again["tool_use_id"] = json!("toolu_3");
        assert_eq!(
            asked(&mut bridge, &again)["updatedInput"],
            json!({ "questions": [{ "question": "Which size?" }] })
        );
    }

    #[test]
    fn a_plans_allow_takes_the_persons_edit_and_mode() {
        let (channel, socket) = (
            Rc::<RefCell<Asked>>::default(),
            Rc::<RefCell<Asked>>::default(),
        );
        let mut bridge = bridge(&channel, &socket, true);
        let plan = |id: &str| {
            json!({
                "tool_name": "ExitPlanMode",
                "tool_use_id": id,
                "input": { "plan": "Write hello.txt", "planFilePath": "/p/plan.md" },
            })
        };
        // Approved as it is: the input goes back unchanged, and no mode is set.
        socket.borrow_mut().answer = Some(Ok(json!({ "behavior": "allow" })));
        assert_eq!(
            asked(&mut bridge, &plan("p1")),
            json!({ "behavior": "allow", "updatedInput": plan("p1")["input"] })
        );
        // Edited, with accept edits after it.
        socket.borrow_mut().answer = Some(Ok(
            json!({ "behavior": "allow", "plan": "Write pelican.txt", "mode": "acceptEdits" }),
        ));
        assert_eq!(
            asked(&mut bridge, &plan("p2")),
            json!({
                "behavior": "allow",
                "updatedInput": { "plan": "Write pelican.txt", "planFilePath": "/p/plan.md" },
                "updatedPermissions": [
                    { "type": "setMode", "mode": "acceptEdits", "destination": "session" },
                ],
            })
        );
        // A mode oso never sends denies; and another tool's input takes neither.
        socket.borrow_mut().answer = Some(Ok(
            json!({ "behavior": "allow", "mode": "bypassPermissions" }),
        ));
        assert_eq!(asked(&mut bridge, &plan("p3"))["behavior"], "deny");
        socket.borrow_mut().answer = Some(Ok(
            json!({ "behavior": "allow", "plan": "rm -rf ~", "mode": "auto" }),
        ));
        assert_eq!(
            asked(&mut bridge, &bash()),
            json!({ "behavior": "allow", "updatedInput": bash()["input"] })
        );
    }

    #[test]
    fn whatever_goes_wrong_denies() {
        let cases: [(Option<Result<Value, String>>, Value); 5] = [
            (
                Some(Ok(json!({ "behavior": "deny", "message": "no" }))),
                bash(),
            ),
            (Some(Ok(json!({ "behavior": "maybe" }))), bash()),
            (
                Some(Err("this socket takes only the bridges".into())),
                bash(),
            ),
            (None, bash()),
            (
                Some(Ok(json!({ "behavior": "allow" }))),
                json!({ "tool_name": "Bash", "input": { "command": "ls" } }),
            ),
        ];
        for (answer, arguments) in cases {
            let (channel, socket) = (Rc::default(), Rc::new(RefCell::new(Asked::default())));
            socket.borrow_mut().answer = answer.clone();
            let mut bridge = bridge(&channel, &socket, true);
            let said = asked(&mut bridge, &arguments);
            assert_eq!(said["behavior"], "deny", "{answer:?} {arguments}: {said}");
            assert!(said["message"].is_string());
        }
    }
}
