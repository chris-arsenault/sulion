use serde_json::Value;
use uuid::Uuid;

use super::ingester::TranscriptSource;
use crate::activity::ActivityState;
use crate::db::Pool;

pub(super) async fn project_from_event_best_effort(
    pool: &Pool,
    session_uuid: Uuid,
    source: TranscriptSource,
    value: &Value,
    byte_offset: i64,
) {
    if let Err(err) = project_from_event(pool, session_uuid, source, value).await {
        tracing::warn!(
            %err,
            session = %session_uuid,
            agent = source.agent_id(),
            byte_offset,
            "agent activity projection failed",
        );
    }
}

async fn project_from_event(
    pool: &Pool,
    session_uuid: Uuid,
    source: TranscriptSource,
    value: &Value,
) -> anyhow::Result<()> {
    let transition = match source {
        TranscriptSource::ClaudeCode => claude_transition(value),
        TranscriptSource::Codex => codex_transition(value),
    };
    let Some((state, summary, confidence)) = transition else {
        return Ok(());
    };
    crate::activity::set_for_current_agent_session(
        pool,
        session_uuid,
        state,
        summary.as_deref(),
        None,
        "ingester",
        confidence,
    )
    .await?;
    Ok(())
}

fn claude_transition(value: &Value) -> Option<(ActivityState, Option<String>, &'static str)> {
    match value.get("type").and_then(Value::as_str) {
        // Any user record — a prompt or a tool result — means the agent
        // is (back to) working. This also clears a needs_input once the
        // interactive tool's result lands.
        Some("user") => Some((ActivityState::Working, first_message_text(value), "derived")),
        // Interactive tools present a selection screen in the terminal
        // and block until the user acts there.
        Some("assistant") => {
            let interactive = interactive_claude_tool(value)?;
            Some((ActivityState::NeedsInput, Some(interactive), "derived"))
        }
        _ => None,
    }
}

/// Summary for an interactive tool call in an assistant record, if one
/// is present: Claude's plan-approval and question dialogs both block on
/// terminal input.
fn interactive_claude_tool(value: &Value) -> Option<String> {
    let content = value.get("message")?.get("content")?.as_array()?;
    content.iter().find_map(|item| {
        if item.get("type").and_then(Value::as_str) != Some("tool_use") {
            return None;
        }
        match item.get("name").and_then(Value::as_str) {
            Some("ExitPlanMode" | "exitPlanMode") => {
                Some("Plan ready for review in the terminal".to_string())
            }
            Some("AskUserQuestion") => Some(
                item.get("input")
                    .and_then(|input| input.get("questions"))
                    .and_then(Value::as_array)
                    .and_then(|questions| questions.first())
                    .and_then(|question| question.get("question"))
                    .and_then(Value::as_str)
                    .map(|question| format!("Question in the terminal: {question}"))
                    .unwrap_or_else(|| "Question awaiting an answer in the terminal".to_string()),
            ),
            _ => None,
        }
    })
}

fn codex_transition(value: &Value) -> Option<(ActivityState, Option<String>, &'static str)> {
    let outer = super::canonical::codex_record_kind(value).unwrap_or("");
    let payload = value.get("payload");
    let subtype = payload
        .and_then(|payload| payload.get("type"))
        .and_then(Value::as_str)
        .unwrap_or("");
    match (outer, subtype) {
        ("event_msg", "task_started" | "turn_started") => {
            Some((ActivityState::Working, None, "explicit"))
        }
        ("event_msg", "task_complete" | "turn_complete") => {
            Some((ActivityState::AwaitingPrompt, None, "explicit"))
        }
        // A user-input request blocks on a terminal selection. In
        // code-mode it hides inside an exec snippet, so match on the
        // call's source text as well as the direct tool name.
        ("response_item", "custom_tool_call" | "function_call") => {
            let payload = payload?;
            let name = payload.get("name").and_then(Value::as_str).unwrap_or("");
            let requests_input = name == "request_user_input"
                || (name == "exec"
                    && payload
                        .get("input")
                        .and_then(Value::as_str)
                        .is_some_and(|code| code.contains("request_user_input")));
            if requests_input {
                Some((
                    ActivityState::NeedsInput,
                    Some("Input requested in the terminal".to_string()),
                    "derived",
                ))
            } else {
                None
            }
        }
        // A tool output only exists once its call finished — for an
        // input request, that is the user having answered.
        ("response_item", "custom_tool_call_output" | "function_call_output") => {
            Some((ActivityState::Working, None, "derived"))
        }
        _ => None,
    }
}

fn first_message_text(value: &Value) -> Option<String> {
    let content = value.get("message")?.get("content")?;
    match content {
        Value::String(text) => non_empty_prefix(text),
        Value::Array(items) => items.iter().find_map(|item| {
            if item.get("type").and_then(Value::as_str) != Some("text") {
                return None;
            }
            item.get("text")
                .and_then(Value::as_str)
                .and_then(non_empty_prefix)
        }),
        _ => None,
    }
}

fn non_empty_prefix(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.chars().take(240).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn claude_plan_approval_needs_input() {
        let value = json!({
            "type": "assistant",
            "message": { "role": "assistant", "content": [
                { "type": "tool_use", "id": "t1", "name": "ExitPlanMode", "input": { "plan": "…" } }
            ]}
        });
        let (state, summary, confidence) = claude_transition(&value).expect("transition");
        assert_eq!(state, ActivityState::NeedsInput);
        assert!(summary.unwrap().contains("Plan ready"));
        assert_eq!(confidence, "derived");
    }

    #[test]
    fn claude_question_needs_input_with_question_text() {
        let value = json!({
            "type": "assistant",
            "message": { "role": "assistant", "content": [
                { "type": "tool_use", "id": "t1", "name": "AskUserQuestion",
                  "input": { "questions": [{ "question": "Which auth method?" }] } }
            ]}
        });
        let (state, summary, _) = claude_transition(&value).expect("transition");
        assert_eq!(state, ActivityState::NeedsInput);
        assert!(summary.unwrap().contains("Which auth method?"));
    }

    #[test]
    fn claude_ordinary_tool_use_is_not_needs_input() {
        let value = json!({
            "type": "assistant",
            "message": { "role": "assistant", "content": [
                { "type": "tool_use", "id": "t1", "name": "Bash", "input": { "command": "ls" } }
            ]}
        });
        assert!(claude_transition(&value).is_none());
    }

    #[test]
    fn claude_user_record_resolves_to_working() {
        let value = json!({
            "type": "user",
            "message": { "role": "user", "content": "approved, go ahead" }
        });
        let (state, _, _) = claude_transition(&value).expect("transition");
        assert_eq!(state, ActivityState::Working);
    }

    #[test]
    fn codex_code_mode_input_request_needs_input_and_output_resolves() {
        let call = json!({
            "type": "response_item",
            "payload": {
                "type": "custom_tool_call",
                "name": "exec",
                "call_id": "c1",
                "input": "const a = await tools.request_user_input({\"questions\":[]});",
            }
        });
        let (state, _, _) = codex_transition(&call).expect("transition");
        assert_eq!(state, ActivityState::NeedsInput);

        let output = json!({
            "type": "response_item",
            "payload": { "type": "custom_tool_call_output", "call_id": "c1", "output": "{}" }
        });
        let (state, _, _) = codex_transition(&output).expect("transition");
        assert_eq!(state, ActivityState::Working);
    }

    #[test]
    fn codex_plain_exec_is_not_needs_input() {
        let call = json!({
            "type": "response_item",
            "payload": {
                "type": "custom_tool_call",
                "name": "exec",
                "call_id": "c1",
                "input": "const r = await tools.exec_command({\"cmd\":\"ls\"});",
            }
        });
        assert!(codex_transition(&call).is_none());
    }
}
