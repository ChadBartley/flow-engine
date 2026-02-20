//! MCP tool call node — dispatches a single tool call to an MCP server.

use std::collections::BTreeMap;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::mcp::types::ToolCallContent;
use crate::node_ctx::NodeCtx;
use crate::traits::NodeHandler;
use crate::types::*;

/// A built-in node that calls a tool on a registered MCP server.
///
/// # Inputs
///
/// - `server` — name of the registered MCP server
/// - `tool_name` — name of the tool to call
/// - `arguments` — JSON object of tool arguments
///
/// # Outputs
///
/// - `result` — the tool call result as JSON
pub struct McpCallNode;

#[async_trait]
impl NodeHandler for McpCallNode {
    fn meta(&self) -> NodeMeta {
        NodeMeta {
            node_type: "mcp_call".into(),
            label: "MCP Tool Call".into(),
            category: "tools".into(),
            inputs: vec![
                PortDef {
                    name: "server".into(),
                    port_type: PortType::String,
                    required: true,
                    default: None,
                    description: Some("Name of the MCP server".into()),
                    sensitivity: Sensitivity::None,
                },
                PortDef {
                    name: "tool_name".into(),
                    port_type: PortType::String,
                    required: true,
                    default: None,
                    description: Some("Name of the tool to call".into()),
                    sensitivity: Sensitivity::None,
                },
                PortDef {
                    name: "arguments".into(),
                    port_type: PortType::Json,
                    required: false,
                    default: Some(json!({})),
                    description: Some("Tool arguments as a JSON object".into()),
                    sensitivity: Sensitivity::None,
                },
            ],
            outputs: vec![PortDef {
                name: "result".into(),
                port_type: PortType::Json,
                required: true,
                default: None,
                description: Some("Tool call result".into()),
                sensitivity: Sensitivity::None,
            }],
            config_schema: json!({}),
            ui: NodeUiHints::default(),
            execution: ExecutionHints::default(),
        }
    }

    async fn run(&self, inputs: Value, _config: &Value, ctx: &NodeCtx) -> Result<Value, NodeError> {
        let server_name = inputs
            .get("server")
            .and_then(|v| v.as_str())
            .ok_or_else(|| NodeError::Validation {
                message: "missing required input: server".into(),
            })?;

        let tool_name = inputs
            .get("tool_name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| NodeError::Validation {
                message: "missing required input: tool_name".into(),
            })?;

        let arguments: BTreeMap<String, Value> = inputs
            .get("arguments")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        let server = ctx
            .mcp_server(server_name)
            .ok_or_else(|| NodeError::Fatal {
                message: format!("MCP server not found: {server_name}"),
            })?;

        let client = server.client().await.map_err(NodeError::from)?;
        let result = client
            .call_tool(tool_name, arguments)
            .await
            .map_err(NodeError::from)?;

        if result.is_error {
            let error_text = result
                .content
                .iter()
                .filter_map(|c| match c {
                    ToolCallContent::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            return Err(NodeError::Fatal {
                message: format!("MCP tool returned error: {error_text}"),
            });
        }

        // Convert content to a JSON value. Single text → string, multiple → array.
        let output = match result.content.len() {
            0 => json!(null),
            1 => match &result.content[0] {
                ToolCallContent::Text { text } => json!(text),
                other => serde_json::to_value(other).unwrap_or(json!(null)),
            },
            _ => serde_json::to_value(&result.content).unwrap_or(json!(null)),
        };

        Ok(json!({ "result": output }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::client::McpClient;
    use crate::mcp::server::{McpServer, McpServerRegistry};
    use crate::mcp::transport::McpTransport;
    use crate::mcp::types::{JsonRpcRequest, JsonRpcResponse};
    use crate::mcp::McpError;
    use crate::node_ctx::TestNodeCtx;
    use async_trait::async_trait;
    use std::sync::Arc;

    // -- Mock transport -----------------------------------------------------

    struct MockMcpTransport {
        responses: std::sync::Mutex<Vec<JsonRpcResponse>>,
    }

    impl MockMcpTransport {
        fn new(responses: Vec<JsonRpcResponse>) -> Self {
            Self {
                responses: std::sync::Mutex::new(responses),
            }
        }
    }

    #[async_trait]
    impl McpTransport for MockMcpTransport {
        async fn send(&self, _req: JsonRpcRequest) -> Result<JsonRpcResponse, McpError> {
            let mut guard = self.responses.lock().unwrap();
            if guard.is_empty() {
                Err(McpError::Connection {
                    message: "no more responses".into(),
                })
            } else {
                Ok(guard.remove(0))
            }
        }

        async fn close(&self) -> Result<(), McpError> {
            Ok(())
        }
    }

    fn ok_resp(id: u64, result: Value) -> JsonRpcResponse {
        JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id: Some(id),
            result: Some(result),
            error: None,
        }
    }

    /// Build a TestNodeCtx with a mock MCP server pre-loaded with canned
    /// responses for tools/call.
    fn build_ctx_with_mock_server(
        server_name: &str,
        call_responses: Vec<JsonRpcResponse>,
    ) -> (NodeCtx, crate::node_ctx::TestNodeCtxInspector) {
        let transport = MockMcpTransport::new(call_responses);
        let client = Arc::new(McpClient::new(Box::new(transport)));
        let server = Arc::new(McpServer::with_client(server_name.to_string(), client));
        let mut registry = McpServerRegistry::new();
        registry.register_server(server_name.to_string(), server);
        TestNodeCtx::builder()
            .mcp_registry(Arc::new(registry))
            .build()
    }

    // -- Tests --------------------------------------------------------------

    #[test]
    fn meta_has_correct_ports() {
        let node = McpCallNode;
        let meta = node.meta();
        assert_eq!(meta.node_type, "mcp_call");
        assert_eq!(meta.inputs.len(), 3);
        assert_eq!(meta.inputs[0].name, "server");
        assert_eq!(meta.inputs[1].name, "tool_name");
        assert_eq!(meta.inputs[2].name, "arguments");
        assert_eq!(meta.outputs.len(), 1);
        assert_eq!(meta.outputs[0].name, "result");
    }

    #[tokio::test]
    async fn run_success_single_text_result() {
        let call_result = json!({
            "content": [{"type": "text", "text": "hello from mcp"}],
            "isError": false
        });
        let (ctx, _inspector) =
            build_ctx_with_mock_server("test-server", vec![ok_resp(1, call_result)]);

        let inputs = json!({
            "server": "test-server",
            "tool_name": "greet",
            "arguments": {"name": "world"}
        });

        let result = McpCallNode
            .run(inputs, &json!({}), &ctx)
            .await
            .expect("should succeed");

        assert_eq!(result["result"], "hello from mcp");
    }

    #[tokio::test]
    async fn run_success_empty_content() {
        let call_result = json!({
            "content": [],
            "isError": false
        });
        let (ctx, _) = build_ctx_with_mock_server("s", vec![ok_resp(1, call_result)]);

        let result = McpCallNode
            .run(
                json!({"server": "s", "tool_name": "noop"}),
                &json!({}),
                &ctx,
            )
            .await
            .unwrap();

        assert_eq!(result["result"], json!(null));
    }

    #[tokio::test]
    async fn run_success_multiple_content_items() {
        let call_result = json!({
            "content": [
                {"type": "text", "text": "line 1"},
                {"type": "text", "text": "line 2"}
            ],
            "isError": false
        });
        let (ctx, _) = build_ctx_with_mock_server("s", vec![ok_resp(1, call_result)]);

        let result = McpCallNode
            .run(
                json!({"server": "s", "tool_name": "multi"}),
                &json!({}),
                &ctx,
            )
            .await
            .unwrap();

        let arr = result["result"].as_array().expect("should be array");
        assert_eq!(arr.len(), 2);
    }

    #[tokio::test]
    async fn run_success_default_arguments() {
        // Omit arguments — should default to empty map.
        let call_result = json!({
            "content": [{"type": "text", "text": "ok"}],
            "isError": false
        });
        let (ctx, _) = build_ctx_with_mock_server("s", vec![ok_resp(1, call_result)]);

        let result = McpCallNode
            .run(
                json!({"server": "s", "tool_name": "no_args"}),
                &json!({}),
                &ctx,
            )
            .await
            .unwrap();

        assert_eq!(result["result"], "ok");
    }

    #[tokio::test]
    async fn run_error_missing_server_input() {
        let (ctx, _) = TestNodeCtx::builder().build();
        let err = McpCallNode
            .run(json!({"tool_name": "foo"}), &json!({}), &ctx)
            .await
            .unwrap_err();

        assert!(matches!(err, NodeError::Validation { .. }));
    }

    #[tokio::test]
    async fn run_error_missing_tool_name_input() {
        let (ctx, _) = TestNodeCtx::builder().build();
        let err = McpCallNode
            .run(json!({"server": "s"}), &json!({}), &ctx)
            .await
            .unwrap_err();

        assert!(matches!(err, NodeError::Validation { .. }));
    }

    #[tokio::test]
    async fn run_error_server_not_found() {
        let (ctx, _) = TestNodeCtx::builder().build();
        let err = McpCallNode
            .run(
                json!({"server": "nonexistent", "tool_name": "foo"}),
                &json!({}),
                &ctx,
            )
            .await
            .unwrap_err();

        match err {
            NodeError::Fatal { message } => {
                assert!(message.contains("not found"), "got: {message}");
            }
            other => panic!("expected Fatal, got: {other}"),
        }
    }

    #[tokio::test]
    async fn run_error_tool_returns_is_error() {
        let call_result = json!({
            "content": [{"type": "text", "text": "something went wrong"}],
            "isError": true
        });
        let (ctx, _) = build_ctx_with_mock_server("s", vec![ok_resp(1, call_result)]);

        let err = McpCallNode
            .run(
                json!({"server": "s", "tool_name": "fail"}),
                &json!({}),
                &ctx,
            )
            .await
            .unwrap_err();

        match err {
            NodeError::Fatal { message } => {
                assert!(message.contains("something went wrong"), "got: {message}");
            }
            other => panic!("expected Fatal, got: {other}"),
        }
    }

    #[tokio::test]
    async fn run_error_transport_failure() {
        // Empty transport → connection error on call
        let (ctx, _) = build_ctx_with_mock_server("s", vec![]);

        let err = McpCallNode
            .run(
                json!({"server": "s", "tool_name": "anything"}),
                &json!({}),
                &ctx,
            )
            .await
            .unwrap_err();

        // Connection errors map to Retryable
        assert!(matches!(err, NodeError::Retryable { .. }));
    }

    #[tokio::test]
    async fn run_error_json_rpc_error_from_server() {
        let error_response = JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id: Some(1),
            result: None,
            error: Some(crate::mcp::types::JsonRpcError {
                code: -32601,
                message: "method not found".into(),
                data: None,
            }),
        };
        let (ctx, _) = build_ctx_with_mock_server("s", vec![error_response]);

        let err = McpCallNode
            .run(
                json!({"server": "s", "tool_name": "bad_method"}),
                &json!({}),
                &ctx,
            )
            .await
            .unwrap_err();

        // Protocol errors map to Fatal
        assert!(matches!(err, NodeError::Fatal { .. }));
    }
}
