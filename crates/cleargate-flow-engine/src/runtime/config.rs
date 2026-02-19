//! Configuration and builder for [`ExecutionSession`].

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::defaults::{DefaultRedactor, InMemoryRunStore};
use crate::traits::{Redactor, RunStore};
use crate::write_pipeline::WritePipelineConfig;

use super::session::{ExecutionSession, SessionError, SessionHandle};

/// Configuration for an execution session.
#[derive(Clone)]
pub struct ExecutionConfig {
    pub session_name: Option<String>,
    pub run_store: Arc<dyn RunStore>,
    pub redactor: Arc<dyn Redactor>,
    pub metadata: BTreeMap<String, serde_json::Value>,
    pub pipeline_config: WritePipelineConfig,
    pub critical_capacity: usize,
    pub advisory_capacity: usize,
    /// Override version_id in RunStarted (default: "standalone").
    pub version_id: Option<String>,
    /// Override config_hash in RunStarted.
    pub config_hash: Option<String>,
    /// Override triggered_by in RunStarted.
    pub triggered_by: Option<crate::types::TriggerSource>,
    /// Override trigger_inputs in RunStarted.
    pub trigger_inputs: Option<serde_json::Value>,
    /// Set parent_run_id for replay lineage.
    pub parent_run_id: Option<String>,
}

/// Fluent builder for [`ExecutionSession`].
pub struct ExecutionSessionBuilder {
    session_name: Option<String>,
    run_store: Option<Arc<dyn RunStore>>,
    redactor: Option<Arc<dyn Redactor>>,
    metadata: BTreeMap<String, serde_json::Value>,
    pipeline_config: Option<WritePipelineConfig>,
    critical_capacity: Option<usize>,
    advisory_capacity: Option<usize>,
    version_id: Option<String>,
    config_hash: Option<String>,
    triggered_by: Option<crate::types::TriggerSource>,
    trigger_inputs: Option<serde_json::Value>,
    parent_run_id: Option<String>,
}

impl ExecutionSessionBuilder {
    pub(crate) fn new() -> Self {
        Self {
            session_name: None,
            run_store: None,
            redactor: None,
            metadata: BTreeMap::new(),
            pipeline_config: None,
            critical_capacity: None,
            advisory_capacity: None,
            version_id: None,
            config_hash: None,
            triggered_by: None,
            trigger_inputs: None,
            parent_run_id: None,
        }
    }

    pub fn session_name(mut self, name: impl Into<String>) -> Self {
        self.session_name = Some(name.into());
        self
    }

    pub fn run_store(mut self, store: Arc<dyn RunStore>) -> Self {
        self.run_store = Some(store);
        self
    }

    pub fn redactor(mut self, redactor: Arc<dyn Redactor>) -> Self {
        self.redactor = Some(redactor);
        self
    }

    pub fn metadata(mut self, metadata: BTreeMap<String, serde_json::Value>) -> Self {
        self.metadata = metadata;
        self
    }

    pub fn pipeline_config(mut self, config: WritePipelineConfig) -> Self {
        self.pipeline_config = Some(config);
        self
    }

    pub fn critical_capacity(mut self, cap: usize) -> Self {
        self.critical_capacity = Some(cap);
        self
    }

    pub fn advisory_capacity(mut self, cap: usize) -> Self {
        self.advisory_capacity = Some(cap);
        self
    }

    pub fn version_id(mut self, version_id: impl Into<String>) -> Self {
        self.version_id = Some(version_id.into());
        self
    }

    pub fn config_hash(mut self, hash: impl Into<String>) -> Self {
        self.config_hash = Some(hash.into());
        self
    }

    pub fn triggered_by(mut self, source: crate::types::TriggerSource) -> Self {
        self.triggered_by = Some(source);
        self
    }

    pub fn trigger_inputs(mut self, inputs: serde_json::Value) -> Self {
        self.trigger_inputs = Some(inputs);
        self
    }

    pub fn parent_run_id(mut self, id: impl Into<String>) -> Self {
        self.parent_run_id = Some(id.into());
        self
    }

    /// Start the session, returning the session and a handle for live event
    /// subscription.
    pub async fn start(self) -> Result<(ExecutionSession, SessionHandle), SessionError> {
        let config = ExecutionConfig {
            session_name: self.session_name,
            run_store: self
                .run_store
                .unwrap_or_else(|| Arc::new(InMemoryRunStore::new())),
            redactor: self.redactor.unwrap_or_else(|| Arc::new(DefaultRedactor)),
            metadata: self.metadata,
            pipeline_config: self.pipeline_config.unwrap_or_default(),
            critical_capacity: self.critical_capacity.unwrap_or(1000),
            advisory_capacity: self.advisory_capacity.unwrap_or(1000),
            version_id: self.version_id,
            config_hash: self.config_hash,
            triggered_by: self.triggered_by,
            trigger_inputs: self.trigger_inputs,
            parent_run_id: self.parent_run_id,
        };
        ExecutionSession::start(config).await
    }
}
