//! Memory and context management for conversation history.
//!
//! This module provides automatic conversation history windowing and
//! summarization for LLM nodes. Without memory management, multi-turn
//! agent loops accumulate unbounded conversation history that exceeds
//! LLM context windows and wastes tokens.
//!
//! # Strategies
//!
//! Three strategies are available via [`MemoryStrategy`]:
//!
//! - **`MessageCount`** — keep the last N messages (simple, no token counting)
//! - **`TokenBudget`** — keep messages fitting within a token budget (newest first)
//! - **`Summarize`** — summarize older messages via an LLM call when history grows
//!
//! All strategies preserve the system message (if present at index 0).
//!
//! # Integration
//!
//! Memory management is applied automatically in `LlmCallNode` when the
//! node's config includes a `memory` key. For advanced pipelines, the
//! standalone `ContextManagementNode` can apply strategies as an explicit
//! graph step.
//!
//! # Token counting
//!
//! The [`TokenCounter`] trait abstracts token estimation. The default
//! [`CharEstimateCounter`] uses a chars/4 heuristic. For accurate counts,
//! inject a provider-specific tokenizer via `EngineBuilder::token_counter()`.
//!
//! # Configuration
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

pub mod config;
pub mod strategy;
pub mod token_counter;

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::traits::FlowLlmProvider;
use crate::types::{LlmRequest, NodeError};

pub use config::MemoryConfig;
pub use strategy::MemoryStrategy;
pub use token_counter::{CharEstimateCounter, TokenCounter};

/// Manages conversation history windowing and summarization.
///
/// Implementations apply a [`MemoryStrategy`] to trim messages before
/// they are sent to an LLM. The default [`InMemoryManager`] operates
/// directly on the message array with no external state.
#[async_trait]
pub trait MemoryManager: Send + Sync {
    /// Prepare messages for an LLM call by applying the given strategy.
    ///
    /// Returns the trimmed message array ready for the LLM request.
    /// The original (untrimmed) messages should still be persisted in
    /// state so future summarizations have the full history.
    async fn prepare_messages(
        &self,
        messages: Vec<Value>,
        strategy: &MemoryStrategy,
        token_counter: &dyn TokenCounter,
        llm_providers: &std::collections::HashMap<String, Arc<dyn FlowLlmProvider>>,
        default_provider: &str,
        default_model: &str,
    ) -> Result<Vec<Value>, NodeError>;
}

/// Default in-memory implementation of [`MemoryManager`].
///
/// Stateless — operates on the message array directly without any
/// external storage. Suitable for most use cases where conversation
/// history is already persisted in the run's `StateStore`.
pub struct InMemoryManager;

#[async_trait]
impl MemoryManager for InMemoryManager {
    async fn prepare_messages(
        &self,
        messages: Vec<Value>,
        strategy: &MemoryStrategy,
        token_counter: &dyn TokenCounter,
        llm_providers: &std::collections::HashMap<String, Arc<dyn FlowLlmProvider>>,
        default_provider: &str,
        default_model: &str,
    ) -> Result<Vec<Value>, NodeError> {
        match strategy {
            MemoryStrategy::MessageCount { max_messages } => {
                Ok(apply_message_count(messages, *max_messages))
            }
            MemoryStrategy::TokenBudget {
                max_tokens,
                reserve_system,
            } => Ok(apply_token_budget(
                messages,
                *max_tokens,
                *reserve_system,
                token_counter,
            )),
            MemoryStrategy::Summarize {
                threshold,
                keep_recent,
                summary_provider,
                summary_model,
                ..
            } => {
                apply_summarize(
                    messages,
                    *threshold,
                    *keep_recent,
                    summary_provider.as_deref().unwrap_or(default_provider),
                    summary_model.as_deref().unwrap_or(default_model),
                    llm_providers,
                )
                .await
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Strategy implementations
// ---------------------------------------------------------------------------

/// Keep system message (if at index 0) + last `max_messages` non-system messages.
fn apply_message_count(messages: Vec<Value>, max_messages: usize) -> Vec<Value> {
    if messages.is_empty() {
        return messages;
    }

    let (system, rest) = split_system(&messages);
    if rest.len() <= max_messages {
        return messages;
    }

    let mut result = Vec::new();
    if let Some(sys) = system {
        result.push(sys);
    }
    let start = rest.len().saturating_sub(max_messages);
    result.extend_from_slice(&rest[start..]);
    result
}

/// Keep messages fitting within `max_tokens`, iterating newest-first.
/// System message is always included; when `reserve_system` is true its
/// tokens don't count against the budget.
fn apply_token_budget(
    messages: Vec<Value>,
    max_tokens: usize,
    reserve_system: bool,
    counter: &dyn TokenCounter,
) -> Vec<Value> {
    if messages.is_empty() {
        return messages;
    }

    let (system, rest) = split_system(&messages);

    let mut budget = max_tokens;
    if !reserve_system {
        if let Some(ref sys) = system {
            let sys_tokens = counter.count_messages(std::slice::from_ref(sys));
            budget = budget.saturating_sub(sys_tokens);
        }
    }

    // Iterate from newest, accumulate tokens.
    let mut kept: Vec<Value> = Vec::new();
    for msg in rest.iter().rev() {
        let msg_tokens = counter.count_messages(std::slice::from_ref(msg));
        if msg_tokens > budget && !kept.is_empty() {
            break;
        }
        budget = budget.saturating_sub(msg_tokens);
        kept.push(msg.clone());
    }
    kept.reverse();

    let mut result = Vec::new();
    if let Some(sys) = system {
        result.push(sys);
    }
    result.extend(kept);
    result
}

/// Summarize older messages when count exceeds threshold.
async fn apply_summarize(
    messages: Vec<Value>,
    threshold: usize,
    keep_recent: usize,
    provider_name: &str,
    model_name: &str,
    llm_providers: &std::collections::HashMap<String, Arc<dyn FlowLlmProvider>>,
) -> Result<Vec<Value>, NodeError> {
    if messages.len() < threshold {
        return Ok(messages);
    }

    let (system, rest) = split_system(&messages);

    if rest.len() <= keep_recent {
        return Ok(messages);
    }

    let split_point = rest.len() - keep_recent;
    let to_summarize = &rest[..split_point];
    let to_keep = &rest[split_point..];

    // Build summarization prompt.
    let conversation_text = to_summarize
        .iter()
        .map(|msg| {
            let role = msg
                .get("role")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let content = msg
                .get("content")
                .map(|v| match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .unwrap_or_default();
            format!("{role}: {content}")
        })
        .collect::<Vec<_>>()
        .join("\n");

    let summary_prompt = format!(
        "Summarize the following conversation concisely, preserving key facts, \
         decisions, and context needed for the ongoing conversation:\n\n{conversation_text}"
    );

    let provider = llm_providers
        .get(provider_name)
        .ok_or_else(|| NodeError::Fatal {
            message: format!("summary LLM provider '{provider_name}' not registered in engine"),
        })?;

    let request = LlmRequest {
        provider: provider_name.to_string(),
        model: model_name.to_string(),
        messages: json!([{"role": "user", "content": summary_prompt}]),
        tools: None,
        temperature: Some(0.3),
        top_p: None,
        max_tokens: Some(1024),
        stop_sequences: None,
        response_format: None,
        seed: None,
        extra_params: BTreeMap::new(),
    };

    let response = provider
        .complete(request)
        .await
        .map_err(|e| NodeError::Retryable {
            message: format!("summarization LLM call failed: {e}"),
        })?;

    let summary_text = match &response.content {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };

    let summary_msg = json!({
        "role": "system",
        "content": format!("[Summary of earlier conversation]\n{summary_text}")
    });

    let mut result = Vec::new();
    if let Some(sys) = system {
        result.push(sys);
    }
    result.push(summary_msg);
    result.extend_from_slice(to_keep);
    Ok(result)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Split messages into optional system message (index 0) and the rest.
fn split_system(messages: &[Value]) -> (Option<Value>, Vec<Value>) {
    if messages.is_empty() {
        return (None, Vec::new());
    }

    let first_is_system = messages[0]
        .get("role")
        .and_then(|v| v.as_str())
        .map(|r| r == "system")
        .unwrap_or(false);

    if first_is_system {
        (Some(messages[0].clone()), messages[1..].to_vec())
    } else {
        (None, messages.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::LlmResponse;
    use serde_json::json;

    fn make_msg(role: &str, content: &str) -> Value {
        json!({"role": role, "content": content})
    }

    fn make_messages(count: usize) -> Vec<Value> {
        let mut msgs = vec![make_msg("system", "You are helpful.")];
        for i in 0..count {
            let role = if i % 2 == 0 { "user" } else { "assistant" };
            msgs.push(make_msg(role, &format!("Message {i}")));
        }
        msgs
    }

    // -- MessageCount tests --

    #[test]
    fn message_count_under_limit() {
        let msgs = make_messages(3);
        let result = apply_message_count(msgs.clone(), 5);
        assert_eq!(result.len(), msgs.len());
    }

    #[test]
    fn message_count_exact_limit() {
        let msgs = make_messages(5);
        let result = apply_message_count(msgs.clone(), 5);
        assert_eq!(result.len(), msgs.len());
    }

    #[test]
    fn message_count_over_limit() {
        let msgs = make_messages(10);
        let result = apply_message_count(msgs, 5);
        // system + last 5 non-system = 6
        assert_eq!(result.len(), 6);
        assert_eq!(result[0]["role"], "system");
        assert_eq!(result[5]["content"], "Message 9");
    }

    #[test]
    fn message_count_preserves_system_message() {
        let msgs = make_messages(10);
        let result = apply_message_count(msgs, 3);
        assert_eq!(result[0]["role"], "system");
        assert_eq!(result[0]["content"], "You are helpful.");
    }

    #[test]
    fn message_count_no_system_message() {
        let msgs = vec![
            make_msg("user", "Hello"),
            make_msg("assistant", "Hi"),
            make_msg("user", "Bye"),
        ];
        let result = apply_message_count(msgs, 2);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0]["role"], "assistant");
        assert_eq!(result[1]["role"], "user");
    }

    #[test]
    fn message_count_limit_one() {
        let msgs = make_messages(5);
        let result = apply_message_count(msgs, 1);
        // system + last 1
        assert_eq!(result.len(), 2);
        assert_eq!(result[0]["role"], "system");
        assert_eq!(result[1]["content"], "Message 4");
    }

    // -- TokenBudget tests --

    #[test]
    fn token_budget_all_fit() {
        let counter = CharEstimateCounter;
        let msgs = make_messages(3);
        let result = apply_token_budget(msgs.clone(), 10000, true, &counter);
        assert_eq!(result.len(), msgs.len());
    }

    #[test]
    fn token_budget_trims_oldest_first() {
        let counter = CharEstimateCounter;
        let msgs = make_messages(10);
        // Very small budget — should keep only most recent
        let result = apply_token_budget(msgs, 10, true, &counter);
        assert!(result.len() < 11); // fewer than all
                                    // System preserved
        assert_eq!(result[0]["role"], "system");
        // Last message preserved
        assert_eq!(result.last().unwrap()["content"], "Message 9");
    }

    #[test]
    fn token_budget_preserves_system_with_reserve() {
        let counter = CharEstimateCounter;
        let msgs = vec![
            make_msg("system", &"x".repeat(100)), // ~25 tokens
            make_msg("user", "Hi"),               // ~1 token
        ];
        // Budget of 5 — system reserved, so only user message counts
        let result = apply_token_budget(msgs, 5, true, &counter);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn token_budget_system_counts_when_not_reserved() {
        let counter = CharEstimateCounter;
        let msgs = vec![
            make_msg("system", &"x".repeat(100)), // ~25 tokens
            make_msg("user", "Hi"),
            make_msg("assistant", "Hello"),
        ];
        // Budget of 5 — system NOT reserved, eats most of the budget
        let result = apply_token_budget(msgs, 5, false, &counter);
        // System still included, but budget squeezed
        assert_eq!(result[0]["role"], "system");
    }

    #[test]
    fn token_budget_single_message_exceeds() {
        let counter = CharEstimateCounter;
        let msgs = vec![make_msg("user", &"x".repeat(1000))];
        // Budget way too small, but single message should still be included
        let result = apply_token_budget(msgs, 1, true, &counter);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn token_budget_zero_budget() {
        let counter = CharEstimateCounter;
        let msgs = make_messages(3);
        let result = apply_token_budget(msgs, 0, true, &counter);
        // System + at least most recent message
        assert!(result.len() >= 2);
        assert_eq!(result[0]["role"], "system");
    }

    // -- Summarize tests --

    /// Mock LLM provider for summarization tests.
    struct MockSummaryProvider {
        summary_text: String,
    }

    #[async_trait]
    impl FlowLlmProvider for MockSummaryProvider {
        async fn complete(&self, _request: LlmRequest) -> Result<LlmResponse, NodeError> {
            Ok(LlmResponse {
                content: json!(self.summary_text),
                tool_calls: None,
                model_used: "mock".into(),
                input_tokens: Some(10),
                output_tokens: Some(20),
                total_tokens: Some(30),
                finish_reason: "stop".into(),
                latency_ms: 50,
                provider_request_id: None,
                cost: None,
            })
        }

        fn name(&self) -> &str {
            "mock-summary"
        }
    }

    fn mock_providers(
        summary: &str,
    ) -> std::collections::HashMap<String, Arc<dyn FlowLlmProvider>> {
        let mut map = std::collections::HashMap::new();
        map.insert(
            "mock".to_string(),
            Arc::new(MockSummaryProvider {
                summary_text: summary.to_string(),
            }) as Arc<dyn FlowLlmProvider>,
        );
        map
    }

    #[tokio::test]
    async fn summarize_below_threshold_noop() {
        let msgs = make_messages(5);
        let providers = mock_providers("should not be called");
        let result = apply_summarize(msgs.clone(), 20, 3, "mock", "model", &providers)
            .await
            .unwrap();
        assert_eq!(result.len(), msgs.len());
    }

    #[tokio::test]
    async fn summarize_triggers_at_threshold() {
        // 1 system + 6 non-system = 7 total, threshold=7
        let msgs = make_messages(6);
        let providers = mock_providers("Summary of old messages");
        let result = apply_summarize(msgs, 7, 3, "mock", "model", &providers)
            .await
            .unwrap();
        // system + summary + 3 recent = 5
        assert_eq!(result.len(), 5);
        assert_eq!(result[0]["role"], "system");
        assert!(result[1]["content"]
            .as_str()
            .unwrap()
            .contains("[Summary of earlier conversation]"));
    }

    #[tokio::test]
    async fn summarize_above_threshold() {
        let msgs = make_messages(10);
        let providers = mock_providers("Condensed conversation");
        let result = apply_summarize(msgs, 5, 3, "mock", "model", &providers)
            .await
            .unwrap();
        // system + summary + 3 recent = 5
        assert_eq!(result.len(), 5);
        assert!(result[1]["content"]
            .as_str()
            .unwrap()
            .contains("Condensed conversation"));
    }

    #[tokio::test]
    async fn summarize_preserves_system_message() {
        let msgs = make_messages(10);
        let providers = mock_providers("summary");
        let result = apply_summarize(msgs, 5, 3, "mock", "model", &providers)
            .await
            .unwrap();
        assert_eq!(result[0]["role"], "system");
        assert_eq!(result[0]["content"], "You are helpful.");
    }

    #[tokio::test]
    async fn summarize_preserves_keep_recent() {
        let msgs = make_messages(10);
        let providers = mock_providers("summary");
        let result = apply_summarize(msgs.clone(), 5, 4, "mock", "model", &providers)
            .await
            .unwrap();
        // Last 4 non-system messages preserved
        let last_4: Vec<_> = msgs[msgs.len() - 4..].to_vec();
        let result_last_4: Vec<_> = result[result.len() - 4..].to_vec();
        assert_eq!(last_4, result_last_4);
    }

    #[tokio::test]
    async fn summarize_summary_format() {
        let msgs = make_messages(10);
        let providers = mock_providers("The user discussed weather");
        let result = apply_summarize(msgs, 5, 3, "mock", "model", &providers)
            .await
            .unwrap();
        let summary = result[1]["content"].as_str().unwrap();
        assert!(summary.starts_with("[Summary of earlier conversation]"));
        assert!(summary.contains("The user discussed weather"));
        assert_eq!(result[1]["role"], "system");
    }

    #[tokio::test]
    async fn summarize_llm_failure_returns_error() {
        struct FailingProvider;

        #[async_trait]
        impl FlowLlmProvider for FailingProvider {
            async fn complete(&self, _request: LlmRequest) -> Result<LlmResponse, NodeError> {
                Err(NodeError::Fatal {
                    message: "API error".into(),
                })
            }
            fn name(&self) -> &str {
                "failing"
            }
        }

        let mut providers: std::collections::HashMap<String, Arc<dyn FlowLlmProvider>> =
            std::collections::HashMap::new();
        providers.insert("fail".into(), Arc::new(FailingProvider) as _);

        let msgs = make_messages(10);
        let result = apply_summarize(msgs, 5, 3, "fail", "model", &providers).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            NodeError::Retryable { message, .. } => {
                assert!(message.contains("summarization"), "got: {message}");
            }
            other => panic!("expected Retryable, got: {other}"),
        }
    }

    #[tokio::test]
    async fn summarize_fallback_provider() {
        let msgs = make_messages(10);
        let providers = mock_providers("summary via default");
        // provider_name "mock" matches what's in the providers map
        let result = apply_summarize(msgs, 5, 3, "mock", "model", &providers)
            .await
            .unwrap();
        assert!(result.len() > 1);
    }

    #[tokio::test]
    async fn summarize_custom_provider() {
        let mut providers: std::collections::HashMap<String, Arc<dyn FlowLlmProvider>> =
            std::collections::HashMap::new();
        providers.insert(
            "custom".into(),
            Arc::new(MockSummaryProvider {
                summary_text: "custom summary".into(),
            }) as _,
        );

        let msgs = make_messages(10);
        let result = apply_summarize(msgs, 5, 3, "custom", "model", &providers)
            .await
            .unwrap();
        assert!(result[1]["content"]
            .as_str()
            .unwrap()
            .contains("custom summary"));
    }

    // -- InMemoryManager trait tests --

    #[tokio::test]
    async fn in_memory_manager_none_passthrough() {
        // Test that MessageCount with large limit acts as passthrough
        let mgr = InMemoryManager;
        let msgs = make_messages(3);
        let counter = CharEstimateCounter;
        let providers = std::collections::HashMap::new();
        let strategy = MemoryStrategy::MessageCount { max_messages: 100 };

        let result = mgr
            .prepare_messages(msgs.clone(), &strategy, &counter, &providers, "p", "m")
            .await
            .unwrap();
        assert_eq!(result.len(), msgs.len());
    }

    #[tokio::test]
    async fn in_memory_manager_message_count() {
        let mgr = InMemoryManager;
        let msgs = make_messages(10);
        let counter = CharEstimateCounter;
        let providers = std::collections::HashMap::new();
        let strategy = MemoryStrategy::MessageCount { max_messages: 3 };

        let result = mgr
            .prepare_messages(msgs, &strategy, &counter, &providers, "p", "m")
            .await
            .unwrap();
        assert_eq!(result.len(), 4); // system + 3
    }

    #[tokio::test]
    async fn in_memory_manager_token_budget() {
        let mgr = InMemoryManager;
        let msgs = make_messages(10);
        let counter = CharEstimateCounter;
        let providers = std::collections::HashMap::new();
        let strategy = MemoryStrategy::TokenBudget {
            max_tokens: 20,
            reserve_system: true,
        };

        let result = mgr
            .prepare_messages(msgs, &strategy, &counter, &providers, "p", "m")
            .await
            .unwrap();
        assert!(result.len() < 11);
        assert_eq!(result[0]["role"], "system");
    }

    #[tokio::test]
    async fn in_memory_manager_summarize() {
        let mgr = InMemoryManager;
        let msgs = make_messages(10);
        let counter = CharEstimateCounter;
        let providers = mock_providers("End-to-end summary");
        let strategy = MemoryStrategy::Summarize {
            threshold: 5,
            keep_recent: 3,
            summary_provider: Some("mock".into()),
            summary_model: Some("model".into()),
        };

        let result = mgr
            .prepare_messages(msgs, &strategy, &counter, &providers, "default", "model")
            .await
            .unwrap();
        assert_eq!(result.len(), 5);
        assert!(result[1]["content"]
            .as_str()
            .unwrap()
            .contains("End-to-end summary"));
    }
}
