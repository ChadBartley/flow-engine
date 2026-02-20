use crate::types::GraphDef;
use std::collections::{HashMap, HashSet};

/// Validate a `GraphDef` for structural correctness.
///
/// Returns `Ok(())` if the graph is valid, or `Err(Vec<String>)` with a list
/// of human-readable validation errors.
pub fn validate_graph(graph: &GraphDef) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();

    // 1. No duplicate node IDs.
    let mut seen_ids = HashSet::new();
    for node in &graph.nodes {
        if !seen_ids.insert(&node.instance_id) {
            errors.push(format!("Duplicate node ID: {}", node.instance_id));
        }
    }

    // 2. All edge endpoints reference existing nodes.
    let node_ids: HashSet<&str> = graph.nodes.iter().map(|n| n.instance_id.as_str()).collect();

    for edge in &graph.edges {
        if !node_ids.contains(edge.from_node.as_str()) {
            errors.push(format!(
                "Edge {} references unknown source node: {}",
                edge.id, edge.from_node
            ));
        }
        if !node_ids.contains(edge.to_node.as_str()) {
            errors.push(format!(
                "Edge {} references unknown target node: {}",
                edge.id, edge.to_node
            ));
        }
    }

    // 3. No duplicate edge IDs.
    let mut seen_edge_ids = HashSet::new();
    for edge in &graph.edges {
        if !seen_edge_ids.insert(&edge.id) {
            errors.push(format!("Duplicate edge ID: {}", edge.id));
        }
    }

    // 4. At least one entry node (nodes with no incoming non-back-edges).
    let back_edge_ids = find_back_edges(graph);
    let nodes_with_incoming: HashSet<&str> = graph
        .edges
        .iter()
        .filter(|e| !back_edge_ids.contains(&e.id))
        .map(|e| e.to_node.as_str())
        .collect();
    let entry_nodes: Vec<&str> = graph
        .nodes
        .iter()
        .map(|n| n.instance_id.as_str())
        .filter(|id| !nodes_with_incoming.contains(id))
        .collect();

    if entry_nodes.is_empty() && !graph.nodes.is_empty() {
        errors.push("No entry nodes found (all nodes have incoming edges)".to_string());
    }

    // 5. Graph must have at least one node.
    if graph.nodes.is_empty() {
        errors.push("Graph has no nodes".to_string());
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Detect back-edges (cycle edges) via DFS. Returns the set of edge IDs
/// that form back-edges.
fn find_back_edges(graph: &GraphDef) -> HashSet<String> {
    let mut back_edges = HashSet::new();
    let mut visited = HashSet::new();
    let mut in_stack = HashSet::new();

    let mut outgoing: HashMap<&str, Vec<&crate::types::Edge>> = HashMap::new();
    for edge in &graph.edges {
        outgoing
            .entry(edge.from_node.as_str())
            .or_default()
            .push(edge);
    }

    let nodes_with_incoming: HashSet<&str> =
        graph.edges.iter().map(|e| e.to_node.as_str()).collect();
    let start_nodes: Vec<&str> = graph
        .nodes
        .iter()
        .filter(|n| !nodes_with_incoming.contains(n.instance_id.as_str()))
        .map(|n| n.instance_id.as_str())
        .collect();

    if start_nodes.is_empty() {
        for node in &graph.nodes {
            if !visited.contains(node.instance_id.as_str()) {
                dfs(
                    &node.instance_id,
                    &outgoing,
                    &mut visited,
                    &mut in_stack,
                    &mut back_edges,
                );
            }
        }
    } else {
        for start in &start_nodes {
            dfs(
                start,
                &outgoing,
                &mut visited,
                &mut in_stack,
                &mut back_edges,
            );
        }
    }

    back_edges
}

fn dfs(
    node: &str,
    outgoing: &HashMap<&str, Vec<&crate::types::Edge>>,
    visited: &mut HashSet<String>,
    in_stack: &mut HashSet<String>,
    back_edges: &mut HashSet<String>,
) {
    if visited.contains(node) {
        return;
    }
    visited.insert(node.to_string());
    in_stack.insert(node.to_string());

    if let Some(edges) = outgoing.get(node) {
        for edge in edges {
            if in_stack.contains(edge.to_node.as_str()) {
                back_edges.insert(edge.id.clone());
            } else if !visited.contains(edge.to_node.as_str()) {
                dfs(&edge.to_node, outgoing, visited, in_stack, back_edges);
            }
        }
    }

    in_stack.remove(node);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Edge, GraphDef, NodeInstance};

    fn make_node(id: &str) -> NodeInstance {
        NodeInstance {
            instance_id: id.to_string(),
            node_type: "test".to_string(),
            config: serde_json::json!({}),
            position: None,
            tool_access: None,
        }
    }

    fn make_edge(id: &str, from: &str, to: &str) -> Edge {
        Edge {
            id: id.to_string(),
            from_node: from.to_string(),
            from_port: "out".to_string(),
            to_node: to.to_string(),
            to_port: "in".to_string(),
            condition: None,
        }
    }

    #[test]
    fn valid_linear_graph() {
        let graph = GraphDef {
            schema_version: 1,
            id: "flow1".into(),
            name: "Test".into(),
            version: "1.0".into(),
            nodes: vec![make_node("a"), make_node("b"), make_node("c")],
            edges: vec![make_edge("e1", "a", "b"), make_edge("e2", "b", "c")],
            tool_definitions: Default::default(),
            metadata: Default::default(),
        };
        assert!(validate_graph(&graph).is_ok());
    }

    #[test]
    fn duplicate_node_ids() {
        let graph = GraphDef {
            schema_version: 1,
            id: "flow1".into(),
            name: "Test".into(),
            version: "1.0".into(),
            nodes: vec![make_node("a"), make_node("a")],
            edges: vec![],
            tool_definitions: Default::default(),
            metadata: Default::default(),
        };
        let errs = validate_graph(&graph).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("Duplicate node ID: a")));
    }

    #[test]
    fn edge_references_unknown_node() {
        let graph = GraphDef {
            schema_version: 1,
            id: "flow1".into(),
            name: "Test".into(),
            version: "1.0".into(),
            nodes: vec![make_node("a")],
            edges: vec![make_edge("e1", "a", "missing")],
            tool_definitions: Default::default(),
            metadata: Default::default(),
        };
        let errs = validate_graph(&graph).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("unknown target node")));
    }

    #[test]
    fn empty_graph() {
        let graph = GraphDef {
            schema_version: 1,
            id: "flow1".into(),
            name: "Test".into(),
            version: "1.0".into(),
            nodes: vec![],
            edges: vec![],
            tool_definitions: Default::default(),
            metadata: Default::default(),
        };
        let errs = validate_graph(&graph).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("no nodes")));
    }

    #[test]
    fn valid_graph_with_cycle() {
        let graph = GraphDef {
            schema_version: 1,
            id: "flow1".into(),
            name: "Test".into(),
            version: "1.0".into(),
            nodes: vec![make_node("a"), make_node("b")],
            edges: vec![make_edge("e1", "a", "b"), make_edge("e2", "b", "a")],
            tool_definitions: Default::default(),
            metadata: Default::default(),
        };
        // Cycles are allowed (back-edges). Entry node detection accounts for them.
        assert!(validate_graph(&graph).is_ok());
    }
}
