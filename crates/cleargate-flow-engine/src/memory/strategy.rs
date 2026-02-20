//! Memory strategies for conversation history windowing.
//!
//! Each variant of [`MemoryStrategy`] defines how conversation history is
//! trimmed before sending to an LLM. Strategies preserve the system message
//! (if present at index 0) and prioritize keeping the most recent messages.
//!
//! # Example JSON configs
//!
//! ```json
//! // Keep last 20 messages
//! { "strategy": "message_count", "max_messages": 20 }
//!
//! // Stay within 8000 tokens
//! { "strategy": "token_budget", "max_tokens": 8000, "reserve_system": true }
//!
//! // Summarize when history exceeds 30 messages
//! {
//!   "strategy": "summarize",
//!   "threshold": 30,
//!   "keep_recent": 10,
//!   "summary_provider": "openai",
//!   "summary_model": "gpt-4o-mini"
//! }
//! ```

use serde::{Deserialize, Serialize};

/// How to trim conversation history before sending to an LLM.
///
/// All strategies preserve the system message (if present at index 0).
/// Trimming always removes the oldest non-system messages first.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "strategy", rename_all = "snake_case")]
pub enum MemoryStrategy {
    /// Keep last N messages (always preserves system message).
    ///
    /// When the conversation has more messages than `max_messages`,
    /// the oldest non-system messages are dropped.
    MessageCount {
        /// Maximum number of non-system messages to keep.
        max_messages: usize,
    },

    /// Keep messages fitting within a token budget (newest first).
    ///
    /// Iterates from the newest message backward, accumulating token
    /// counts. Messages that would exceed the budget are dropped.
    TokenBudget {
        /// Maximum tokens for the message array.
        max_tokens: usize,
        /// When true, the system message's tokens are not counted against
        /// the budget. Default: true.
        #[serde(default = "default_true")]
        reserve_system: bool,
    },

    /// Summarize older messages when count exceeds threshold.
    ///
    /// When message count exceeds `threshold`, messages older than
    /// `keep_recent` (excluding system) are sent to an LLM for
    /// summarization. The summary replaces those messages.
    Summarize {
        /// Trigger summarization when message count exceeds this.
        threshold: usize,
        /// How many recent messages to keep verbatim.
        keep_recent: usize,
        /// Provider name for the summarization LLM call.
        /// Falls back to the node's own provider if `None`.
        #[serde(default)]
        summary_provider: Option<String>,
        /// Model for summarization.
        /// Falls back to the node's own model if `None`.
        #[serde(default)]
        summary_model: Option<String>,
    },
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_roundtrip_message_count() {
        let strategy = MemoryStrategy::MessageCount { max_messages: 20 };
        let json = serde_json::to_value(&strategy).unwrap();
        let back: MemoryStrategy = serde_json::from_value(json).unwrap();
        assert_eq!(strategy, back);
    }

    #[test]
    fn serialize_roundtrip_token_budget() {
        let strategy = MemoryStrategy::TokenBudget {
            max_tokens: 8000,
            reserve_system: true,
        };
        let json = serde_json::to_value(&strategy).unwrap();
        let back: MemoryStrategy = serde_json::from_value(json).unwrap();
        assert_eq!(strategy, back);
    }

    #[test]
    fn serialize_roundtrip_summarize() {
        let strategy = MemoryStrategy::Summarize {
            threshold: 30,
            keep_recent: 10,
            summary_provider: Some("openai".into()),
            summary_model: Some("gpt-4o-mini".into()),
        };
        let json = serde_json::to_value(&strategy).unwrap();
        let back: MemoryStrategy = serde_json::from_value(json).unwrap();
        assert_eq!(strategy, back);
    }

    #[test]
    fn deserialize_unknown_strategy_error() {
        let json = serde_json::json!({"strategy": "banana", "count": 5});
        let result = serde_json::from_value::<MemoryStrategy>(json);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("banana") || err.contains("unknown variant"),
            "got: {err}"
        );
    }

    #[test]
    fn token_budget_reserve_system_defaults_to_true() {
        let json = serde_json::json!({"strategy": "token_budget", "max_tokens": 4000});
        let strategy: MemoryStrategy = serde_json::from_value(json).unwrap();
        match strategy {
            MemoryStrategy::TokenBudget { reserve_system, .. } => {
                assert!(reserve_system);
            }
            _ => panic!("expected TokenBudget"),
        }
    }

    #[test]
    fn summarize_optional_fields_default_to_none() {
        let json = serde_json::json!({
            "strategy": "summarize",
            "threshold": 30,
            "keep_recent": 10
        });
        let strategy: MemoryStrategy = serde_json::from_value(json).unwrap();
        match strategy {
            MemoryStrategy::Summarize {
                summary_provider,
                summary_model,
                ..
            } => {
                assert!(summary_provider.is_none());
                assert!(summary_model.is_none());
            }
            _ => panic!("expected Summarize"),
        }
    }
}
