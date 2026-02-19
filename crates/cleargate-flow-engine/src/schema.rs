//! JSON Schema generation for all public flow engine types.
//!
//! This module is only available when the `schemars` feature is enabled.

use schemars::{schema_for, JsonSchema};
use serde_json::Value;
use std::collections::BTreeMap;

/// Generate JSON Schema for a single type.
pub fn schema_of<T: JsonSchema>() -> Value {
    serde_json::to_value(schema_for!(T)).expect("schema serialization")
}

/// Generate all flow engine type schemas, keyed by type name.
pub fn all_schemas() -> BTreeMap<String, Value> {
    let mut schemas = BTreeMap::new();

    // types.rs
    schemas.insert("PortType".into(), schema_of::<crate::PortType>());
    schemas.insert("Sensitivity".into(), schema_of::<crate::Sensitivity>());
    schemas.insert("PortDef".into(), schema_of::<crate::PortDef>());
    schemas.insert("NodeMeta".into(), schema_of::<crate::NodeMeta>());
    schemas.insert("NodeUiHints".into(), schema_of::<crate::NodeUiHints>());
    schemas.insert(
        "ExecutionHints".into(),
        schema_of::<crate::ExecutionHints>(),
    );
    schemas.insert("RetryPolicy".into(), schema_of::<crate::RetryPolicy>());
    schemas.insert("GraphDef".into(), schema_of::<crate::GraphDef>());
    schemas.insert("NodeInstance".into(), schema_of::<crate::NodeInstance>());
    schemas.insert("Edge".into(), schema_of::<crate::Edge>());
    schemas.insert("EdgeCondition".into(), schema_of::<crate::EdgeCondition>());
    schemas.insert("TypeDef".into(), schema_of::<crate::TypeDef>());
    schemas.insert("TypeField".into(), schema_of::<crate::TypeField>());
    schemas.insert("ToolDef".into(), schema_of::<crate::ToolDef>());
    schemas.insert("ToolType".into(), schema_of::<crate::ToolType>());
    schemas.insert("LlmRequest".into(), schema_of::<crate::LlmRequest>());
    schemas.insert("LlmResponse".into(), schema_of::<crate::LlmResponse>());
    schemas.insert("LlmToolCall".into(), schema_of::<crate::LlmToolCall>());
    schemas.insert("LlmCost".into(), schema_of::<crate::LlmCost>());
    schemas.insert("LlmRunSummary".into(), schema_of::<crate::LlmRunSummary>());
    schemas.insert(
        "LlmInvocationRecord".into(),
        schema_of::<crate::LlmInvocationRecord>(),
    );
    schemas.insert("FlowVersion".into(), schema_of::<crate::FlowVersion>());
    schemas.insert("FlowHead".into(), schema_of::<crate::FlowHead>());
    schemas.insert("RunRecord".into(), schema_of::<crate::RunRecord>());
    schemas.insert("RunStatus".into(), schema_of::<crate::RunStatus>());
    schemas.insert("TriggerEvent".into(), schema_of::<crate::TriggerEvent>());
    schemas.insert("TriggerSource".into(), schema_of::<crate::TriggerSource>());
    schemas.insert("NodeError".into(), schema_of::<crate::NodeError>());

    // write_event.rs
    schemas.insert("WriteEvent".into(), schema_of::<crate::WriteEvent>());

    // traits.rs
    schemas.insert("NodeRunResult".into(), schema_of::<crate::NodeRunResult>());
    schemas.insert("EdgeRunResult".into(), schema_of::<crate::EdgeRunResult>());
    schemas.insert("NodeRunDetail".into(), schema_of::<crate::NodeRunDetail>());
    schemas.insert(
        "NodeAttemptDetail".into(),
        schema_of::<crate::NodeAttemptDetail>(),
    );
    schemas.insert(
        "CustomEventRecord".into(),
        schema_of::<crate::CustomEventRecord>(),
    );

    schemas
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_of_produces_valid_json_schema() {
        let schema = schema_of::<crate::GraphDef>();
        assert!(schema.is_object());
        let obj = schema.as_object().unwrap();
        assert!(obj.contains_key("title") || obj.contains_key("type") || obj.contains_key("$ref"));
    }

    #[test]
    fn all_schemas_non_empty() {
        let schemas = all_schemas();
        assert!(
            schemas.len() >= 30,
            "Expected at least 30 schemas, got {}",
            schemas.len()
        );
        for (name, schema) in &schemas {
            assert!(schema.is_object(), "Schema for {name} is not an object");
        }
    }
}
