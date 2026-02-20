//! MCP protocol client — wraps a transport with protocol-level operations.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;

use super::transport::McpTransport;
use super::types::*;
use super::McpError;

/// MCP protocol client. Wraps a transport and provides typed protocol methods.
pub struct McpClient {
    transport: Box<dyn McpTransport>,
    next_id: AtomicU64,
    initialized: std::sync::atomic::AtomicBool,
}

impl McpClient {
    /// Create a new MCP client wrapping the given transport.
    pub fn new(transport: Box<dyn McpTransport>) -> Self {
        Self {
            transport,
            next_id: AtomicU64::new(1),
            initialized: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Perform the MCP initialization handshake.
    pub async fn initialize(&self) -> Result<InitializeResult, McpError> {
        let params = InitializeParams {
            protocol_version: "2024-11-05".into(),
            capabilities: ClientCapabilities {},
            client_info: ClientInfo {
                name: "cleargate-flow-engine".into(),
                version: env!("CARGO_PKG_VERSION").into(),
            },
        };

        let result_value = self
            .call_method("initialize", Some(serde_json::to_value(&params).unwrap()))
            .await?;

        let init_result: InitializeResult =
            serde_json::from_value(result_value).map_err(|e| McpError::Protocol {
                message: format!("failed to parse initialize result: {e}"),
            })?;

        // Send initialized notification (no response expected, but we use our
        // transport's send and ignore the timeout/error for the notification).
        let notification = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: self.next_id(),
            method: "notifications/initialized".into(),
            params: None,
        };
        // Best-effort — some transports may not support notifications cleanly.
        let _ = self.transport.send(notification).await;

        self.initialized
            .store(true, std::sync::atomic::Ordering::Relaxed);
        Ok(init_result)
    }

    /// List all tools from the server, following pagination cursors.
    pub async fn list_tools(&self) -> Result<Vec<McpTool>, McpError> {
        let mut all_tools = Vec::new();
        let mut cursor: Option<String> = None;

        loop {
            let params = cursor.as_ref().map(|c| serde_json::json!({"cursor": c}));

            let result_value = self.call_method("tools/list", params).await?;
            let page: ToolsListResult =
                serde_json::from_value(result_value).map_err(|e| McpError::Protocol {
                    message: format!("failed to parse tools/list result: {e}"),
                })?;

            all_tools.extend(page.tools);

            match page.next_cursor {
                Some(c) if !c.is_empty() => cursor = Some(c),
                _ => break,
            }
        }

        Ok(all_tools)
    }

    /// Call a tool on the MCP server.
    pub async fn call_tool(
        &self,
        name: &str,
        arguments: BTreeMap<String, Value>,
    ) -> Result<ToolsCallResult, McpError> {
        let params = ToolsCallParams {
            name: name.to_string(),
            arguments,
        };

        let result_value = self
            .call_method("tools/call", Some(serde_json::to_value(&params).unwrap()))
            .await?;

        serde_json::from_value(result_value).map_err(|e| McpError::Protocol {
            message: format!("failed to parse tools/call result: {e}"),
        })
    }

    /// Close the underlying transport.
    pub async fn close(&self) -> Result<(), McpError> {
        self.transport.close().await
    }

    /// Low-level JSON-RPC method call with ID generation and error handling.
    async fn call_method(&self, method: &str, params: Option<Value>) -> Result<Value, McpError> {
        let request = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: self.next_id(),
            method: method.to_string(),
            params,
        };

        let response = self.transport.send(request).await?;

        if let Some(error) = response.error {
            return Err(McpError::Protocol {
                message: format!("JSON-RPC error {}: {}", error.code, error.message),
            });
        }

        response.result.ok_or_else(|| McpError::Protocol {
            message: "JSON-RPC response has neither result nor error".into(),
        })
    }

    fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::transport::McpTransport;
    use async_trait::async_trait;
    use std::sync::Mutex;

    /// Mock transport that returns canned responses.
    struct MockTransport {
        responses: Mutex<Vec<JsonRpcResponse>>,
    }

    impl MockTransport {
        fn new(responses: Vec<JsonRpcResponse>) -> Self {
            Self {
                responses: Mutex::new(responses),
            }
        }
    }

    #[async_trait]
    impl McpTransport for MockTransport {
        async fn send(&self, _request: JsonRpcRequest) -> Result<JsonRpcResponse, McpError> {
            let mut guard = self.responses.lock().unwrap();
            if guard.is_empty() {
                Err(McpError::Connection {
                    message: "no more canned responses".into(),
                })
            } else {
                Ok(guard.remove(0))
            }
        }

        async fn close(&self) -> Result<(), McpError> {
            Ok(())
        }
    }

    fn ok_response(id: u64, result: Value) -> JsonRpcResponse {
        JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id: Some(id),
            result: Some(result),
            error: None,
        }
    }

    #[tokio::test]
    async fn test_initialize() {
        let init_result = serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {"tools": {"listChanged": false}},
            "serverInfo": {"name": "test-server", "version": "1.0"}
        });
        // Two responses: one for initialize, one for notifications/initialized
        let transport = MockTransport::new(vec![
            ok_response(1, init_result),
            ok_response(2, serde_json::json!(null)),
        ]);

        let client = McpClient::new(Box::new(transport));
        let result = client.initialize().await.unwrap();
        assert_eq!(result.protocol_version, "2024-11-05");
        assert_eq!(result.server_info.name, "test-server");
    }

    #[tokio::test]
    async fn test_list_tools_single_page() {
        let list_result = serde_json::json!({
            "tools": [
                {"name": "read_file", "description": "Read a file", "inputSchema": {"type": "object"}},
                {"name": "write_file", "description": "Write a file", "inputSchema": {"type": "object"}}
            ]
        });
        let transport = MockTransport::new(vec![ok_response(1, list_result)]);
        let client = McpClient::new(Box::new(transport));

        let tools = client.list_tools().await.unwrap();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, "read_file");
        assert_eq!(tools[1].name, "write_file");
    }

    #[tokio::test]
    async fn test_list_tools_paginated() {
        let page1 = serde_json::json!({
            "tools": [{"name": "tool_a", "description": "A"}],
            "nextCursor": "page2"
        });
        let page2 = serde_json::json!({
            "tools": [{"name": "tool_b", "description": "B"}]
        });
        let transport = MockTransport::new(vec![ok_response(1, page1), ok_response(2, page2)]);
        let client = McpClient::new(Box::new(transport));

        let tools = client.list_tools().await.unwrap();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, "tool_a");
        assert_eq!(tools[1].name, "tool_b");
    }

    #[tokio::test]
    async fn test_call_tool() {
        let call_result = serde_json::json!({
            "content": [{"type": "text", "text": "file contents here"}],
            "isError": false
        });
        let transport = MockTransport::new(vec![ok_response(1, call_result)]);
        let client = McpClient::new(Box::new(transport));

        let mut args = BTreeMap::new();
        args.insert("path".into(), serde_json::json!("/tmp/test.txt"));

        let result = client.call_tool("read_file", args).await.unwrap();
        assert!(!result.is_error);
        assert_eq!(result.content.len(), 1);
    }

    #[tokio::test]
    async fn test_json_rpc_error() {
        let error_response = JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id: Some(1),
            result: None,
            error: Some(super::super::types::JsonRpcError {
                code: -32601,
                message: "method not found".into(),
                data: None,
            }),
        };
        let transport = MockTransport::new(vec![error_response]);
        let client = McpClient::new(Box::new(transport));

        let err = client.list_tools().await.unwrap_err();
        match err {
            McpError::Protocol { message } => {
                assert!(message.contains("-32601"));
                assert!(message.contains("method not found"));
            }
            other => panic!("expected Protocol error, got: {other:?}"),
        }
    }
}
