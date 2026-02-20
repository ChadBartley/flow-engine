//! Tool discovery — converts MCP tool listings to FlowEngine `ToolDef` values.

use std::collections::BTreeMap;

use super::client::McpClient;
use super::types::McpTool;
use super::McpError;
use crate::types::{ToolDef, ToolType};

/// Discover tools from an MCP server and convert them to `ToolDef` values.
///
/// Each discovered tool gets `ToolType::Mcp` with the given `server_name` so
/// the engine can route calls back to the correct MCP server at execution time.
pub async fn discover_tools(
    server_name: &str,
    client: &McpClient,
) -> Result<Vec<ToolDef>, McpError> {
    let mcp_tools = client.list_tools().await?;
    Ok(mcp_tools
        .into_iter()
        .map(|t| mcp_tool_to_tool_def(server_name, t))
        .collect())
}

/// Convert a single `McpTool` to a `ToolDef`.
fn mcp_tool_to_tool_def(server_name: &str, tool: McpTool) -> ToolDef {
    ToolDef {
        name: tool.name.clone(),
        description: tool.description,
        parameters: tool.input_schema,
        tool_type: ToolType::Mcp {
            server: server_name.to_string(),
            tool_name: tool.name,
        },
        metadata: BTreeMap::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_mcp_tool_to_tool_def() {
        let mcp_tool = McpTool {
            name: "read_file".into(),
            description: "Read a file from disk".into(),
            input_schema: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
        };

        let def = mcp_tool_to_tool_def("filesystem", mcp_tool);
        assert_eq!(def.name, "read_file");
        assert_eq!(def.description, "Read a file from disk");
        match &def.tool_type {
            ToolType::Mcp { server, tool_name } => {
                assert_eq!(server, "filesystem");
                assert_eq!(tool_name, "read_file");
            }
            other => panic!("expected Mcp tool type, got: {other:?}"),
        }
    }
}
