//! MCP transport trait and implementations (stdio, SSE).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{oneshot, Mutex};

use super::types::{JsonRpcRequest, JsonRpcResponse};
use super::McpError;

/// Transport layer for communicating with an MCP server.
#[async_trait]
pub trait McpTransport: Send + Sync {
    /// Send a JSON-RPC request and receive the response.
    async fn send(&self, request: JsonRpcRequest) -> Result<JsonRpcResponse, McpError>;

    /// Close the transport, releasing any resources.
    async fn close(&self) -> Result<(), McpError>;
}

// ---------------------------------------------------------------------------
// Stdio transport
// ---------------------------------------------------------------------------

/// Communicates with an MCP server over stdin/stdout of a child process.
///
/// Sends newline-delimited JSON on stdin, reads newline-delimited JSON from
/// stdout. A background task reads stdout and correlates responses by ID.
pub struct StdioTransport {
    stdin: Mutex<tokio::process::ChildStdin>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>>,
    child: Mutex<Child>,
    closed: AtomicBool,
    _reader_handle: tokio::task::JoinHandle<()>,
}

impl StdioTransport {
    /// Spawn a child process and create a stdio transport.
    pub fn new(mut child: Child) -> Result<Self, McpError> {
        let stdin = child.stdin.take().ok_or_else(|| McpError::Connection {
            message: "child process stdin not captured".into(),
        })?;
        let stdout = child.stdout.take().ok_or_else(|| McpError::Connection {
            message: "child process stdout not captured".into(),
        })?;

        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let pending_clone = Arc::clone(&pending);

        // Background reader: reads newline-delimited JSON from stdout.
        let reader_handle = tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                if line.trim().is_empty() {
                    continue;
                }
                match serde_json::from_str::<JsonRpcResponse>(&line) {
                    Ok(resp) => {
                        if let Some(id) = resp.id {
                            let mut guard = pending_clone.lock().await;
                            if let Some(sender) = guard.remove(&id) {
                                let _ = sender.send(resp);
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "failed to parse MCP response line");
                    }
                }
            }
        });

        Ok(Self {
            stdin: Mutex::new(stdin),
            pending,
            child: Mutex::new(child),
            closed: AtomicBool::new(false),
            _reader_handle: reader_handle,
        })
    }

    /// Spawn a child process from the given command and arguments.
    pub async fn spawn(
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
    ) -> Result<Self, McpError> {
        let mut cmd = Command::new(command);
        cmd.args(args)
            .envs(env)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());

        let child = cmd.spawn().map_err(|e| McpError::Connection {
            message: format!("failed to spawn MCP server process `{command}`: {e}"),
        })?;

        Self::new(child)
    }
}

#[async_trait]
impl McpTransport for StdioTransport {
    async fn send(&self, request: JsonRpcRequest) -> Result<JsonRpcResponse, McpError> {
        if self.closed.load(Ordering::Relaxed) {
            return Err(McpError::Connection {
                message: "transport closed".into(),
            });
        }

        let id = request.id;

        // Register pending response.
        let (tx, rx) = oneshot::channel();
        {
            let mut guard = self.pending.lock().await;
            guard.insert(id, tx);
        }

        // Serialize and send.
        let mut line = serde_json::to_string(&request).map_err(|e| McpError::Protocol {
            message: format!("failed to serialize request: {e}"),
        })?;
        line.push('\n');

        {
            let mut stdin = self.stdin.lock().await;
            stdin
                .write_all(line.as_bytes())
                .await
                .map_err(|e| McpError::Connection {
                    message: format!("failed to write to stdin: {e}"),
                })?;
            stdin.flush().await.map_err(|e| McpError::Connection {
                message: format!("failed to flush stdin: {e}"),
            })?;
        }

        // Wait for the correlated response.
        let resp = tokio::time::timeout(std::time::Duration::from_secs(30), rx)
            .await
            .map_err(|_| McpError::Timeout { elapsed_ms: 30_000 })?
            .map_err(|_| McpError::Connection {
                message: "response channel closed".into(),
            })?;

        Ok(resp)
    }

    async fn close(&self) -> Result<(), McpError> {
        self.closed.store(true, Ordering::Relaxed);
        let mut child = self.child.lock().await;
        let _ = child.kill().await;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// SSE transport
// ---------------------------------------------------------------------------

/// Communicates with an MCP server over HTTP Server-Sent Events.
///
/// Sends JSON-RPC requests via HTTP POST and reads responses from an SSE
/// stream.
pub struct SseTransport {
    client: reqwest::Client,
    endpoint: String,
    headers: HashMap<String, String>,
    closed: AtomicBool,
}

impl SseTransport {
    /// Create a new SSE transport pointing at the given endpoint.
    pub fn new(endpoint: String, headers: HashMap<String, String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            endpoint,
            headers,
            closed: AtomicBool::new(false),
        }
    }
}

#[async_trait]
impl McpTransport for SseTransport {
    async fn send(&self, request: JsonRpcRequest) -> Result<JsonRpcResponse, McpError> {
        if self.closed.load(Ordering::Relaxed) {
            return Err(McpError::Connection {
                message: "transport closed".into(),
            });
        }

        let mut builder = self.client.post(&self.endpoint);
        builder = builder.header("Content-Type", "application/json");
        for (k, v) in &self.headers {
            builder = builder.header(k, v);
        }

        let body = serde_json::to_string(&request).map_err(|e| McpError::Protocol {
            message: format!("failed to serialize request: {e}"),
        })?;

        let resp = builder
            .body(body)
            .send()
            .await
            .map_err(|e| McpError::Connection {
                message: format!("HTTP request failed: {e}"),
            })?;

        if !resp.status().is_success() {
            return Err(McpError::Connection {
                message: format!("HTTP {} from MCP server", resp.status()),
            });
        }

        let text = resp.text().await.map_err(|e| McpError::Connection {
            message: format!("failed to read response body: {e}"),
        })?;

        serde_json::from_str::<JsonRpcResponse>(&text).map_err(|e| McpError::Protocol {
            message: format!("failed to parse JSON-RPC response: {e}"),
        })
    }

    async fn close(&self) -> Result<(), McpError> {
        self.closed.store(true, Ordering::Relaxed);
        Ok(())
    }
}
