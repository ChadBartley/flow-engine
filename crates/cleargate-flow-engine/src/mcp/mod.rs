//! MCP (Model Context Protocol) integration for FlowEngine.
//!
//! Provides client-side MCP support: connecting to MCP servers, discovering
//! tools, and calling them from within flow executions. Supports stdio and
//! SSE transports.
//!
//! # Architecture
//!
//! ```text
//! McpServerRegistry (engine-level)
//!   └── McpServer (per server, lazy connection)
//!         └── McpClient (protocol operations)
//!               └── McpTransport (stdio / SSE)
//! ```
//!
//! MCP servers are registered at engine build time via
//! [`EngineBuilder::mcp_server()`](crate::engine::EngineBuilder::mcp_server).
//! Connections are established lazily on first use.

pub mod client;
pub mod discovery;
pub mod server;
pub mod transport;
pub mod types;

pub use client::McpClient;
pub use server::{McpServer, McpServerConfig, McpServerRegistry};
pub use transport::McpTransport;

/// Errors from MCP operations.
///
/// Maps to `NodeError` variants: connection/transport errors are retryable,
/// protocol errors are fatal, timeouts map to `NodeError::Timeout`.
#[derive(Debug)]
pub enum McpError {
    /// Transport or network error — may succeed on retry.
    Connection { message: String },
    /// Protocol-level error (invalid response, method not found, etc.) — fatal.
    Protocol { message: String },
    /// Operation timed out.
    Timeout { elapsed_ms: u64 },
}

impl std::fmt::Display for McpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Connection { message } => write!(f, "MCP connection error: {message}"),
            Self::Protocol { message } => write!(f, "MCP protocol error: {message}"),
            Self::Timeout { elapsed_ms } => write!(f, "MCP timeout after {elapsed_ms}ms"),
        }
    }
}

impl std::error::Error for McpError {}

impl From<McpError> for crate::types::NodeError {
    fn from(e: McpError) -> Self {
        match e {
            McpError::Connection { message } => Self::Retryable { message },
            McpError::Protocol { message } => Self::Fatal { message },
            McpError::Timeout { elapsed_ms } => Self::Timeout { elapsed_ms },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::McpError;
    use crate::types::NodeError;

    #[test]
    fn connection_error_maps_to_retryable() {
        let err: NodeError = McpError::Connection {
            message: "conn failed".into(),
        }
        .into();
        assert!(matches!(err, NodeError::Retryable { .. }));
    }

    #[test]
    fn protocol_error_maps_to_fatal() {
        let err: NodeError = McpError::Protocol {
            message: "bad response".into(),
        }
        .into();
        assert!(matches!(err, NodeError::Fatal { .. }));
    }

    #[test]
    fn timeout_error_maps_to_timeout() {
        let err: NodeError = McpError::Timeout { elapsed_ms: 5000 }.into();
        assert!(matches!(err, NodeError::Timeout { elapsed_ms: 5000 }));
    }

    #[test]
    fn error_display() {
        let e = McpError::Connection {
            message: "refused".into(),
        };
        assert_eq!(format!("{e}"), "MCP connection error: refused");

        let e = McpError::Protocol {
            message: "invalid".into(),
        };
        assert_eq!(format!("{e}"), "MCP protocol error: invalid");

        let e = McpError::Timeout { elapsed_ms: 3000 };
        assert_eq!(format!("{e}"), "MCP timeout after 3000ms");
    }
}
