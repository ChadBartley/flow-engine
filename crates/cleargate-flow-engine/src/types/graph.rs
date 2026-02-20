//! Graph schema types — the contract between UI and engine.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

use super::{PortType, Sensitivity, GRAPH_SCHEMA_VERSION};

/// The complete definition of a flow graph.
///
/// **Invariant**: `metadata` and `tool_definitions` use `BTreeMap`, never
/// `HashMap`. HashMap produces nondeterministic JSON key ordering, which
/// breaks content-addressed hashing. This is a correctness invariant
/// enforced by the type system.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub struct GraphDef {
    /// Schema version for forward-compatible deserialization.
    #[serde(default = "default_schema_version")]
    pub schema_version: u16,
    pub id: std::string::String,
    pub name: std::string::String,
    pub version: std::string::String,
    pub nodes: Vec<NodeInstance>,
    pub edges: Vec<Edge>,
    /// Arbitrary metadata. BTreeMap for deterministic serialization.
    #[serde(default)]
    pub metadata: BTreeMap<std::string::String, serde_json::Value>,
    /// Tool definitions available to LLM nodes in this flow.
    /// Keyed by tool name for deterministic serialization.
    /// Included in the content-addressed hash — same flow with different
    /// tools = different version.
    #[serde(default)]
    pub tool_definitions: BTreeMap<std::string::String, ToolDef>,
}

fn default_schema_version() -> u16 {
    GRAPH_SCHEMA_VERSION
}

/// A concrete instance of a node within a graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub struct NodeInstance {
    pub instance_id: std::string::String,
    /// References `NodeMeta.node_type`.
    pub node_type: std::string::String,
    pub config: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<(f64, f64)>,
    /// Per-node tool access control. When set and the `dynamic-tools`
    /// feature is enabled, restricts which tools this node can see and
    /// what permission labels the node holds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_access: Option<NodeToolAccess>,
}

/// Per-node tool access control for the `dynamic-tools` feature.
///
/// When attached to a [`NodeInstance`], restricts which tools the node can
/// see and what permission labels the node holds for accessing
/// permission-gated tools.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub struct NodeToolAccess {
    /// Allowlist of tool names. `None` means all tools are visible
    /// (subject to permission checks).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_tools: Option<BTreeSet<String>>,
    /// Permission labels this node holds. A tool is visible only if its
    /// required permissions are a subset of these.
    #[serde(default)]
    pub granted_permissions: BTreeSet<String>,
}

/// A directed connection between two node ports.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub struct Edge {
    pub id: std::string::String,
    pub from_node: std::string::String,
    pub from_port: std::string::String,
    pub to_node: std::string::String,
    pub to_port: std::string::String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<EdgeCondition>,
}

/// An expression evaluated on the source port's data to decide if the
/// edge is traversed. E.g. `finish_reason == "tool_calls"`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub struct EdgeCondition {
    /// Expression string, e.g. `"finish_reason == 'tool_calls'"`.
    pub expression: std::string::String,
    /// Human-readable label for the UI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<std::string::String>,
}

// ---------------------------------------------------------------------------
// Type registry (UI type hints)
// ---------------------------------------------------------------------------

/// Custom types registered by plugins for UI expansion.
/// `TypeDef.name` matches `PortType::Custom(name)`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub struct TypeDef {
    pub name: std::string::String,
    pub description: std::string::String,
    pub fields: Vec<TypeField>,
    /// Concrete example — helpful for non-technical users.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub example: Option<serde_json::Value>,
}

/// A single field within a `TypeDef`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub struct TypeField {
    pub name: std::string::String,
    pub field_type: PortType,
    pub description: std::string::String,
    #[serde(default = "super::default_true")]
    pub required: bool,
    #[serde(default)]
    pub sensitivity: Sensitivity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub example: Option<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Tool definitions
// ---------------------------------------------------------------------------

/// A tool that LLM nodes can invoke. Defined at the graph level so the
/// flow is self-contained. The `tool_type` determines how the engine
/// dispatches execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub struct ToolDef {
    /// Unique within the graph, e.g. `"search_contacts"`.
    pub name: std::string::String,
    /// Sent to the LLM as the tool description.
    pub description: std::string::String,
    /// JSON Schema — defines what the LLM must provide.
    pub parameters: serde_json::Value,
    pub tool_type: ToolType,
    /// BTreeMap for deterministic serialization.
    #[serde(default)]
    pub metadata: BTreeMap<std::string::String, serde_json::Value>,
    /// Permission labels required to access this tool. Empty means
    /// unrestricted (any node can use it). Only enforced when the
    /// `dynamic-tools` feature is enabled.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub permissions: BTreeSet<String>,
}

/// How the engine dispatches tool execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case", tag = "kind")]
#[non_exhaustive]
pub enum ToolType {
    /// Routed to a node in the graph. The executor resolves the target
    /// node and passes the LLM's tool_call arguments as inputs.
    Node { target_node_id: std::string::String },
    /// Calls an HTTP endpoint directly.
    Http {
        url: std::string::String,
        method: std::string::String,
        #[serde(default)]
        headers: BTreeMap<std::string::String, std::string::String>,
    },
    /// Delegates to an MCP server.
    Mcp {
        server: std::string::String,
        tool_name: std::string::String,
    },
}
