//! Memory configuration deserialized from node config JSON.
//!
//! The [`MemoryConfig`] struct is read from the `memory` key in a node's
//! config object. When absent or when `strategy` is `None`, no memory
//! management is applied (backward-compatible with pre-O4 behavior).
//!
//! # Example
//!
//! ```json
//! {
//!   "provider": "openai",
//!   "model": "gpt-4",
//!   "memory": {
//!     "strategy": "token_budget",
//!     "max_tokens": 8000,
//!     "reserve_system": true
//!   }
//! }
//! ```

use serde::Deserialize;

use super::strategy::MemoryStrategy;

/// Per-node memory configuration.
///
/// Deserialized from the `memory` key in a node's config JSON.
/// When `strategy` is `None`, no memory management is applied.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct MemoryConfig {
    /// The memory strategy to apply. `None` means no windowing.
    #[serde(flatten)]
    pub strategy: Option<MemoryStrategy>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn deserialize_no_memory_config() {
        let config: MemoryConfig = serde_json::from_value(json!({})).unwrap();
        assert!(config.strategy.is_none());
    }

    #[test]
    fn deserialize_message_count() {
        let config: MemoryConfig =
            serde_json::from_value(json!({"strategy": "message_count", "max_messages": 20}))
                .unwrap();
        assert!(matches!(
            config.strategy,
            Some(MemoryStrategy::MessageCount { max_messages: 20 })
        ));
    }

    #[test]
    fn deserialize_token_budget() {
        let config: MemoryConfig = serde_json::from_value(json!({
            "strategy": "token_budget",
            "max_tokens": 4000,
            "reserve_system": true
        }))
        .unwrap();
        assert!(matches!(
            config.strategy,
            Some(MemoryStrategy::TokenBudget {
                max_tokens: 4000,
                reserve_system: true,
            })
        ));
    }

    #[test]
    fn deserialize_summarize_full() {
        let config: MemoryConfig = serde_json::from_value(json!({
            "strategy": "summarize",
            "threshold": 30,
            "keep_recent": 10,
            "summary_provider": "openai",
            "summary_model": "gpt-4o-mini"
        }))
        .unwrap();
        match config.strategy {
            Some(MemoryStrategy::Summarize {
                threshold,
                keep_recent,
                summary_provider,
                summary_model,
            }) => {
                assert_eq!(threshold, 30);
                assert_eq!(keep_recent, 10);
                assert_eq!(summary_provider.as_deref(), Some("openai"));
                assert_eq!(summary_model.as_deref(), Some("gpt-4o-mini"));
            }
            other => panic!("expected Summarize, got: {other:?}"),
        }
    }

    #[test]
    fn deserialize_summarize_defaults() {
        let config: MemoryConfig = serde_json::from_value(json!({
            "strategy": "summarize",
            "threshold": 20,
            "keep_recent": 5
        }))
        .unwrap();
        match config.strategy {
            Some(MemoryStrategy::Summarize {
                summary_provider,
                summary_model,
                ..
            }) => {
                assert!(summary_provider.is_none());
                assert!(summary_model.is_none());
            }
            other => panic!("expected Summarize, got: {other:?}"),
        }
    }
}
