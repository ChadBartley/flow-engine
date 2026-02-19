use async_trait::async_trait;
use futures::Stream;
use serde::{Deserialize, Serialize};
use std::pin::Pin;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("HTTP request failed: {0}")]
    RequestFailed(String),
    #[error("Invalid response: {0}")]
    InvalidResponse(String),
    #[error("API error: {0}")]
    ApiError(String),
    #[error("Streaming error: {0}")]
    StreamError(String),
}

pub type ProviderResult<T> = Result<T, ProviderError>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: ToolCallFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallFunction {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    #[serde(default)]
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionRequest {
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<serde_json::Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionResponse {
    pub content: String,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub model: String,
    pub provider: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(default = "default_finish_reason")]
    pub finish_reason: String,
}

fn default_finish_reason() -> String {
    "stop".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamChunk {
    pub content: String,
    pub is_final: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completion_tokens: Option<u64>,
}

/// Trait for extracting token usage from provider-specific streaming responses
pub trait StreamResponseTokens {
    /// Extract prompt token count from the response
    /// Returns None if no prompt token data is available in this chunk
    fn extract_prompt_tokens(&self) -> Option<u64>;

    /// Extract completion token count from the response
    /// Returns None if no completion token data is available in this chunk
    fn extract_completion_tokens(&self) -> Option<u64>;
}

/// Unified interface for LLM providers (OpenAI, Anthropic, Gemini, Ollama, etc.).
///
/// Implementations handle request translation, API communication, and response
/// normalization for a specific provider backend.
///
/// # Example
///
/// ```ignore
/// let provider: &dyn LLMProvider = &openai_provider;
/// let req = CompletionRequest {
///     model: "gpt-4".into(),
///     messages: vec![Message { role: "user".into(), content: "Hello".into(), ..Default::default() }],
///     ..Default::default()
/// };
/// let resp = provider.complete(req).await?;
/// println!("tokens used: {} + {}", resp.prompt_tokens, resp.completion_tokens);
/// ```
#[async_trait]
pub trait LLMProvider: Send + Sync {
    /// Send a completion request and return the full response.
    async fn complete(&self, req: CompletionRequest) -> ProviderResult<CompletionResponse>;

    /// Send a streaming completion request, returning chunks as they arrive.
    async fn stream(
        &self,
        req: CompletionRequest,
    ) -> ProviderResult<Pin<Box<dyn Stream<Item = ProviderResult<StreamChunk>> + Send>>>;

    /// Look up pricing for a model. Returns `None` if unknown.
    fn get_pricing(&self, model: &str) -> Option<crate::pricing::ModelPricing>;

    /// Provider name for diagnostics and routing (e.g. `"openai"`, `"anthropic"`).
    fn name(&self) -> &str;

    /// Normalize a model name to its canonical form for pricing lookups.
    ///
    /// This allows providers to strip version numbers, dates, or other suffixes
    /// that don't affect pricing. For example:
    /// - OpenAI: "gpt-4-0613" -> "gpt-4"
    /// - Anthropic: "claude-3-sonnet-20240229" -> "claude-3-sonnet"
    ///
    /// Default implementation:
    /// 1. Returns model as-is if exact pricing exists (O(1) fast path)
    /// 2. Tries stripping suffix after last dash and checks pricing
    /// 3. Returns original if no match found
    fn normalize_model_name(&self, model: &str) -> String {
        // If we have exact pricing, use as-is (fast path)
        if self.get_pricing(model).is_some() {
            return model.to_string();
        }

        // Try stripping suffix after last dash (e.g., "gpt-4-0613" -> "gpt-4")
        if let Some(pos) = model.rfind('-') {
            let base = &model[..pos];
            if self.get_pricing(base).is_some() {
                return base.to_string();
            }
        }

        // No match found, return original
        model.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MockProvider;

    #[test]
    fn test_normalize_model_name_with_exact_match() {
        let models = vec![(
            "test-model".to_string(),
            crate::pricing::ModelPricing {
                prompt: 0.00003,
                completion: 0.00006,
            },
        )];

        let provider = MockProvider::instant(models);

        // Exact match should return as-is
        assert_eq!(provider.normalize_model_name("test-model"), "test-model");
    }

    #[test]
    fn test_normalize_model_name_with_version_suffix() {
        let models = vec![
            (
                "test-model".to_string(),
                crate::pricing::ModelPricing {
                    prompt: 0.00003,
                    completion: 0.00006,
                },
            ),
            (
                "test-model-turbo".to_string(),
                crate::pricing::ModelPricing {
                    prompt: 0.00005,
                    completion: 0.00010,
                },
            ),
        ];

        let provider = MockProvider::instant(models);

        // Should strip version suffix and return base model
        assert_eq!(
            provider.normalize_model_name("test-model-20240101"),
            "test-model"
        );
        assert_eq!(
            provider.normalize_model_name("test-model-turbo-preview"),
            "test-model-turbo"
        );
    }

    #[test]
    fn test_normalize_model_name_no_match() {
        let models = vec![];
        let provider = MockProvider::instant(models);

        // No pricing found, should return original
        assert_eq!(
            provider.normalize_model_name("unknown-model"),
            "unknown-model"
        );
        assert_eq!(
            provider.normalize_model_name("unknown-model-v2"),
            "unknown-model-v2"
        );
    }

    #[test]
    fn test_normalize_model_name_no_dash() {
        let models = vec![(
            "testmodel".to_string(),
            crate::pricing::ModelPricing {
                prompt: 0.00003,
                completion: 0.00006,
            },
        )];

        let provider = MockProvider::instant(models);

        // No dash to strip, should return as-is
        assert_eq!(provider.normalize_model_name("testmodel"), "testmodel");
    }
}
