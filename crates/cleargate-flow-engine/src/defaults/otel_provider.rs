//! Observability provider using the `tracing` crate.
//!
//! Produces deterministic span IDs derived from execution identifiers
//! (run_id, node_id, attempt) via SHA-256. Uses `tracing` spans and
//! events for telemetry emission.

use sha2::{Digest, Sha256};
use tokio::time::Instant;

use crate::traits::{ObservabilityProvider, SpanHandle, SpanStatus};
use crate::types::{LlmRequest, LlmResponse, NodeError, NodeMeta};
use serde_json::Value;

use crate::types::Edge;

/// Observability provider backed by the `tracing` crate.
///
/// Span IDs are deterministic — the same `(run_id, node_id, attempt)`
/// always produces the same trace/span ID, satisfying invariant I5.
pub struct OtelProvider;

impl OtelProvider {
    /// Create a new `OtelProvider`.
    pub fn new() -> Self {
        Self
    }
}

impl Default for OtelProvider {
    fn default() -> Self {
        Self::new()
    }
}

/// Derive a deterministic hex ID from the given input parts.
fn deterministic_id(parts: &[&str], len: usize) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part.as_bytes());
        hasher.update(b":");
    }
    let hash = hasher.finalize();
    hash.iter().take(len).map(|b| format!("{b:02x}")).collect()
}

impl ObservabilityProvider for OtelProvider {
    fn start_node_span(
        &self,
        run_id: &str,
        node_id: &str,
        attempt: u32,
        meta: &NodeMeta,
    ) -> SpanHandle {
        let attempt_str = attempt.to_string();
        let trace_id = deterministic_id(&[run_id], 16); // 128-bit
        let span_id = deterministic_id(&[run_id, node_id, &attempt_str], 8); // 64-bit

        let span = tracing::info_span!(
            "node_execution",
            %run_id,
            %node_id,
            attempt,
            node_type = %meta.node_type,
            otel.trace_id = %trace_id,
            otel.span_id = %span_id,
        );

        SpanHandle {
            trace_id,
            span_id,
            start_time: Instant::now(),
            inner: Box::new(span),
        }
    }

    fn record_io(&self, span: &SpanHandle, inputs: &Value, outputs: &Value) {
        if let Some(s) = span.inner.downcast_ref::<tracing::Span>() {
            s.in_scope(|| {
                tracing::info!(
                    inputs = %inputs,
                    outputs = %outputs,
                    "node I/O"
                );
            });
        }
    }

    fn record_edge(&self, span: &SpanHandle, edge: &Edge, result: bool, data: &Value) {
        if let Some(s) = span.inner.downcast_ref::<tracing::Span>() {
            s.in_scope(|| {
                tracing::info!(
                    edge_id = %edge.id,
                    result,
                    data = %data,
                    "edge evaluated"
                );
            });
        }
    }

    fn record_event(&self, span: &SpanHandle, name: &str, data: &Value) {
        if let Some(s) = span.inner.downcast_ref::<tracing::Span>() {
            s.in_scope(|| {
                tracing::info!(
                    event_name = %name,
                    data = %data,
                    "custom event"
                );
            });
        }
    }

    fn record_error(&self, span: &SpanHandle, error: &NodeError) {
        if let Some(s) = span.inner.downcast_ref::<tracing::Span>() {
            s.in_scope(|| {
                tracing::error!(
                    error = %error,
                    "node error"
                );
            });
        }
    }

    fn record_llm(&self, span: &SpanHandle, request: &LlmRequest, response: &LlmResponse) {
        if let Some(s) = span.inner.downcast_ref::<tracing::Span>() {
            s.in_scope(|| {
                tracing::info!(
                    model = %request.model,
                    provider = %request.provider,
                    input_tokens = ?response.input_tokens,
                    output_tokens = ?response.output_tokens,
                    finish_reason = %response.finish_reason,
                    latency_ms = response.latency_ms,
                    "LLM invocation"
                );
            });
        }
    }

    fn end_span(&self, span: SpanHandle, status: SpanStatus) {
        if let Some(s) = span.inner.downcast_ref::<tracing::Span>() {
            s.in_scope(|| match &status {
                SpanStatus::Ok => tracing::info!("span completed"),
                SpanStatus::Error(msg) => tracing::error!(error = %msg, "span failed"),
                SpanStatus::Timeout => tracing::warn!("span timed out"),
                SpanStatus::Skipped => tracing::debug!("span skipped"),
            });
        }
        // SpanHandle is dropped here, closing the tracing span.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ExecutionHints, NodeMeta, NodeUiHints};
    use serde_json::json;
    use std::collections::BTreeMap;

    fn test_meta() -> NodeMeta {
        NodeMeta {
            node_type: "test_node".into(),
            label: "Test Node".into(),
            category: "test".into(),
            inputs: vec![],
            outputs: vec![],
            config_schema: json!({}),
            ui: NodeUiHints::default(),
            execution: ExecutionHints::default(),
        }
    }

    #[test]
    fn test_deterministic_span_ids() {
        let provider = OtelProvider::new();
        let meta = test_meta();

        let span1 = provider.start_node_span("run-1", "node-1", 1, &meta);
        let span2 = provider.start_node_span("run-1", "node-1", 1, &meta);

        assert_eq!(span1.trace_id, span2.trace_id);
        assert_eq!(span1.span_id, span2.span_id);

        // Different inputs produce different IDs.
        let span3 = provider.start_node_span("run-2", "node-1", 1, &meta);
        assert_ne!(span1.trace_id, span3.trace_id);

        let span4 = provider.start_node_span("run-1", "node-1", 2, &meta);
        assert_ne!(span1.span_id, span4.span_id);
    }

    #[test]
    fn test_span_lifecycle() {
        let provider = OtelProvider::new();
        let meta = test_meta();

        let span = provider.start_node_span("run-1", "node-1", 1, &meta);
        assert!(!span.trace_id.is_empty());
        assert!(!span.span_id.is_empty());

        provider.record_io(&span, &json!({"in": 1}), &json!({"out": 2}));

        let edge = Edge {
            id: "e1".into(),
            from_node: "a".into(),
            from_port: "out".into(),
            to_node: "b".into(),
            to_port: "in".into(),
            condition: None,
        };
        provider.record_edge(&span, &edge, true, &json!({}));
        provider.record_event(&span, "test_event", &json!({"key": "value"}));
        provider.record_error(
            &span,
            &NodeError::Fatal {
                message: "test error".into(),
            },
        );

        let req = LlmRequest {
            provider: "openai".into(),
            model: "gpt-4o".into(),
            messages: json!([]),
            tools: None,
            temperature: None,
            top_p: None,
            max_tokens: None,
            stop_sequences: None,
            response_format: None,
            seed: None,
            extra_params: BTreeMap::new(),
        };
        let resp = LlmResponse {
            content: json!("hi"),
            tool_calls: None,
            model_used: "gpt-4o".into(),
            input_tokens: Some(10),
            output_tokens: Some(5),
            total_tokens: Some(15),
            finish_reason: "stop".into(),
            latency_ms: 100,
            provider_request_id: None,
            cost: None,
        };
        provider.record_llm(&span, &req, &resp);

        provider.end_span(span, SpanStatus::Ok);
    }
}
