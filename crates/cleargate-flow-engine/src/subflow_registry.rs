//! Registry for named sub-flow graph definitions.
//!
//! The [`SubflowRegistry`] stores [`GraphDef`] instances keyed by name,
//! allowing the [`SubflowNode`](crate::nodes::SubflowNode) to resolve
//! sub-flows at execution time. Register sub-flows via
//! [`EngineBuilder::register_subflow`](crate::EngineBuilder::register_subflow).
//!
//! Thread-safe and cheaply cloneable — all instances share the same
//! underlying storage via `Arc`.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;

use crate::types::GraphDef;

/// Thread-safe registry of named sub-flow graph definitions.
///
/// Sub-flows registered here can be referenced by name in a
/// `SubflowNode` config: `{ "subflow": "<name>" }`.
#[derive(Clone, Default)]
pub struct SubflowRegistry {
    inner: Arc<RwLock<HashMap<String, GraphDef>>>,
}

impl SubflowRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a named sub-flow. Overwrites any previous definition
    /// with the same name.
    pub fn register(&self, name: impl Into<String>, graph: GraphDef) {
        let mut guard = self.inner.write();
        guard.insert(name.into(), graph);
    }

    /// Look up a sub-flow by name.
    pub fn get(&self, name: &str) -> Option<GraphDef> {
        let guard = self.inner.read();
        guard.get(name).cloned()
    }

    /// Returns the number of registered sub-flows.
    pub fn len(&self) -> usize {
        self.inner.read().len()
    }

    /// Returns `true` if no sub-flows are registered.
    pub fn is_empty(&self) -> bool {
        self.inner.read().is_empty()
    }
}

impl std::fmt::Debug for SubflowRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let guard = self.inner.read();
        f.debug_struct("SubflowRegistry")
            .field("count", &guard.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::GraphDef;
    use std::collections::BTreeMap;

    fn test_graph(id: &str) -> GraphDef {
        GraphDef {
            schema_version: 1,
            id: id.to_string(),
            name: id.to_string(),
            version: "1.0".to_string(),
            nodes: vec![],
            edges: vec![],
            metadata: BTreeMap::new(),
            tool_definitions: BTreeMap::new(),
        }
    }

    #[test]
    fn register_and_get() {
        let registry = SubflowRegistry::new();
        assert!(registry.is_empty());

        registry.register("my-flow", test_graph("flow-1"));
        assert_eq!(registry.len(), 1);

        let graph = registry
            .get("my-flow")
            .expect("should find registered flow");
        assert_eq!(graph.id, "flow-1");
    }

    #[test]
    fn get_missing_returns_none() {
        let registry = SubflowRegistry::new();
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn overwrite_existing() {
        let registry = SubflowRegistry::new();
        registry.register("flow", test_graph("v1"));
        registry.register("flow", test_graph("v2"));

        let graph = registry.get("flow").unwrap();
        assert_eq!(graph.id, "v2");
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn clone_shares_state() {
        let registry = SubflowRegistry::new();
        let clone = registry.clone();

        registry.register("flow", test_graph("shared"));
        assert!(
            clone.get("flow").is_some(),
            "clone should see registered flows"
        );
    }
}
