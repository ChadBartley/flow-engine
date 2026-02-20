//! Token counting for memory management.
//!
//! The [`TokenCounter`] trait provides an abstraction for estimating token
//! counts in messages. The default [`CharEstimateCounter`] uses a simple
//! chars/4 heuristic that requires no external dependencies. For accurate
//! counts, plug in a provider-specific tokenizer (e.g., tiktoken-rs for
//! OpenAI models) via the `EngineBuilder::token_counter()` method.

use serde_json::Value;

/// Estimates token count for text and message arrays.
///
/// Implementations must be thread-safe (`Send + Sync`) since the counter
/// is shared across concurrent node executions via `Arc`.
pub trait TokenCounter: Send + Sync {
    /// Estimate the token count for a text string.
    fn count_tokens(&self, text: &str) -> usize;

    /// Estimate the total token count for an array of chat messages.
    ///
    /// Each message is expected to be a JSON object with at least `role` and
    /// `content` fields. The default implementation sums `count_tokens` over
    /// the serialized `role` and `content` (plus `tool_calls` if present).
    fn count_messages(&self, messages: &[Value]) -> usize {
        messages
            .iter()
            .map(|msg| {
                let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or_default();
                let content = msg
                    .get("content")
                    .map(|v| match v {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    })
                    .unwrap_or_default();

                let mut total = self.count_tokens(role) + self.count_tokens(&content);

                // Include tool_calls if present (serialized to string).
                if let Some(tc) = msg.get("tool_calls") {
                    total += self.count_tokens(&tc.to_string());
                }

                total
            })
            .sum()
    }
}

/// Simple character-based token estimator (chars / 4).
///
/// This provides a rough approximation suitable for budget checks without
/// any external tokenizer dependency. For production use with specific LLM
/// providers, prefer an accurate tokenizer implementation.
pub struct CharEstimateCounter;

impl TokenCounter for CharEstimateCounter {
    fn count_tokens(&self, text: &str) -> usize {
        // Use char count (not byte count) for better Unicode handling.
        text.chars().count().div_ceil(4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn char_estimate_empty_string() {
        let counter = CharEstimateCounter;
        assert_eq!(counter.count_tokens(""), 0);
    }

    #[test]
    fn char_estimate_short_string() {
        let counter = CharEstimateCounter;
        // "hello" = 5 chars → (5+3)/4 = 2
        assert_eq!(counter.count_tokens("hello"), 2);
    }

    #[test]
    fn char_estimate_long_string() {
        let counter = CharEstimateCounter;
        let text = "a".repeat(1000);
        // 1000 chars → (1000+3)/4 = 250
        assert_eq!(counter.count_tokens(&text), 250);
    }

    #[test]
    fn char_estimate_unicode() {
        let counter = CharEstimateCounter;
        // 4 emoji chars (multi-byte), should count chars not bytes
        let text = "🎉🎊🎈🎁";
        assert_eq!(counter.count_tokens(text), (4 + 3) / 4);
    }

    #[test]
    fn count_messages_single() {
        let counter = CharEstimateCounter;
        let msgs = vec![json!({"role": "user", "content": "hello world"})];
        let count = counter.count_messages(&msgs);
        // "user" = 1 token, "hello world" = (11+3)/4 = 3 tokens
        assert_eq!(count, 1 + 3);
    }

    #[test]
    fn count_messages_array() {
        let counter = CharEstimateCounter;
        let msgs = vec![
            json!({"role": "system", "content": "You are helpful."}),
            json!({"role": "user", "content": "Hi"}),
        ];
        let count = counter.count_messages(&msgs);
        assert!(count > 0);
        // system: "system"=(6+3)/4=2, "You are helpful."=(16+3)/4=4 → 6
        // user: "user"=(4+3)/4=1, "Hi"=(2+3)/4=1 → 2
        assert_eq!(count, 6 + 2);
    }

    #[test]
    fn count_messages_with_tool_calls() {
        let counter = CharEstimateCounter;
        let msgs = vec![json!({
            "role": "assistant",
            "content": "ok",
            "tool_calls": [{"id": "1", "name": "search", "arguments": {"q": "test"}}]
        })];
        let count = counter.count_messages(&msgs);
        // Should include tool_calls serialized string tokens
        assert!(count > 2);
    }

    #[test]
    fn count_messages_empty_array() {
        let counter = CharEstimateCounter;
        assert_eq!(counter.count_messages(&[]), 0);
    }
}
