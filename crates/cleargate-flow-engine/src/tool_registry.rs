//! Dynamic tool registry — the single source of truth for available tools.
//!
//! The [`ToolRegistry`] manages tool definitions at runtime with support for
//! concurrent read access, runtime registration/removal, and per-node
//! filtering based on allowlists and permission labels.
//!
//! # Feature gate
//!
//! This module is always compiled, but permission-based filtering and
//! per-node scoping require the `dynamic-tools` feature.
//!
//! # Example
//!
//! ```ignore
//! let registry = ToolRegistry::new();
//! registry.register(tool_def, BTreeSet::new());
//! let all = registry.snapshot();
//! let filtered = registry.snapshot_filtered(
//!     Some(&allowed),
//!     &caller_perms,
//! );
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use parking_lot::RwLock;

use crate::types::ToolDef;

/// A permission label required to access a tool.
pub type ToolPermissionLabel = String;

/// Thread-safe registry of tool definitions with runtime mutability.
///
/// Cheaply cloneable (inner state is `Arc`-wrapped). Multiple clones share
/// the same underlying registry, so a tool registered through one handle
/// is immediately visible through all others.
#[derive(Clone)]
pub struct ToolRegistry {
    inner: Arc<RwLock<ToolRegistryInner>>,
}

struct ToolRegistryInner {
    tools: BTreeMap<String, ToolEntry>,
}

struct ToolEntry {
    def: ToolDef,
    permissions: BTreeSet<ToolPermissionLabel>,
}

impl ToolRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(ToolRegistryInner {
                tools: BTreeMap::new(),
            })),
        }
    }

    /// Create a registry pre-populated with tools.
    ///
    /// Each tool's `permissions` field is used as its required permission set.
    pub fn from_tools(tools: impl IntoIterator<Item = ToolDef>) -> Self {
        let registry = Self::new();
        for tool in tools {
            let perms = tool.permissions.clone();
            registry.register(tool, perms);
        }
        registry
    }

    /// Register a tool, replacing any existing tool with the same name.
    pub fn register(&self, def: ToolDef, permissions: BTreeSet<ToolPermissionLabel>) {
        let mut inner = self.inner.write();
        inner
            .tools
            .insert(def.name.clone(), ToolEntry { def, permissions });
    }

    /// Remove a tool by name. Returns `true` if the tool existed.
    pub fn remove(&self, name: &str) -> bool {
        let mut inner = self.inner.write();
        inner.tools.remove(name).is_some()
    }

    /// Look up a single tool by name.
    pub fn get(&self, name: &str) -> Option<ToolDef> {
        let inner = self.inner.read();
        inner.tools.get(name).map(|e| e.def.clone())
    }

    /// Snapshot all tools (unfiltered).
    pub fn snapshot(&self) -> Vec<ToolDef> {
        let inner = self.inner.read();
        inner.tools.values().map(|e| e.def.clone()).collect()
    }

    /// Snapshot tools visible to a caller with the given permissions and allowlist.
    ///
    /// A tool is included if:
    /// 1. `allowed` is `None` (no allowlist) OR the tool's name is in `allowed`
    /// 2. The tool's required permissions are a subset of `caller_perms`
    ///    (tools with no required permissions are always visible)
    pub fn snapshot_filtered(
        &self,
        allowed: Option<&BTreeSet<String>>,
        caller_perms: &BTreeSet<ToolPermissionLabel>,
    ) -> Vec<ToolDef> {
        let inner = self.inner.read();
        inner
            .tools
            .values()
            .filter(|entry| {
                // Check allowlist
                let in_allowlist = match allowed {
                    Some(set) => set.contains(&entry.def.name),
                    None => true,
                };
                // Check permissions
                let has_perms = entry.permissions.is_subset(caller_perms);
                in_allowlist && has_perms
            })
            .map(|e| e.def.clone())
            .collect()
    }

    /// Returns `true` if no tools are registered.
    pub fn is_empty(&self) -> bool {
        self.inner.read().tools.is_empty()
    }

    /// Number of registered tools.
    pub fn len(&self) -> usize {
        self.inner.read().tools.len()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;

    use super::*;
    use crate::types::ToolType;

    fn make_tool(name: &str) -> ToolDef {
        ToolDef {
            name: name.to_string(),
            description: format!("Tool {name}"),
            parameters: json!({"type": "object"}),
            tool_type: ToolType::Node {
                target_node_id: format!("{name}-node"),
            },
            metadata: BTreeMap::new(),
            permissions: BTreeSet::new(),
        }
    }

    #[test]
    fn register_and_snapshot() {
        let reg = ToolRegistry::new();
        reg.register(make_tool("search"), BTreeSet::new());
        reg.register(make_tool("calc"), BTreeSet::new());

        let snap = reg.snapshot();
        assert_eq!(snap.len(), 2);
        assert_eq!(snap[0].name, "calc"); // BTreeMap ordering
        assert_eq!(snap[1].name, "search");
    }

    #[test]
    fn remove_tool() {
        let reg = ToolRegistry::new();
        reg.register(make_tool("search"), BTreeSet::new());
        assert_eq!(reg.len(), 1);

        assert!(reg.remove("search"));
        assert!(reg.is_empty());
        assert!(!reg.remove("search")); // already gone
    }

    #[test]
    fn get_tool() {
        let reg = ToolRegistry::new();
        reg.register(make_tool("search"), BTreeSet::new());

        assert!(reg.get("search").is_some());
        assert_eq!(reg.get("search").unwrap().name, "search");
        assert!(reg.get("missing").is_none());
    }

    #[test]
    fn filter_by_allowed_tools() {
        let reg = ToolRegistry::new();
        reg.register(make_tool("search"), BTreeSet::new());
        reg.register(make_tool("calc"), BTreeSet::new());
        reg.register(make_tool("email"), BTreeSet::new());

        let allowed: BTreeSet<String> = ["search", "calc"].iter().map(|s| s.to_string()).collect();
        let filtered = reg.snapshot_filtered(Some(&allowed), &BTreeSet::new());
        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].name, "calc");
        assert_eq!(filtered[1].name, "search");
    }

    #[test]
    fn filter_by_permissions() {
        let reg = ToolRegistry::new();
        reg.register(make_tool("public"), BTreeSet::new());
        reg.register(
            make_tool("admin_tool"),
            BTreeSet::from(["admin".to_string()]),
        );
        reg.register(
            make_tool("multi_perm"),
            BTreeSet::from(["admin".to_string(), "write".to_string()]),
        );

        // Caller with no perms sees only unrestricted tools
        let filtered = reg.snapshot_filtered(None, &BTreeSet::new());
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "public");

        // Caller with admin sees public + admin_tool
        let admin_perms = BTreeSet::from(["admin".to_string()]);
        let filtered = reg.snapshot_filtered(None, &admin_perms);
        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].name, "admin_tool");
        assert_eq!(filtered[1].name, "public");

        // Caller with admin+write sees all
        let all_perms = BTreeSet::from(["admin".to_string(), "write".to_string()]);
        let filtered = reg.snapshot_filtered(None, &all_perms);
        assert_eq!(filtered.len(), 3);
    }

    #[test]
    fn empty_tool_perms_always_visible() {
        let reg = ToolRegistry::new();
        reg.register(make_tool("open"), BTreeSet::new());

        // Visible even with no caller perms
        let filtered = reg.snapshot_filtered(None, &BTreeSet::new());
        assert_eq!(filtered.len(), 1);

        // Visible with arbitrary caller perms
        let perms = BTreeSet::from(["anything".to_string()]);
        let filtered = reg.snapshot_filtered(None, &perms);
        assert_eq!(filtered.len(), 1);
    }

    #[test]
    fn empty_caller_perms_only_unrestricted() {
        let reg = ToolRegistry::new();
        reg.register(make_tool("open"), BTreeSet::new());
        reg.register(
            make_tool("restricted"),
            BTreeSet::from(["secret".to_string()]),
        );

        let filtered = reg.snapshot_filtered(None, &BTreeSet::new());
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "open");
    }

    #[test]
    fn register_replaces_existing() {
        let reg = ToolRegistry::new();
        let mut tool = make_tool("search");
        tool.description = "v1".to_string();
        reg.register(tool, BTreeSet::new());

        let mut tool2 = make_tool("search");
        tool2.description = "v2".to_string();
        reg.register(tool2, BTreeSet::from(["admin".to_string()]));

        assert_eq!(reg.len(), 1);
        let snap = reg.snapshot();
        assert_eq!(snap[0].description, "v2");

        // Replaced tool now has permission requirement
        let filtered = reg.snapshot_filtered(None, &BTreeSet::new());
        assert_eq!(filtered.len(), 0);
    }

    #[test]
    fn from_tools_constructor() {
        let tools = vec![make_tool("a"), make_tool("b"), make_tool("c")];
        let reg = ToolRegistry::from_tools(tools);
        assert_eq!(reg.len(), 3);
    }

    #[test]
    fn clone_shares_state() {
        let reg = ToolRegistry::new();
        let reg2 = reg.clone();

        reg.register(make_tool("search"), BTreeSet::new());
        assert_eq!(reg2.len(), 1); // visible through clone
    }
}
