//! MCP server configuration, lazy connection management, and registry.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use super::client::McpClient;
use super::discovery::discover_tools;
use super::transport::{SseTransport, StdioTransport};
use super::McpError;
use crate::types::ToolDef;

// ---------------------------------------------------------------------------
// Server configuration
// ---------------------------------------------------------------------------

/// Configuration for connecting to an MCP server.
#[derive(Debug, Clone)]
pub enum McpServerConfig {
    /// Spawn a child process and communicate over stdin/stdout.
    Stdio {
        command: String,
        args: Vec<String>,
        #[allow(clippy::doc_markdown)]
        env: HashMap<String, String>,
    },
    /// Connect to an HTTP SSE endpoint.
    Sse {
        url: String,
        headers: HashMap<String, String>,
    },
}

// ---------------------------------------------------------------------------
// Server (lazy connection)
// ---------------------------------------------------------------------------

/// A named MCP server with lazy connection management.
///
/// The connection is established on first use and cached for subsequent calls.
pub struct McpServer {
    name: String,
    config: McpServerConfig,
    client: RwLock<Option<Arc<McpClient>>>,
}

impl McpServer {
    /// Create a new server entry (not yet connected).
    pub fn new(name: String, config: McpServerConfig) -> Self {
        Self {
            name,
            config,
            client: RwLock::new(None),
        }
    }

    /// Create a server with a pre-connected client (for testing).
    #[cfg(any(test, feature = "test-support"))]
    pub fn with_client(name: String, client: Arc<McpClient>) -> Self {
        Self {
            name,
            config: McpServerConfig::Stdio {
                command: String::new(),
                args: Vec::new(),
                env: HashMap::new(),
            },
            client: RwLock::new(Some(client)),
        }
    }

    /// The server's registered name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Get or create the MCP client, connecting lazily on first call.
    pub async fn client(&self) -> Result<Arc<McpClient>, McpError> {
        // Fast path: already connected.
        {
            let guard = self.client.read().await;
            if let Some(c) = guard.as_ref() {
                return Ok(Arc::clone(c));
            }
        }

        // Slow path: connect.
        let mut guard = self.client.write().await;
        // Double-check after acquiring write lock.
        if let Some(c) = guard.as_ref() {
            return Ok(Arc::clone(c));
        }

        let transport: Box<dyn super::transport::McpTransport> = match &self.config {
            McpServerConfig::Stdio { command, args, env } => {
                Box::new(StdioTransport::spawn(command, args, env).await?)
            }
            McpServerConfig::Sse { url, headers } => {
                Box::new(SseTransport::new(url.clone(), headers.clone()))
            }
        };

        let client = Arc::new(McpClient::new(transport));
        client.initialize().await?;

        *guard = Some(Arc::clone(&client));
        Ok(client)
    }
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

/// Registry of named MCP servers. Used by the engine to route tool calls.
pub struct McpServerRegistry {
    servers: HashMap<String, Arc<McpServer>>,
}

impl McpServerRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            servers: HashMap::new(),
        }
    }

    /// Register a server by name.
    pub fn register(&mut self, name: String, config: McpServerConfig) {
        self.servers
            .insert(name.clone(), Arc::new(McpServer::new(name, config)));
    }

    /// Register a pre-built server (for testing with mock transports).
    #[cfg(any(test, feature = "test-support"))]
    pub fn register_server(&mut self, name: String, server: Arc<McpServer>) {
        self.servers.insert(name, server);
    }

    /// Look up a server by name.
    pub fn get(&self, name: &str) -> Option<Arc<McpServer>> {
        self.servers.get(name).cloned()
    }

    /// Discover tools from all registered servers.
    ///
    /// Servers that fail to connect are logged and skipped — partial discovery
    /// is better than failing the entire flow.
    pub async fn discover_all_tools(&self) -> Result<Vec<ToolDef>, McpError> {
        let mut all_tools = Vec::new();

        for (name, server) in &self.servers {
            match server.client().await {
                Ok(client) => match discover_tools(name, &client).await {
                    Ok(tools) => all_tools.extend(tools),
                    Err(e) => {
                        tracing::warn!(
                            server = %name,
                            error = %e,
                            "failed to discover tools from MCP server"
                        );
                    }
                },
                Err(e) => {
                    tracing::warn!(
                        server = %name,
                        error = %e,
                        "failed to connect to MCP server"
                    );
                }
            }
        }

        Ok(all_tools)
    }

    /// Returns `true` if no servers are registered.
    pub fn is_empty(&self) -> bool {
        self.servers.is_empty()
    }
}

impl Default for McpServerRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::client::McpClient;
    use crate::mcp::transport::McpTransport;
    use crate::mcp::types::{JsonRpcRequest, JsonRpcResponse};
    use crate::mcp::McpError;
    use async_trait::async_trait;

    struct MockTransport {
        responses: std::sync::Mutex<Vec<JsonRpcResponse>>,
    }

    impl MockTransport {
        fn new(responses: Vec<JsonRpcResponse>) -> Self {
            Self {
                responses: std::sync::Mutex::new(responses),
            }
        }
    }

    #[async_trait]
    impl McpTransport for MockTransport {
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

    fn ok_resp(id: u64, result: serde_json::Value) -> JsonRpcResponse {
        JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id: Some(id),
            result: Some(result),
            error: None,
        }
    }

    fn mock_init_responses() -> Vec<JsonRpcResponse> {
        vec![
            ok_resp(
                1,
                serde_json::json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "serverInfo": {"name": "mock", "version": "1.0"}
                }),
            ),
            ok_resp(2, serde_json::json!(null)), // notifications/initialized
        ]
    }

    // -- Registry tests -----------------------------------------------------

    #[test]
    fn registry_new_is_empty() {
        let reg = McpServerRegistry::new();
        assert!(reg.is_empty());
        assert!(reg.get("anything").is_none());
    }

    #[test]
    fn registry_register_and_get() {
        let mut reg = McpServerRegistry::new();
        reg.register(
            "fs".to_string(),
            McpServerConfig::Stdio {
                command: "echo".into(),
                args: vec![],
                env: HashMap::new(),
            },
        );
        assert!(!reg.is_empty());
        assert!(reg.get("fs").is_some());
        assert!(reg.get("other").is_none());
    }

    #[test]
    fn registry_register_server_with_pre_built() {
        let client = Arc::new(McpClient::new(Box::new(MockTransport::new(vec![]))));
        let server = Arc::new(McpServer::with_client("mock".to_string(), client));
        let mut reg = McpServerRegistry::new();
        reg.register_server("mock".to_string(), server);
        assert!(reg.get("mock").is_some());
    }

    // -- McpServer tests ----------------------------------------------------

    #[tokio::test]
    async fn server_with_client_returns_client_immediately() {
        let transport = MockTransport::new(vec![]);
        let client = Arc::new(McpClient::new(Box::new(transport)));
        let server = McpServer::with_client("test".to_string(), client);

        // Should return the injected client without trying to connect
        let c = server.client().await;
        assert!(c.is_ok());
    }

    #[test]
    fn server_name() {
        let server = McpServer::new(
            "my-server".to_string(),
            McpServerConfig::Sse {
                url: "http://localhost:8080".into(),
                headers: HashMap::new(),
            },
        );
        assert_eq!(server.name(), "my-server");
    }

    // -- Discovery integration via registry ---------------------------------

    #[tokio::test]
    async fn discover_all_tools_from_mock_server() {
        let mut responses = mock_init_responses();
        // tools/list response
        responses.push(ok_resp(
            3,
            serde_json::json!({
                "tools": [
                    {"name": "read_file", "description": "Read a file", "inputSchema": {"type": "object"}},
                    {"name": "write_file", "description": "Write a file", "inputSchema": {"type": "object"}}
                ]
            }),
        ));

        let transport = MockTransport::new(responses);
        let client = Arc::new(McpClient::new(Box::new(transport)));
        // Pre-initialize so discovery doesn't try to init again
        client.initialize().await.unwrap();

        let server = Arc::new(McpServer::with_client("fs".to_string(), client));
        let mut reg = McpServerRegistry::new();
        reg.register_server("fs".to_string(), server);

        let tools = reg.discover_all_tools().await.unwrap();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, "read_file");
        assert_eq!(tools[1].name, "write_file");

        // All tools should have ToolType::Mcp with server = "fs"
        for tool in &tools {
            match &tool.tool_type {
                crate::types::ToolType::Mcp { server, .. } => {
                    assert_eq!(server, "fs");
                }
                other => panic!("expected Mcp tool type, got: {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn discover_all_tools_skips_failed_servers() {
        // Server with no responses → connection failure during discovery
        let transport = MockTransport::new(vec![]);
        let client = Arc::new(McpClient::new(Box::new(transport)));
        let server = Arc::new(McpServer::with_client("bad".to_string(), client));

        let mut reg = McpServerRegistry::new();
        reg.register_server("bad".to_string(), server);

        // Should not error, just return empty
        let tools = reg.discover_all_tools().await.unwrap();
        assert!(tools.is_empty());
    }

    #[tokio::test]
    async fn discover_all_tools_empty_registry() {
        let reg = McpServerRegistry::new();
        let tools = reg.discover_all_tools().await.unwrap();
        assert!(tools.is_empty());
    }
}
