//! LLM invocation, streaming, and structured output types.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// ---------------------------------------------------------------------------
// LLM invocation types
// ---------------------------------------------------------------------------

/// A normalized LLM request, regardless of provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub struct LlmRequest {
    pub provider: String,
    pub model: String,
    pub messages: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<serde_json::Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_sequences: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_format: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    /// Provider-specific parameters not covered above.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra_params: BTreeMap<String, serde_json::Value>,
}

/// A normalized LLM response, regardless of provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub struct LlmResponse {
    pub content: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<LlmToolCall>>,
    pub model_used: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u32>,
    pub finish_reason: String,
    pub latency_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<LlmCost>,
}

/// A single tool call returned by the LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub struct LlmToolCall {
    pub id: String,
    pub tool_name: String,
    pub arguments: serde_json::Value,
}

/// Cost breakdown for a single LLM invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub struct LlmCost {
    pub input_cost_usd: f64,
    pub output_cost_usd: f64,
    pub total_cost_usd: f64,
    pub pricing_source: String,
}

/// Aggregate LLM stats for a run.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub struct LlmRunSummary {
    pub total_llm_calls: u32,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_cost_usd: Option<f64>,
    pub models_used: Vec<String>,
    pub tools_invoked: Vec<String>,
}

/// A complete record of one LLM request/response pair.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub struct LlmInvocationRecord {
    pub run_id: std::string::String,
    pub node_id: std::string::String,
    pub request: LlmRequest,
    pub response: LlmResponse,
    pub timestamp: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Structured output validation
// ---------------------------------------------------------------------------

/// Configuration for validating LLM responses against a JSON Schema.
///
/// When attached to an `LlmCallNode`, the engine validates the response content
/// against the provided schema and automatically retries with feedback on failure.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub struct StructuredOutputConfig {
    /// The JSON Schema that the LLM response must conform to.
    pub schema: serde_json::Value,

    /// Maximum number of retry attempts when validation fails (default: 2).
    #[serde(default = "default_max_retries")]
    pub max_retries: u8,

    /// When `true`, also sets `response_format` on the provider request so that
    /// providers with native structured-output support (e.g. OpenAI) can enforce
    /// the schema at generation time (default: `true`).
    #[serde(default = "default_strict")]
    pub strict: bool,
}

fn default_max_retries() -> u8 {
    2
}

fn default_strict() -> bool {
    true
}

// ---------------------------------------------------------------------------
// LLM streaming
// ---------------------------------------------------------------------------

/// A single chunk from a streaming LLM response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case", tag = "kind")]
#[non_exhaustive]
pub enum LlmChunk {
    TextDelta {
        delta: String,
    },
    ToolCallDelta {
        index: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        arguments_delta: String,
    },
    Usage {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        input_tokens: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output_tokens: Option<u32>,
    },
    Done {
        finish_reason: String,
    },
}
