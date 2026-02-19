//! Foundational types for the FlowEngine v2 execution model.
//!
//! Every type here is `Serialize + Deserialize + Debug + Clone`. All map
//! fields use `BTreeMap` (never `HashMap`) to guarantee deterministic
//! serialization — this is a correctness invariant for content-addressed
//! hashing, not a style choice.
//!
//! All enums use `#[non_exhaustive]` so adding variants is never a
//! breaking change for downstream consumers.

pub mod execution;
pub mod graph;
pub mod llm;
pub mod triggers;

// Re-export all types at module level for backwards compatibility.
pub use execution::*;
pub use graph::*;
pub use llm::*;
pub use triggers::*;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Current schema version for GraphDef serialization.
pub const GRAPH_SCHEMA_VERSION: u16 = 1;

/// Current schema version for WriteEvent serialization.
pub const WRITE_EVENT_SCHEMA_VERSION: u16 = 1;

// ---------------------------------------------------------------------------
// Port system
// ---------------------------------------------------------------------------

/// Data types for node ports. Governs type checking between connected ports.
///
/// `Custom(String)` is the escape hatch for plugin-defined types.
/// Deserializers must handle unknown variants gracefully via `Custom`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PortType {
    String,
    Number,
    Bool,
    Json,
    List(Box<PortType>),
    Map(Box<PortType>),
    Binary,
    Any,
    Custom(std::string::String),
}

/// Sensitivity classification for data fields.
/// Governs redaction behavior in the write pipeline.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Sensitivity {
    #[default]
    None,
    Pii,
    Masked,
    Secret,
    Custom(std::string::String),
}

/// A typed port on a node: name, type, sensitivity, and defaults.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub struct PortDef {
    pub name: std::string::String,
    pub port_type: PortType,
    /// Whether this port must be connected. Default: `true`.
    #[serde(default = "default_true")]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<std::string::String>,
    #[serde(default)]
    pub sensitivity: Sensitivity,
}

fn default_true() -> bool {
    true
}

// ---------------------------------------------------------------------------
// Node metadata
// ---------------------------------------------------------------------------

/// Everything the UI and engine need about a node type without executing it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub struct NodeMeta {
    pub node_type: std::string::String,
    pub label: std::string::String,
    pub category: std::string::String,
    pub inputs: Vec<PortDef>,
    pub outputs: Vec<PortDef>,
    /// JSON Schema — the UI renders this as a config form.
    pub config_schema: serde_json::Value,
    #[serde(default)]
    pub ui: NodeUiHints,
    #[serde(default)]
    pub execution: ExecutionHints,
}

/// Visual hints consumed by the drag-and-drop UI.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub struct NodeUiHints {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<std::string::String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<std::string::String>,
    #[serde(default)]
    pub dynamic_ports: bool,
}

/// Execution tuning — all fields have sensible defaults.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub struct ExecutionHints {
    /// Node timeout in milliseconds. Default: 30 000 (30 s).
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default)]
    pub retry: RetryPolicy,
    /// `None` means unlimited concurrency.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrency: Option<u32>,
}

impl Default for ExecutionHints {
    fn default() -> Self {
        Self {
            timeout_ms: default_timeout_ms(),
            retry: RetryPolicy::default(),
            max_concurrency: None,
        }
    }
}

fn default_timeout_ms() -> u64 {
    30_000
}

/// Retry policy with exponential backoff.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub struct RetryPolicy {
    /// Total attempts (1 = no retry). Default: 1.
    #[serde(default = "default_max_attempts")]
    pub max_attempts: u32,
    /// Initial backoff in milliseconds. Default: 1 000.
    #[serde(default = "default_backoff_ms")]
    pub backoff_ms: u64,
    /// Backoff multiplier per attempt. Default: 2.0.
    #[serde(default = "default_backoff_multiplier")]
    pub backoff_multiplier: f64,
    /// If set, only retry errors whose message matches one of these strings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retryable_errors: Option<Vec<std::string::String>>,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: default_max_attempts(),
            backoff_ms: default_backoff_ms(),
            backoff_multiplier: default_backoff_multiplier(),
            retryable_errors: None,
        }
    }
}

fn default_max_attempts() -> u32 {
    1
}
fn default_backoff_ms() -> u64 {
    1_000
}
fn default_backoff_multiplier() -> f64 {
    2.0
}

// ---------------------------------------------------------------------------
// Node errors
// ---------------------------------------------------------------------------

/// Structured, retryable-aware errors returned by node execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case", tag = "kind")]
#[non_exhaustive]
pub enum NodeError {
    Retryable { message: std::string::String },
    Fatal { message: std::string::String },
    Timeout { elapsed_ms: u64 },
    Validation { message: std::string::String },
}

impl std::fmt::Display for NodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Retryable { message } => write!(f, "retryable: {message}"),
            Self::Fatal { message } => write!(f, "fatal: {message}"),
            Self::Timeout { elapsed_ms } => write!(f, "timeout after {elapsed_ms}ms"),
            Self::Validation { message } => write!(f, "validation: {message}"),
        }
    }
}

impl std::error::Error for NodeError {}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Round-trip a type through JSON and verify equality.
    fn round_trip<T: Serialize + for<'de> Deserialize<'de> + std::fmt::Debug>(val: &T) -> T {
        let json = serde_json::to_string(val).expect("serialize");
        serde_json::from_str(&json).expect("deserialize")
    }

    #[test]
    fn port_type_round_trip() {
        let types = vec![
            PortType::String,
            PortType::Number,
            PortType::Bool,
            PortType::Json,
            PortType::List(Box::new(PortType::String)),
            PortType::Map(Box::new(PortType::Number)),
            PortType::Binary,
            PortType::Any,
            PortType::Custom("my_type".into()),
        ];
        for t in &types {
            let rt = round_trip(t);
            assert_eq!(t, &rt);
        }
    }

    #[test]
    fn sensitivity_round_trip() {
        let values = vec![
            Sensitivity::None,
            Sensitivity::Pii,
            Sensitivity::Masked,
            Sensitivity::Secret,
            Sensitivity::Custom("hipaa".into()),
        ];
        for v in &values {
            let rt = round_trip(v);
            assert_eq!(v, &rt);
        }
    }

    #[test]
    fn sensitivity_default_is_none() {
        assert_eq!(Sensitivity::default(), Sensitivity::None);
    }

    #[test]
    fn retry_policy_defaults() {
        let p = RetryPolicy::default();
        assert_eq!(p.max_attempts, 1);
        assert_eq!(p.backoff_ms, 1_000);
        assert!((p.backoff_multiplier - 2.0).abs() < f64::EPSILON);
        assert!(p.retryable_errors.is_none());
    }

    #[test]
    fn execution_hints_defaults() {
        let h = ExecutionHints::default();
        assert_eq!(h.timeout_ms, 30_000);
        assert_eq!(h.retry.max_attempts, 1);
        assert!(h.max_concurrency.is_none());
    }

    #[test]
    fn btreemap_determinism() {
        use std::collections::BTreeMap;
        let mut meta_a = BTreeMap::new();
        meta_a.insert("z_key".to_string(), json!("z_val"));
        meta_a.insert("a_key".to_string(), json!("a_val"));

        let mut meta_b = BTreeMap::new();
        meta_b.insert("a_key".to_string(), json!("a_val"));
        meta_b.insert("z_key".to_string(), json!("z_val"));

        let mut tools_a = BTreeMap::new();
        tools_a.insert(
            "tool_b".to_string(),
            ToolDef {
                name: "tool_b".into(),
                description: "B".into(),
                parameters: json!({}),
                tool_type: ToolType::Node {
                    target_node_id: "n1".into(),
                },
                metadata: BTreeMap::new(),
            },
        );
        tools_a.insert(
            "tool_a".to_string(),
            ToolDef {
                name: "tool_a".into(),
                description: "A".into(),
                parameters: json!({}),
                tool_type: ToolType::Http {
                    url: "http://example.com".into(),
                    method: "POST".into(),
                    headers: BTreeMap::new(),
                },
                metadata: BTreeMap::new(),
            },
        );

        let mut tools_b = BTreeMap::new();
        tools_b.insert(
            "tool_a".to_string(),
            ToolDef {
                name: "tool_a".into(),
                description: "A".into(),
                parameters: json!({}),
                tool_type: ToolType::Http {
                    url: "http://example.com".into(),
                    method: "POST".into(),
                    headers: BTreeMap::new(),
                },
                metadata: BTreeMap::new(),
            },
        );
        tools_b.insert(
            "tool_b".to_string(),
            ToolDef {
                name: "tool_b".into(),
                description: "B".into(),
                parameters: json!({}),
                tool_type: ToolType::Node {
                    target_node_id: "n1".into(),
                },
                metadata: BTreeMap::new(),
            },
        );

        let graph_a = GraphDef {
            schema_version: GRAPH_SCHEMA_VERSION,
            id: "flow-1".into(),
            name: "Test Flow".into(),
            version: "1".into(),
            nodes: vec![],
            edges: vec![],
            metadata: meta_a,
            tool_definitions: tools_a,
        };

        let graph_b = GraphDef {
            schema_version: GRAPH_SCHEMA_VERSION,
            id: "flow-1".into(),
            name: "Test Flow".into(),
            version: "1".into(),
            nodes: vec![],
            edges: vec![],
            metadata: meta_b,
            tool_definitions: tools_b,
        };

        let json_a = serde_json::to_string(&graph_a).unwrap();
        let json_b = serde_json::to_string(&graph_b).unwrap();
        assert_eq!(
            json_a, json_b,
            "BTreeMap insertion order must not affect JSON output"
        );
    }

    #[test]
    fn tool_type_variants_round_trip() {
        use std::collections::BTreeMap;
        let node_tool = ToolType::Node {
            target_node_id: "n1".into(),
        };
        let http_tool = ToolType::Http {
            url: "https://api.example.com".into(),
            method: "POST".into(),
            headers: {
                let mut h = BTreeMap::new();
                h.insert("Authorization".into(), "Bearer xxx".into());
                h
            },
        };
        let mcp_tool = ToolType::Mcp {
            server: "my_mcp".into(),
            tool_name: "search".into(),
        };

        for t in &[node_tool, http_tool, mcp_tool] {
            let json = serde_json::to_string(t).unwrap();
            let rt: ToolType = serde_json::from_str(&json).unwrap();
            let json2 = serde_json::to_string(&rt).unwrap();
            assert_eq!(json, json2);
        }
    }

    #[test]
    fn llm_request_round_trip() {
        use std::collections::BTreeMap;
        let req = LlmRequest {
            provider: "openai".into(),
            model: "gpt-4o".into(),
            messages: json!([{"role": "user", "content": "hello"}]),
            tools: Some(vec![
                json!({"type": "function", "function": {"name": "search"}}),
            ]),
            temperature: Some(0.7),
            top_p: None,
            max_tokens: Some(4096),
            stop_sequences: None,
            response_format: None,
            seed: Some(42),
            extra_params: BTreeMap::new(),
        };
        let rt = round_trip(&req);
        assert_eq!(req.provider, rt.provider);
        assert_eq!(req.model, rt.model);
        assert_eq!(req.temperature, rt.temperature);
        assert_eq!(req.max_tokens, rt.max_tokens);
        assert_eq!(req.seed, rt.seed);
    }

    #[test]
    fn llm_response_round_trip() {
        let resp = LlmResponse {
            content: json!("Hello, world!"),
            tool_calls: Some(vec![LlmToolCall {
                id: "call_1".into(),
                tool_name: "search".into(),
                arguments: json!({"query": "rust"}),
            }]),
            model_used: "gpt-4o-2024-08-06".into(),
            input_tokens: Some(100),
            output_tokens: Some(50),
            total_tokens: Some(150),
            finish_reason: "tool_calls".into(),
            latency_ms: 1234,
            provider_request_id: Some("req_abc".into()),
            cost: Some(LlmCost {
                input_cost_usd: 0.001,
                output_cost_usd: 0.002,
                total_cost_usd: 0.003,
                pricing_source: "config".into(),
            }),
        };
        let rt = round_trip(&resp);
        assert_eq!(resp.model_used, rt.model_used);
        assert_eq!(resp.finish_reason, rt.finish_reason);
        assert_eq!(resp.latency_ms, rt.latency_ms);
        assert_eq!(resp.input_tokens, rt.input_tokens);
        assert!(rt.cost.is_some());
        assert!(rt.tool_calls.is_some());
        assert_eq!(rt.tool_calls.unwrap().len(), 1);
    }

    #[test]
    fn llm_run_summary_round_trip() {
        let summary = LlmRunSummary {
            total_llm_calls: 5,
            total_input_tokens: 10_000,
            total_output_tokens: 3_000,
            total_cost_usd: Some(0.15),
            models_used: vec!["gpt-4o".into(), "claude-sonnet-4-20250514".into()],
            tools_invoked: vec!["search".into(), "calculator".into()],
        };
        let rt = round_trip(&summary);
        assert_eq!(summary.total_llm_calls, rt.total_llm_calls);
        assert_eq!(summary.total_input_tokens, rt.total_input_tokens);
        assert_eq!(summary.models_used, rt.models_used);
        assert_eq!(summary.tools_invoked, rt.tools_invoked);
    }

    #[test]
    fn run_status_round_trip() {
        let statuses = vec![
            RunStatus::Pending,
            RunStatus::Running,
            RunStatus::Completed,
            RunStatus::Failed,
            RunStatus::Cancelled,
            RunStatus::Interrupted,
        ];
        for s in &statuses {
            let rt = round_trip(s);
            assert_eq!(s, &rt);
        }
    }

    #[test]
    fn trigger_source_all_variants() {
        use chrono::Utc;
        let sources = vec![
            TriggerSource::Api {
                request_id: "req-1".into(),
            },
            TriggerSource::Cron {
                schedule: "0 9 * * MON-FRI".into(),
                tick: Utc::now(),
            },
            TriggerSource::Flow {
                source_run_id: "run-1".into(),
                source_flow_id: "flow-1".into(),
            },
            TriggerSource::Queue {
                topic: "events".into(),
                message_id: "msg-1".into(),
            },
            TriggerSource::Manual {
                principal: "admin".into(),
            },
            TriggerSource::Custom {
                trigger_type: "webhook".into(),
                metadata: json!({"key": "val"}),
            },
        ];
        for s in &sources {
            let json = serde_json::to_string(s).unwrap();
            let rt: TriggerSource = serde_json::from_str(&json).unwrap();
            let json2 = serde_json::to_string(&rt).unwrap();
            assert_eq!(json, json2);
        }
    }

    #[test]
    fn node_error_display() {
        assert_eq!(
            NodeError::Retryable {
                message: "oops".into()
            }
            .to_string(),
            "retryable: oops"
        );
        assert_eq!(
            NodeError::Timeout { elapsed_ms: 5000 }.to_string(),
            "timeout after 5000ms"
        );
    }

    #[test]
    fn graph_def_full_round_trip() {
        use std::collections::BTreeMap;
        let graph = GraphDef {
            schema_version: GRAPH_SCHEMA_VERSION,
            id: "flow-1".into(),
            name: "Agent Loop".into(),
            version: "v1".into(),
            nodes: vec![
                NodeInstance {
                    instance_id: "llm-1".into(),
                    node_type: "llm_call".into(),
                    config: json!({"model": "gpt-4o"}),
                    position: Some((100.0, 200.0)),
                },
                NodeInstance {
                    instance_id: "router-1".into(),
                    node_type: "tool_router".into(),
                    config: json!({}),
                    position: None,
                },
            ],
            edges: vec![Edge {
                id: "e1".into(),
                from_node: "llm-1".into(),
                from_port: "output".into(),
                to_node: "router-1".into(),
                to_port: "input".into(),
                condition: Some(EdgeCondition {
                    expression: "finish_reason == 'tool_calls'".into(),
                    label: Some("Has tool calls".into()),
                }),
            }],
            metadata: BTreeMap::new(),
            tool_definitions: {
                let mut t = BTreeMap::new();
                t.insert(
                    "search".to_string(),
                    ToolDef {
                        name: "search".into(),
                        description: "Search the web".into(),
                        parameters: json!({"type": "object", "properties": {"q": {"type": "string"}}}),
                        tool_type: ToolType::Node {
                            target_node_id: "search-node".into(),
                        },
                        metadata: BTreeMap::new(),
                    },
                );
                t
            },
        };

        let json_str = serde_json::to_string_pretty(&graph).unwrap();
        let rt: GraphDef = serde_json::from_str(&json_str).unwrap();
        assert_eq!(graph.id, rt.id);
        assert_eq!(graph.nodes.len(), rt.nodes.len());
        assert_eq!(graph.edges.len(), rt.edges.len());
        assert_eq!(graph.tool_definitions.len(), rt.tool_definitions.len());
        assert!(rt.tool_definitions.contains_key("search"));
    }

    #[test]
    fn flow_version_round_trip() {
        use chrono::Utc;
        use std::collections::BTreeMap;
        let fv = FlowVersion {
            version_id: "abc123".into(),
            flow_id: "flow-1".into(),
            graph: GraphDef {
                schema_version: GRAPH_SCHEMA_VERSION,
                id: "flow-1".into(),
                name: "Test".into(),
                version: "1".into(),
                nodes: vec![],
                edges: vec![],
                metadata: BTreeMap::new(),
                tool_definitions: BTreeMap::new(),
            },
            created_at: Utc::now(),
            created_by: Some("admin".into()),
            message: Some("Initial version".into()),
        };
        let rt = round_trip(&fv);
        assert_eq!(fv.version_id, rt.version_id);
        assert_eq!(fv.flow_id, rt.flow_id);
    }

    #[test]
    fn run_record_defaults() {
        let r = RunRecord::default();
        assert_eq!(r.status, RunStatus::Pending);
        assert!(r.llm_summary.is_none());
        assert!(r.config_hash.is_none());
    }

    #[test]
    fn port_def_required_defaults_true() {
        let json_str = r#"{"name":"input","port_type":"string"}"#;
        let pd: PortDef = serde_json::from_str(json_str).unwrap();
        assert!(pd.required);
        assert_eq!(pd.sensitivity, Sensitivity::None);
    }

    #[test]
    fn llm_chunk_round_trip() {
        let chunks = vec![
            LlmChunk::TextDelta {
                delta: "Hello".into(),
            },
            LlmChunk::ToolCallDelta {
                index: 0,
                id: Some("call_1".into()),
                name: Some("search".into()),
                arguments_delta: r#"{"q":"#.into(),
            },
            LlmChunk::Usage {
                input_tokens: Some(100),
                output_tokens: Some(50),
            },
            LlmChunk::Done {
                finish_reason: "stop".into(),
            },
        ];
        for chunk in &chunks {
            let json_str = serde_json::to_string(chunk).unwrap();
            let rt: LlmChunk = serde_json::from_str(&json_str).unwrap();
            let json_str2 = serde_json::to_string(&rt).unwrap();
            assert_eq!(json_str, json_str2);
        }
    }

    #[test]
    fn graph_schema_version_constant() {
        use std::collections::BTreeMap;
        let graph = GraphDef {
            schema_version: GRAPH_SCHEMA_VERSION,
            id: "f".into(),
            name: "n".into(),
            version: "1".into(),
            nodes: vec![],
            edges: vec![],
            metadata: BTreeMap::new(),
            tool_definitions: BTreeMap::new(),
        };
        let rt = round_trip(&graph);
        assert_eq!(rt.schema_version, 1);
    }
}
