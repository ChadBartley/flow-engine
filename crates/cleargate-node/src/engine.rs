//! Node.js wrapper for `EngineBuilder` and `Engine`.

use std::path::PathBuf;
use std::sync::Arc;

use napi::bindgen_prelude::*;
use napi_derive::napi;

use cleargate_flow_engine::engine::{Engine, EngineBuilder};
use cleargate_flow_engine::executor::ExecutorConfig;
use cleargate_flow_engine::types::GraphDef;

use crate::execution::NapiExecutionHandle;
use crate::runtime::block_on;

/// Node.js-facing engine builder.
#[napi]
pub struct NapiEngineBuilder {
    inner: Option<EngineBuilder>,
}

#[napi]
impl NapiEngineBuilder {
    /// Create a new engine builder with default settings.
    #[napi(constructor)]
    pub fn new() -> Self {
        Self {
            inner: Some(Engine::builder()),
        }
    }

    /// Set the directory for flow store (JSON flow definitions).
    #[napi]
    pub fn flow_store_path(&mut self, path: String) -> Result<()> {
        let builder = self.take_builder()?;
        let store = cleargate_flow_engine::defaults::FileFlowStore::new(PathBuf::from(path))
            .map_err(|e| Error::from_reason(format!("flow store error: {e}")))?;
        self.inner = Some(builder.flow_store(store));
        Ok(())
    }

    /// Set the directory for run store (execution records).
    #[napi]
    pub fn run_store_path(&mut self, path: String) -> Result<()> {
        let builder = self.take_builder()?;
        let store = cleargate_flow_engine::defaults::FileRunStore::new(PathBuf::from(path))
            .map_err(|e| Error::from_reason(format!("run store error: {e}")))?;
        self.inner = Some(builder.run_store(store));
        Ok(())
    }

    /// Set a database URL for the run store (SQLite or PostgreSQL).
    #[napi]
    pub fn run_store_url(&mut self, url: String) -> Result<()> {
        let builder = self.take_builder()?;
        let db = block_on(cleargate_storage_oss::connect(&url))
            .map_err(|e| Error::from_reason(format!("DB connect failed: {e}")))?;
        block_on(cleargate_storage_oss::run_migrations(&db))
            .map_err(|e| Error::from_reason(format!("DB migration failed: {e}")))?;
        let store = cleargate_storage_oss::SeaOrmRunStore::new(Arc::new(db));
        self.inner = Some(builder.run_store(store));
        Ok(())
    }

    /// Register an LLM provider by name.
    ///
    /// The provider must be an object with `complete(request) => Promise<response>`
    /// and a `name` property.
    #[napi]
    pub fn llm_provider(&mut self, _name: String, _provider: serde_json::Value) -> Result<()> {
        // For now, JS LLM providers must be registered via a JSON config
        // that specifies a built-in provider type. Direct JS function bridging
        // requires ThreadsafeFunction which adds complexity.
        // TODO: Add JsLlmProviderBridge support via ThreadsafeFunction
        Err(Error::from_reason(
            "JS LLM provider bridging not yet supported. Use built-in providers or register from Rust.",
        ))
    }

    /// Register a custom node handler.
    ///
    /// The handler must be an object with `meta()` and `run(inputs, config, context)` methods.
    #[napi]
    pub fn node(&mut self, _handler: serde_json::Value) -> Result<()> {
        // Similar to LLM provider — direct JS object bridging requires
        // ThreadsafeFunction. For now, custom nodes must be registered from Rust.
        Err(Error::from_reason(
            "JS node handler bridging not yet supported. Use built-in nodes or register from Rust.",
        ))
    }

    /// Set the maximum edge traversals for cycle detection. Default: 50.
    #[napi]
    pub fn max_traversals(&mut self, value: u32) -> Result<()> {
        let builder = self.take_builder()?;
        self.inner = Some(builder.executor_config(ExecutorConfig {
            max_traversals: value,
        }));
        Ok(())
    }

    /// Enable or disable crash recovery. Default: true.
    #[napi]
    pub fn crash_recovery(&mut self, enabled: bool) -> Result<()> {
        let builder = self.take_builder()?;
        self.inner = Some(builder.crash_recovery(enabled));
        Ok(())
    }

    /// Build the engine. Consumes the builder.
    #[napi]
    pub fn build(&mut self) -> Result<NapiEngine> {
        let builder = self.take_builder()?;
        let engine = block_on(builder.build())
            .map_err(|e| Error::from_reason(format!("engine build failed: {e}")))?;
        Ok(NapiEngine {
            inner: Arc::new(engine),
        })
    }
}

impl NapiEngineBuilder {
    fn take_builder(&mut self) -> Result<EngineBuilder> {
        self.inner
            .take()
            .ok_or_else(|| Error::from_reason("builder already consumed by build()"))
    }
}

/// Node.js-facing engine — the assembled runtime.
#[napi]
pub struct NapiEngine {
    inner: Arc<Engine>,
}

#[napi]
impl NapiEngine {
    /// Execute a flow by ID with the given inputs (a JSON object).
    #[napi]
    pub fn execute(
        &self,
        flow_id: String,
        inputs: Option<serde_json::Value>,
    ) -> Result<NapiExecutionHandle> {
        let inputs_val = inputs.unwrap_or_else(|| serde_json::json!({}));
        let handle = block_on(self.inner.execute(&flow_id, inputs_val, None))
            .map_err(|e| Error::from_reason(format!("execute failed: {e}")))?;
        Ok(NapiExecutionHandle::new(handle))
    }

    /// Execute a graph definition directly.
    #[napi]
    pub fn execute_graph(
        &self,
        graph_json: serde_json::Value,
        inputs: Option<serde_json::Value>,
    ) -> Result<NapiExecutionHandle> {
        let graph: GraphDef = serde_json::from_value(graph_json)
            .map_err(|e| Error::from_reason(format!("invalid graph JSON: {e}")))?;
        let inputs_val = inputs.unwrap_or_else(|| serde_json::json!({}));

        let handle = block_on(self.inner.execute_graph(
            &graph,
            "node-adhoc",
            inputs_val,
            cleargate_flow_engine::types::TriggerSource::Api {
                request_id: uuid::Uuid::new_v4().to_string(),
            },
            None,
        ))
        .map_err(|e| Error::from_reason(format!("execute_graph failed: {e}")))?;

        Ok(NapiExecutionHandle::new(handle))
    }

    /// Deliver input to a node waiting for human-in-the-loop.
    #[napi]
    pub fn provide_input(
        &self,
        run_id: String,
        node_id: String,
        input: serde_json::Value,
    ) -> Result<()> {
        block_on(self.inner.provide_input(&run_id, &node_id, input))
            .map_err(|e| Error::from_reason(format!("provide_input failed: {e}")))
    }

    /// Return metadata for all registered node handlers.
    #[napi]
    pub fn node_catalog(&self) -> Result<serde_json::Value> {
        serde_json::to_value(self.inner.node_catalog())
            .map_err(|e| Error::from_reason(format!("serialize failed: {e}")))
    }

    /// Gracefully shut down the engine.
    #[napi]
    pub fn shutdown(&self) {
        block_on(self.inner.shutdown());
    }
}
