//! The single event enum for all execution data flowing through the write pipeline.
//!
//! Every variant carries a monotonic `seq` number (assigned by WriteHandle)
//! and a `schema_version` for forward-compatible deserialization.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::types::{LlmChunk, LlmRequest, LlmResponse, LlmRunSummary, RunStatus, TriggerSource};

/// All execution data passes through this enum. Authoritative events use
/// the critical write channel (never dropped); diagnostic events use the
/// advisory channel (may be dropped under backpressure).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case", tag = "event_type")]
#[non_exhaustive]
pub enum WriteEvent {
    RunStarted {
        seq: u64,
        schema_version: u16,
        run_id: String,
        flow_id: String,
        version_id: String,
        trigger_inputs: serde_json::Value,
        triggered_by: TriggerSource,
        config_hash: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_run_id: Option<String>,
        timestamp: DateTime<Utc>,
    },
    RunCompleted {
        seq: u64,
        schema_version: u16,
        run_id: String,
        status: RunStatus,
        duration_ms: u64,
        llm_summary: Option<LlmRunSummary>,
        timestamp: DateTime<Utc>,
    },
    NodeStarted {
        seq: u64,
        schema_version: u16,
        run_id: String,
        node_id: String,
        inputs: serde_json::Value,
        fan_out_index: Option<u32>,
        attempt: u32,
        timestamp: DateTime<Utc>,
    },
    NodeCompleted {
        seq: u64,
        schema_version: u16,
        run_id: String,
        node_id: String,
        outputs: serde_json::Value,
        fan_out_index: Option<u32>,
        duration_ms: u64,
        timestamp: DateTime<Utc>,
    },
    NodeFailed {
        seq: u64,
        schema_version: u16,
        run_id: String,
        node_id: String,
        error: String,
        will_retry: bool,
        attempt: u32,
        timestamp: DateTime<Utc>,
    },
    EdgeEvaluated {
        seq: u64,
        schema_version: u16,
        run_id: String,
        edge_id: String,
        condition: Option<String>,
        result: bool,
        data_summary: serde_json::Value,
        timestamp: DateTime<Utc>,
    },
    Checkpoint {
        seq: u64,
        schema_version: u16,
        run_id: String,
        node_id: String,
        state_snapshot: serde_json::Value,
        timestamp: DateTime<Utc>,
    },
    /// Structured LLM invocation record. Authoritative — always sent
    /// through the critical channel. Contains everything needed to
    /// reproduce the call, analyze cost, and compare across models.
    /// Request/response are boxed to keep variant sizes uniform.
    LlmInvocation {
        seq: u64,
        schema_version: u16,
        run_id: String,
        node_id: String,
        request: Box<LlmRequest>,
        response: Box<LlmResponse>,
        timestamp: DateTime<Utc>,
    },
    /// Ephemeral streaming LLM chunk. Advisory — may be dropped under
    /// backpressure. The final `LlmInvocation` event is authoritative.
    LlmStreamChunk {
        seq: u64,
        schema_version: u16,
        run_id: String,
        node_id: String,
        chunk: LlmChunk,
        timestamp: DateTime<Utc>,
    },
    ToolInvocation {
        seq: u64,
        schema_version: u16,
        run_id: String,
        tool_name: String,
        inputs: serde_json::Value,
        outputs: serde_json::Value,
        duration_ms: u64,
        timestamp: DateTime<Utc>,
    },
    StepRecorded {
        seq: u64,
        schema_version: u16,
        run_id: String,
        step_name: String,
        data: serde_json::Value,
        timestamp: DateTime<Utc>,
    },
    Custom {
        seq: u64,
        schema_version: u16,
        run_id: String,
        node_id: Option<String>,
        name: String,
        data: serde_json::Value,
        timestamp: DateTime<Utc>,
    },
    /// Optional graph topology hint emitted by ObserverSession.
    /// Advisory — helps visualization and analysis but is not required
    /// for correctness. Emitted explicitly via `set_flow_topology()` or
    /// auto-inferred from recorded node IDs on `finish()`.
    TopologyHint {
        seq: u64,
        schema_version: u16,
        run_id: String,
        nodes: Vec<String>,
        edges: Vec<(String, String)>,
        timestamp: DateTime<Utc>,
    },
}

impl WriteEvent {
    /// Returns the sequence number of this event.
    pub fn seq(&self) -> u64 {
        match self {
            Self::RunStarted { seq, .. }
            | Self::RunCompleted { seq, .. }
            | Self::NodeStarted { seq, .. }
            | Self::NodeCompleted { seq, .. }
            | Self::NodeFailed { seq, .. }
            | Self::EdgeEvaluated { seq, .. }
            | Self::Checkpoint { seq, .. }
            | Self::LlmInvocation { seq, .. }
            | Self::LlmStreamChunk { seq, .. }
            | Self::ToolInvocation { seq, .. }
            | Self::StepRecorded { seq, .. }
            | Self::Custom { seq, .. }
            | Self::TopologyHint { seq, .. } => *seq,
        }
    }

    /// Sets the sequence number on this event.
    pub fn set_seq(&mut self, new_seq: u64) {
        match self {
            Self::RunStarted { seq, .. }
            | Self::RunCompleted { seq, .. }
            | Self::NodeStarted { seq, .. }
            | Self::NodeCompleted { seq, .. }
            | Self::NodeFailed { seq, .. }
            | Self::EdgeEvaluated { seq, .. }
            | Self::Checkpoint { seq, .. }
            | Self::LlmInvocation { seq, .. }
            | Self::LlmStreamChunk { seq, .. }
            | Self::ToolInvocation { seq, .. }
            | Self::StepRecorded { seq, .. }
            | Self::Custom { seq, .. }
            | Self::TopologyHint { seq, .. } => *seq = new_seq,
        }
    }

    /// Returns the run_id associated with this event.
    pub fn run_id(&self) -> &str {
        match self {
            Self::RunStarted { run_id, .. }
            | Self::RunCompleted { run_id, .. }
            | Self::NodeStarted { run_id, .. }
            | Self::NodeCompleted { run_id, .. }
            | Self::NodeFailed { run_id, .. }
            | Self::EdgeEvaluated { run_id, .. }
            | Self::Checkpoint { run_id, .. }
            | Self::LlmInvocation { run_id, .. }
            | Self::LlmStreamChunk { run_id, .. }
            | Self::ToolInvocation { run_id, .. }
            | Self::StepRecorded { run_id, .. }
            | Self::Custom { run_id, .. }
            | Self::TopologyHint { run_id, .. } => run_id,
        }
    }

    /// Whether this event must go through the critical (never-drop) channel.
    pub fn is_critical(&self) -> bool {
        matches!(
            self,
            Self::RunStarted { .. }
                | Self::RunCompleted { .. }
                | Self::NodeStarted { .. }
                | Self::NodeCompleted { .. }
                | Self::NodeFailed { .. }
                | Self::EdgeEvaluated { .. }
                | Self::Checkpoint { .. }
                | Self::LlmInvocation { .. }
                | Self::ToolInvocation { .. }
                | Self::StepRecorded { .. }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::super::types::WRITE_EVENT_SCHEMA_VERSION;
    use super::*;
    use serde_json::json;
    use std::collections::BTreeMap;

    #[test]
    fn write_event_round_trip() {
        let event = WriteEvent::LlmInvocation {
            seq: 42,
            schema_version: WRITE_EVENT_SCHEMA_VERSION,
            run_id: "run-1".into(),
            node_id: "llm-1".into(),
            request: Box::new(LlmRequest {
                provider: "openai".into(),
                model: "gpt-4o".into(),
                messages: json!([]),
                tools: None,
                temperature: Some(0.7),
                top_p: None,
                max_tokens: None,
                stop_sequences: None,
                response_format: None,
                seed: None,
                extra_params: BTreeMap::new(),
            }),
            response: Box::new(LlmResponse {
                content: json!("hi"),
                tool_calls: None,
                model_used: "gpt-4o".into(),
                input_tokens: Some(10),
                output_tokens: Some(5),
                total_tokens: Some(15),
                finish_reason: "stop".into(),
                latency_ms: 200,
                provider_request_id: None,
                cost: None,
            }),
            timestamp: chrono::Utc::now(),
        };

        let json_str = serde_json::to_string(&event).unwrap();
        let rt: WriteEvent = serde_json::from_str(&json_str).unwrap();
        assert_eq!(rt.seq(), 42);
        assert!(rt.is_critical());
    }

    #[test]
    fn custom_event_is_not_critical() {
        let event = WriteEvent::Custom {
            seq: 1,
            schema_version: WRITE_EVENT_SCHEMA_VERSION,
            run_id: "r".into(),
            node_id: None,
            name: "debug".into(),
            data: json!({}),
            timestamp: chrono::Utc::now(),
        };
        assert!(!event.is_critical());
    }

    #[test]
    fn set_seq_works() {
        let mut event = WriteEvent::NodeStarted {
            seq: 0,
            schema_version: WRITE_EVENT_SCHEMA_VERSION,
            run_id: "r".into(),
            node_id: "n".into(),
            inputs: json!({}),
            fan_out_index: None,
            attempt: 1,
            timestamp: chrono::Utc::now(),
        };
        event.set_seq(99);
        assert_eq!(event.seq(), 99);
    }

    #[test]
    fn all_critical_variants() {
        let ts = chrono::Utc::now();
        let sv = WRITE_EVENT_SCHEMA_VERSION;
        let criticals = vec![
            WriteEvent::RunStarted {
                seq: 0,
                schema_version: sv,
                run_id: "r".into(),
                flow_id: "f".into(),
                version_id: "v".into(),
                trigger_inputs: json!({}),
                triggered_by: super::super::TriggerSource::Manual {
                    principal: "x".into(),
                },
                config_hash: None,
                parent_run_id: None,
                timestamp: ts,
            },
            WriteEvent::RunCompleted {
                seq: 0,
                schema_version: sv,
                run_id: "r".into(),
                status: RunStatus::Completed,
                duration_ms: 100,
                llm_summary: None,
                timestamp: ts,
            },
            WriteEvent::NodeStarted {
                seq: 0,
                schema_version: sv,
                run_id: "r".into(),
                node_id: "n".into(),
                inputs: json!({}),
                fan_out_index: None,
                attempt: 1,
                timestamp: ts,
            },
            WriteEvent::NodeCompleted {
                seq: 0,
                schema_version: sv,
                run_id: "r".into(),
                node_id: "n".into(),
                outputs: json!({}),
                fan_out_index: None,
                duration_ms: 50,
                timestamp: ts,
            },
            WriteEvent::NodeFailed {
                seq: 0,
                schema_version: sv,
                run_id: "r".into(),
                node_id: "n".into(),
                error: "e".into(),
                will_retry: false,
                attempt: 1,
                timestamp: ts,
            },
            WriteEvent::EdgeEvaluated {
                seq: 0,
                schema_version: sv,
                run_id: "r".into(),
                edge_id: "e".into(),
                condition: None,
                result: true,
                data_summary: json!({}),
                timestamp: ts,
            },
            WriteEvent::Checkpoint {
                seq: 0,
                schema_version: sv,
                run_id: "r".into(),
                node_id: "n".into(),
                state_snapshot: json!({}),
                timestamp: ts,
            },
        ];
        for ev in &criticals {
            assert!(ev.is_critical(), "Expected critical: {:?}", ev);
        }
    }

    #[test]
    fn llm_stream_chunk_round_trip_and_not_critical() {
        let event = WriteEvent::LlmStreamChunk {
            seq: 10,
            schema_version: WRITE_EVENT_SCHEMA_VERSION,
            run_id: "r".into(),
            node_id: "llm-1".into(),
            chunk: super::super::LlmChunk::TextDelta {
                delta: "Hello".into(),
            },
            timestamp: chrono::Utc::now(),
        };
        let json_str = serde_json::to_string(&event).unwrap();
        let rt: WriteEvent = serde_json::from_str(&json_str).unwrap();
        assert_eq!(rt.seq(), 10);
        assert_eq!(rt.run_id(), "r");
        assert!(!rt.is_critical());
    }
}
