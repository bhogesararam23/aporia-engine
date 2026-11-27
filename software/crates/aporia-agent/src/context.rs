//! Keeping the conversation bounded without losing the record.
//!
//! A long investigation accumulates tool outputs faster than any model's context. The host
//! elides the *oldest* tool outputs from the conversation when their total passes a limit; the
//! full text stays in the journal, and the elision says so. The rule is a pure function of the
//! message list, so a resume that rebuilds the full conversation and prunes again arrives at
//! exactly the same transcript -- nothing about pruning needs its own journal event.
//!
//! System, objective, user and assistant messages are never elided: they carry the task, the
//! decisions and the call ids. Only tool results shrink, oldest first, and each keeps its head
//! so a citation's context survives.

use crate::provider::{Message, Role};

/// The conversation's total content budget, in bytes.
pub const CONTEXT_LIMIT: usize = 256 * 1024;
/// What an elided tool output keeps: its head, and the notice.
const KEPT_HEAD: usize = 1500;
/// The notice appended to an elided output.
pub const ELISION_NOTICE: &str =
    "\n[older output elided from the conversation; the full text is in the journal]";

/// Elide the oldest tool outputs until the conversation fits `limit`. Returns how many were
/// elided. Idempotent: an already-elided message is not elided again.
pub fn prune(messages: &mut [Message], limit: usize) -> usize {
    let total: usize = messages.iter().map(|m| m.content.len()).sum();
    if total <= limit {
        return 0;
    }
    let mut excess = total - limit;
    let mut elided = 0;
    for message in messages.iter_mut() {
        if excess == 0 {
            break;
        }
        if message.role != Role::Tool || message.content.ends_with(ELISION_NOTICE) {
            continue;
        }
        if message.content.len() <= KEPT_HEAD + ELISION_NOTICE.len() {
            continue;
        }
        let mut end = KEPT_HEAD.min(message.content.len());
        while !message.content.is_char_boundary(end) {
            end -= 1;
        }
        let mut kept = message.content[..end].to_string();
        kept.push_str(ELISION_NOTICE);
        excess = excess.saturating_sub(message.content.len() - kept.len());
        message.content = kept;
        elided += 1;
    }
    elided
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(id: &str, bytes: usize) -> Message {
        Message::tool(id, "x".repeat(bytes))
    }

    #[test]
    fn a_small_conversation_is_untouched() {
        let mut messages = vec![
            Message::system("rules"),
            Message::user("objective"),
            tool("c1", 1000),
        ];
        assert_eq!(prune(&mut messages, CONTEXT_LIMIT), 0);
        assert_eq!(messages[2].content.len(), 1000);
    }

    #[test]
    fn the_oldest_tool_outputs_shrink_first_and_say_so() {
        let mut messages = vec![
            Message::system("rules"),
            Message::user("objective"),
            tool("c1", 100_000),
            tool("c2", 100_000),
            tool("c3", 100_000),
        ];
        let elided = prune(&mut messages, 250_000);
        assert!(elided >= 1);
        assert!(messages[2].content.ends_with(ELISION_NOTICE));
        assert!(messages[2].content.len() < 100_000);
        // The newest output is intact while the total fits.
        let total: usize = messages.iter().map(|m| m.content.len()).sum();
        assert!(total <= 250_000 + KEPT_HEAD);
        assert_eq!(messages[0].content, "rules");
        assert_eq!(messages[1].content, "objective");
    }

    #[test]
    fn pruning_is_idempotent_and_never_touches_non_tool_messages() {
        let mut messages = vec![
            Message::system("s".repeat(10_000)),
            Message::user("u".repeat(10_000)),
            Message::assistant("a".repeat(10_000), Vec::new()),
            tool("c1", 300_000),
        ];
        let first = prune(&mut messages, 100_000);
        assert_eq!(first, 1);
        let snapshot = messages.clone();
        assert_eq!(prune(&mut messages, 100_000), 0);
        assert_eq!(messages, snapshot);
        assert_eq!(messages[0].content.len(), 10_000);
        assert_eq!(messages[1].content.len(), 10_000);
        assert_eq!(messages[2].content.len(), 10_000);
    }
}
