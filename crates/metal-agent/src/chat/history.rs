//! History pruning by whole exchanges, from ZeroClaw's `history_pruner` /
//! `pruned_tool_exchange_summary`. The rule it fixes (problem 8): a `tool`
//! row may never ride the wire without the assistant `tool_calls` row that
//! names it, so trimming has to drop or keep a whole exchange, never a slice
//! of messages.

use super::message::ChatMessage;

/// How many trailing exchanges stay whole. Older ones collapse to a summary.
pub const HISTORY_MAX_EXCHANGES: usize = 8;

/// One line standing in for a dropped tool result.
pub fn summarize_tool_result(content: &str) -> String {
    let first = content.lines().next().unwrap_or("").trim();
    let clipped = if first.chars().count() > 120 {
        let head: String = first.chars().take(120).collect();
        format!("{head}…")
    } else {
        first.to_string()
    };
    if clipped.is_empty() {
        "(empty result)".to_string()
    } else {
        clipped
    }
}

/// Split into (leading system rows, exchanges). An exchange starts at a
/// real `user` turn (one carrying no lifted images) and runs until the next
/// one — so an assistant `tool_calls` row and the `tool` rows answering it
/// always share a group.
fn group(messages: &[ChatMessage]) -> (Vec<ChatMessage>, Vec<Vec<ChatMessage>>) {
    let mut system = Vec::new();
    let mut groups: Vec<Vec<ChatMessage>> = Vec::new();
    for m in messages {
        if m.role == "system" && groups.is_empty() {
            system.push(m.clone());
            continue;
        }
        let starts_exchange = m.role == "user" && m.images.is_empty();
        if starts_exchange || groups.is_empty() {
            groups.push(Vec::new());
        }
        groups.last_mut().unwrap().push(m.clone());
    }
    (system, groups)
}

/// Keep the leading system rows and the last `max_exchanges` exchanges,
/// collapsing earlier tool results into one summary row. Mutates in place.
pub fn prune_exchanges(messages: &mut Vec<ChatMessage>, max_exchanges: usize) {
    let (system, mut groups) = group(messages);
    if groups.len() <= max_exchanges || max_exchanges == 0 {
        return;
    }
    let drop_count = groups.len() - max_exchanges;
    let dropped: Vec<Vec<ChatMessage>> = groups.drain(..drop_count).collect();

    let mut summaries: Vec<String> = Vec::new();
    for g in &dropped {
        for m in g {
            if m.role == "tool" {
                if let Some(call) = g
                    .iter()
                    .flat_map(|a| a.tool_calls.iter())
                    .find(|c| m.tool_call_id.as_deref() == Some(c.id.as_str()))
                {
                    summaries.push(format!("{} → {}", call.name, summarize_tool_result(&m.content)));
                }
            }
        }
    }

    let mut out = system;
    if !summaries.is_empty() {
        out.push(ChatMessage::notice(
            "summary",
            format!(
                "{drop_count} earlier exchanges omitted; results summarized: {}",
                summaries.join("; ")
            ),
        ));
    }
    for g in groups {
        out.extend(g);
    }
    *messages = out;
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::message::ToolCallState;
    use serde_json::json;

    fn user(text: &str) -> ChatMessage {
        ChatMessage::new("user", text)
    }

    fn assistant_with_call(id: &str, name: &str) -> ChatMessage {
        let mut m = ChatMessage::new("assistant", "");
        m.tool_calls.push(ToolCallState::new(id, name, json!({})));
        m
    }

    fn tool(id: &str, content: &str) -> ChatMessage {
        let mut m = ChatMessage::new("tool", content);
        m.tool_call_id = Some(id.into());
        m
    }

    #[test]
    fn a_short_history_is_untouched() {
        let mut messages = vec![user("a"), assistant_with_call("c1", "x"), tool("c1", "big")];
        let before = messages.len();
        prune_exchanges(&mut messages, 8);
        assert_eq!(messages.len(), before);
    }

    #[test]
    fn no_tool_row_survives_without_its_assistant_call() {
        let mut messages = vec![ChatMessage::new("system", "sys")];
        for i in 0..6 {
            messages.push(user(&format!("u{i}")));
            messages.push(assistant_with_call(&format!("c{i}"), "list_nodes"));
            messages.push(tool(&format!("c{i}"), &format!("result {i}")));
        }
        prune_exchanges(&mut messages, 2);

        // Every tool row still has a naming assistant row before it.
        let ids: Vec<String> = messages
            .iter()
            .flat_map(|m| m.tool_calls.iter().map(|c| c.id.clone()))
            .collect();
        for m in &messages {
            if let Some(id) = &m.tool_call_id {
                assert!(ids.contains(id), "orphan tool row {id}: {messages:#?}");
            }
        }
        // The system row is preserved.
        assert_eq!(messages[0].role, "system");
    }

    #[test]
    fn dropped_results_collapse_to_a_summary() {
        let mut messages = vec![];
        for i in 0..4 {
            messages.push(user(&format!("u{i}")));
            messages.push(assistant_with_call(&format!("c{i}"), "list_nodes"));
            messages.push(tool(&format!("c{i}"), "line one\nline two\nline three"));
        }
        prune_exchanges(&mut messages, 1);
        assert!(messages.iter().any(|m| m.kind.as_deref() == Some("summary")));
        let summary = messages.iter().find(|m| m.kind.as_deref() == Some("summary")).unwrap();
        assert!(summary.content.contains("list_nodes → line one"));
    }

    #[test]
    fn summarize_clips_a_long_first_line() {
        let long = "x".repeat(300);
        let s = summarize_tool_result(&long);
        assert!(s.ends_with('…'));
        assert!(s.chars().count() <= 121);
    }
}
