//! Engine builder — assembles all v2 components into a unified runtime.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;
use tokio::sync::{broadcast, mpsc};

use super::error::EngineError;
use super::Engine;
use crate::defaults::{
    DefaultRedactor, EnvSecretsProvider, FileFlowStore, FileRunStore, FileTagStore, InMemoryQueue,
    InMemoryState,
};
use crate::dylib::DylibNodeLoader;
use crate::executor::{Executor, ExecutorConfig};
use crate::nodes::{
    ConditionalNode, CsvToolNode, HttpNode, HumanInLoopNode, JqNode, JsonLookupNode, LlmCallNode,
    ToolRouterNode,
};
use crate::traits::{
    FlowLlmProvider, FlowStore, NodeHandler, NoopObservability, ObservabilityProvider,
    QueueProvider, Redactor, RunStore, SecretsProvider, StateStore, TagStore, Trigger,
};
use crate::triggers::{RunCompletedEvent, TriggerInstance, TriggerRunner};
use crate::types::{TriggerEvent, TypeDef};
use crate::versioning::FlowVersioning;
use crate::write_pipeline::WritePipelineConfig;

/// Internal struct to hold trigger registration data before build().
struct TriggerRegistration {
    trigger: Arc<dyn Trigger>,
    flow_id: String,
    config: Value,
}

/// Builder for assembling the [`Engine`].
///
/// All provider fields are optional — sensible defaults are applied during
/// [`build()`](EngineBuilder::build).
pub struct EngineBuilder {
    nodes: BTreeMap<String, Arc<dyn NodeHandler>>,
    type_defs: Vec<TypeDef>,
    triggers: Vec<TriggerRegistration>,
    secrets: Option<Arc<dyn SecretsProvider>>,
    state: Option<Arc<dyn StateStore>>,
    queue: Option<Arc<dyn QueueProvider>>,
    run_store: Option<Arc<dyn RunStore>>,
    flow_store: Option<Arc<dyn FlowStore>>,
    tag_store: Option<Arc<dyn TagStore>>,
    redactor: Option<Arc<dyn Redactor>>,
    observability: Option<Arc<dyn ObservabilityProvider>>,
    llm_providers: HashMap<String, Arc<dyn FlowLlmProvider>>,
    executor_config: ExecutorConfig,
    pipeline_config: WritePipelineConfig,
    dylib_dirs: Vec<PathBuf>,
    crash_recovery: bool,
    #[cfg(feature = "mcp")]
    mcp_servers: Vec<(String, crate::mcp::McpServerConfig)>,
    #[cfg(feature = "memory")]
    memory_manager: Option<Arc<dyn crate::memory::MemoryManager>>,
    #[cfg(feature = "memory")]
    token_counter: Option<Arc<dyn crate::memory::TokenCounter>>,
}

impl EngineBuilder {
    pub(super) fn new() -> Self {
        Self {
            nodes: BTreeMap::new(),
            type_defs: Vec::new(),
            triggers: Vec::new(),
            secrets: None,
            state: None,
            queue: None,
            run_store: None,
            flow_store: None,
            tag_store: None,
            redactor: None,
            observability: None,
            llm_providers: HashMap::new(),
            executor_config: ExecutorConfig::default(),
            pipeline_config: WritePipelineConfig::default(),
            dylib_dirs: Vec::new(),
            crash_recovery: true,
            #[cfg(feature = "mcp")]
            mcp_servers: Vec::new(),
            #[cfg(feature = "memory")]
            memory_manager: None,
            #[cfg(feature = "memory")]
            token_counter: None,
        }
    }

    /// Register a node handler. Keyed by `handler.meta().node_type`.
    pub fn node(mut self, handler: impl NodeHandler + 'static) -> Self {
        let meta = handler.meta();
        self.nodes.insert(meta.node_type, Arc::new(handler));
        self
    }

    /// Append type definitions for the type catalog.
    pub fn types(mut self, defs: Vec<TypeDef>) -> Self {
        self.type_defs.extend(defs);
        self
    }

    /// Register a trigger bound to a specific flow.
    pub fn trigger(
        mut self,
        trigger: impl Trigger + 'static,
        flow_id: &str,
        config: Value,
    ) -> Self {
        self.triggers.push(TriggerRegistration {
            trigger: Arc::new(trigger),
            flow_id: flow_id.to_string(),
            config,
        });
        self
    }

    /// Set the secrets provider. Default: [`EnvSecretsProvider`].
    pub fn secrets(mut self, provider: impl SecretsProvider + 'static) -> Self {
        self.secrets = Some(Arc::new(provider));
        self
    }

    /// Set the state store. Default: [`InMemoryState`].
    pub fn state(mut self, store: impl StateStore + 'static) -> Self {
        self.state = Some(Arc::new(store));
        self
    }

    /// Set the queue provider. Default: [`InMemoryQueue`].
    pub fn queue(mut self, provider: impl QueueProvider + 'static) -> Self {
        self.queue = Some(Arc::new(provider));
        self
    }

    /// Set the run store. Default: [`FileRunStore`].
    pub fn run_store(mut self, store: impl RunStore + 'static) -> Self {
        self.run_store = Some(Arc::new(store));
        self
    }

    /// Set the flow store. Default: [`FileFlowStore`].
    pub fn flow_store(mut self, store: impl FlowStore + 'static) -> Self {
        self.flow_store = Some(Arc::new(store));
        self
    }

    /// Set the tag store. Default: [`FileTagStore`].
    pub fn tag_store(mut self, store: impl TagStore + 'static) -> Self {
        self.tag_store = Some(Arc::new(store));
        self
    }

    /// Set the redactor. Default: [`DefaultRedactor`].
    pub fn redactor(mut self, redactor: impl Redactor + 'static) -> Self {
        self.redactor = Some(Arc::new(redactor));
        self
    }

    /// Set the observability provider. Default: [`NoopObservability`].
    pub fn observability(mut self, provider: impl ObservabilityProvider + 'static) -> Self {
        self.observability = Some(Arc::new(provider));
        self
    }

    /// Register an LLM provider by name. Nodes reference providers by this name.
    pub fn llm_provider(mut self, name: &str, provider: Arc<dyn FlowLlmProvider>) -> Self {
        self.llm_providers.insert(name.to_string(), provider);
        self
    }

    /// Set the executor configuration.
    pub fn executor_config(mut self, config: ExecutorConfig) -> Self {
        self.executor_config = config;
        self
    }

    /// Set the write pipeline configuration.
    pub fn pipeline_config(mut self, config: WritePipelineConfig) -> Self {
        self.pipeline_config = config;
        self
    }

    /// Add a directory to scan for dylib node plugins.
    pub fn dylib_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.dylib_dirs.push(path.into());
        self
    }

    /// Enable or disable crash recovery (marking orphaned runs as interrupted).
    /// Default: true.
    pub fn crash_recovery(mut self, enabled: bool) -> Self {
        self.crash_recovery = enabled;
        self
    }

    /// Register an MCP server by name and configuration.
    ///
    /// The server connects lazily on first use. Discovered tools are
    /// automatically merged into `available_tools()` at run time.
    #[cfg(feature = "mcp")]
    pub fn mcp_server(mut self, name: &str, config: crate::mcp::McpServerConfig) -> Self {
        self.mcp_servers.push((name.to_string(), config));
        self
    }

    /// Set a custom memory manager. Default: [`InMemoryManager`](crate::memory::InMemoryManager).
    #[cfg(feature = "memory")]
    pub fn memory_manager(mut self, mgr: Arc<dyn crate::memory::MemoryManager>) -> Self {
        self.memory_manager = Some(mgr);
        self
    }

    /// Set a custom token counter. Default: [`CharEstimateCounter`](crate::memory::CharEstimateCounter).
    #[cfg(feature = "memory")]
    pub fn token_counter(mut self, counter: Arc<dyn crate::memory::TokenCounter>) -> Self {
        self.token_counter = Some(counter);
        self
    }

    /// Assemble the engine. Applies defaults for any unset providers,
    /// scans dylib directories, registers built-in nodes, and starts
    /// the trigger system.
    pub async fn build(mut self) -> Result<Engine, EngineError> {
        // 1. Apply defaults for unset providers.
        let flow_store: Arc<dyn FlowStore> = match self.flow_store {
            Some(s) => s,
            None => {
                let store = FileFlowStore::new(PathBuf::from("./data")).map_err(|e| {
                    EngineError::Build {
                        message: format!("failed to create default flow store: {e}"),
                    }
                })?;
                Arc::new(store)
            }
        };

        let tag_store: Arc<dyn TagStore> = match self.tag_store {
            Some(s) => s,
            None => {
                let store =
                    FileTagStore::new(PathBuf::from("./data")).map_err(|e| EngineError::Build {
                        message: format!("failed to create default tag store: {e}"),
                    })?;
                Arc::new(store)
            }
        };

        let run_store: Arc<dyn RunStore> = match self.run_store {
            Some(s) => s,
            None => {
                let store =
                    FileRunStore::new(PathBuf::from("./data")).map_err(|e| EngineError::Build {
                        message: format!("failed to create default run store: {e}"),
                    })?;
                Arc::new(store)
            }
        };

        let secrets: Arc<dyn SecretsProvider> =
            self.secrets.unwrap_or_else(|| Arc::new(EnvSecretsProvider));

        let state: Arc<dyn StateStore> =
            self.state.unwrap_or_else(|| Arc::new(InMemoryState::new()));

        let queue: Arc<dyn QueueProvider> =
            self.queue.unwrap_or_else(|| Arc::new(InMemoryQueue::new()));

        let redactor: Arc<dyn Redactor> =
            self.redactor.unwrap_or_else(|| Arc::new(DefaultRedactor));

        let observability: Arc<dyn ObservabilityProvider> = self
            .observability
            .unwrap_or_else(|| Arc::new(NoopObservability));

        // 2. Scan dylib directories and merge discovered nodes.
        for dir in &self.dylib_dirs {
            if dir.exists() {
                let handlers = DylibNodeLoader::scan(dir)?;
                for handler in handlers {
                    let node_type = handler.meta().node_type.clone();
                    self.nodes
                        .entry(node_type)
                        .or_insert_with(|| Arc::new(handler));
                }
            }
        }

        // 3. Register built-in nodes if not already present.
        #[allow(unused_mut)]
        let mut builtins: Vec<Box<dyn NodeHandler>> = vec![
            Box::new(LlmCallNode),
            Box::new(ToolRouterNode),
            Box::new(HttpNode),
            Box::new(ConditionalNode),
            Box::new(HumanInLoopNode),
            Box::new(JqNode),
            Box::new(CsvToolNode),
            Box::new(JsonLookupNode),
        ];
        #[cfg(feature = "mcp")]
        {
            builtins.push(Box::new(crate::nodes::McpCallNode));
        }
        #[cfg(feature = "memory")]
        {
            builtins.push(Box::new(crate::nodes::ContextManagementNode));
        }
        for builtin in builtins {
            let meta = builtin.meta();
            self.nodes
                .entry(meta.node_type)
                .or_insert_with(|| Arc::from(builtin));
        }

        // 4. Create FlowVersioning from the flow store.
        let versioning = Arc::new(FlowVersioning::new(Arc::clone(&flow_store)));

        // 5. Create broadcast channel for RunCompletedEvent.
        let (run_completed_tx, _) = broadcast::channel::<RunCompletedEvent>(256);

        // 6. Create Executor.
        let node_registry: HashMap<String, Arc<dyn NodeHandler>> = self
            .nodes
            .iter()
            .map(|(k, v)| (k.clone(), Arc::clone(v)))
            .collect();

        let llm_providers = Arc::new(self.llm_providers);

        #[cfg(feature = "mcp")]
        let mcp_registry = {
            let mut registry = crate::mcp::McpServerRegistry::new();
            for (name, config) in self.mcp_servers {
                registry.register(name, config);
            }
            Arc::new(registry)
        };

        let executor = Arc::new(Executor::new(
            self.executor_config,
            node_registry,
            Arc::clone(&secrets),
            Arc::clone(&state),
            Arc::clone(&queue),
            Arc::clone(&observability),
            Arc::clone(&run_store),
            Arc::clone(&redactor),
            self.pipeline_config,
            Arc::clone(&llm_providers),
            run_completed_tx.clone(),
            #[cfg(feature = "mcp")]
            Arc::clone(&mcp_registry),
            #[cfg(feature = "memory")]
            self.memory_manager
                .unwrap_or_else(|| Arc::new(crate::memory::InMemoryManager)),
            #[cfg(feature = "memory")]
            self.token_counter
                .unwrap_or_else(|| Arc::new(crate::memory::CharEstimateCounter)),
        ));

        // 9. Set up TriggerRunner (if any triggers registered).
        let (trigger_runner, trigger_event_rx, trigger_shutdown_tx) = if self.triggers.is_empty() {
            (None, None, None)
        } else {
            let instances: Vec<TriggerInstance> = self
                .triggers
                .into_iter()
                .map(|reg| TriggerInstance {
                    trigger: reg.trigger,
                    flow_id: reg.flow_id,
                    config: reg.config,
                })
                .collect();

            let (trigger_event_tx, trigger_event_rx) = mpsc::channel::<TriggerEvent>(1000);
            let (shutdown_tx, _) = broadcast::channel::<()>(1);

            let runner = TriggerRunner::new(instances, trigger_event_tx, shutdown_tx.clone());
            (Some(runner), Some(trigger_event_rx), Some(shutdown_tx))
        };

        // 10. Crash recovery: mark orphaned Running runs as Interrupted.
        if self.crash_recovery {
            let filter = crate::traits::RunFilter {
                status: Some(crate::types::RunStatus::Running),
                ..Default::default()
            };
            match run_store.list_runs(&filter).await {
                Ok(page) if !page.runs.is_empty() => {
                    tracing::warn!(
                        count = page.runs.len(),
                        "Marking interrupted runs from previous session"
                    );
                    for run in &page.runs {
                        if let Err(e) = run_store.mark_interrupted(&run.run_id).await {
                            tracing::error!(
                                run_id = %run.run_id,
                                error = %e,
                                "Failed to mark interrupted run"
                            );
                        }
                    }
                }
                Err(e) => tracing::warn!(error = %e, "Crash recovery scan failed"),
                _ => {}
            }
        }

        let nodes = Arc::new(self.nodes);
        let type_defs = Arc::new(self.type_defs);

        Ok(Engine {
            nodes,
            type_defs,
            executor,
            versioning,
            flow_store,
            tag_store,
            run_store,
            secrets,
            state,
            queue,
            redactor,
            observability,
            llm_providers,
            #[cfg(feature = "mcp")]
            mcp_registry,
            trigger_runner,
            run_completed_tx,
            trigger_event_rx: tokio::sync::Mutex::new(trigger_event_rx),
            trigger_shutdown_tx,
            trigger_handles: tokio::sync::Mutex::new(Vec::new()),
        })
    }
}
