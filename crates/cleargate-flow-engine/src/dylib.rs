//! Dynamic library (dylib) node runtime for FlowEngine v2.
//!
//! Loads native shared libraries (`.so`/`.dll`/`.dylib`) that implement a C ABI
//! contract, allowing nodes written in any compiled language (Rust, C#/NativeAOT,
//! C, C++, Go, Zig).
//!
//! # C ABI Contract
//!
//! Every shared library must export these symbols:
//!
//! - `node_meta() -> *const c_char` — returns JSON-encoded [`NodeMeta`]. Freed via `node_free()`.
//! - `node_run(inputs, config, callbacks) -> *const c_char` — executes the node. Freed via `node_free()`.
//! - `node_free(ptr)` — frees a string previously returned by the dylib.
//! - `node_validate_config(config) -> *const c_char` — optional config validation.
//!
//! The engine passes a [`NodeCallbacks`] vtable so dylib code can call back into
//! engine services (emit events, record LLM calls, access secrets/state).

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;
use serde_json::Value;

use super::node_ctx::NodeCtx;
use super::traits::NodeHandler;
use super::types::{LlmRequest, LlmResponse, NodeError, NodeMeta};

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

/// Errors from loading or interacting with dylib nodes.
#[derive(Debug, thiserror::Error)]
pub enum DylibError {
    #[error("failed to load library at {path}: {source}")]
    LoadFailed {
        path: PathBuf,
        source: libloading::Error,
    },

    #[error("required symbol '{symbol}' not found in {path}")]
    SymbolNotFound { path: PathBuf, symbol: String },

    #[error("invalid UTF-8 in string returned by dylib")]
    InvalidUtf8,

    #[error("dylib returned null pointer from {function}")]
    NullPointer { function: String },

    #[error("failed to parse node_meta JSON: {source}")]
    MetaParseFailed { source: serde_json::Error },

    #[error("dylib panicked during {function}: {message}")]
    Panic { function: String, message: String },

    #[error("manifest error in {path}: {message}")]
    ManifestError { path: PathBuf, message: String },

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

// ---------------------------------------------------------------------------
// C ABI callback vtable
// ---------------------------------------------------------------------------

/// C ABI callback vtable passed to dylib nodes so they can call back into
/// engine services during execution.
///
/// The `opaque` pointer is a `Box<CallbackContext>` that lives for the duration
/// of the `run()` call. Strings returned by `secret_get` and `state_get` are
/// owned by the `CallbackContext` and valid until the next call to the same
/// callback or until `run()` returns.
#[repr(C)]
pub struct NodeCallbacks {
    pub opaque: *mut c_void,
    pub emit: unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char) -> c_int,
    pub record_llm_call: unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char) -> c_int,
    pub secret_get: unsafe extern "C" fn(*mut c_void, *const c_char) -> *const c_char,
    pub state_get: unsafe extern "C" fn(*mut c_void, *const c_char) -> *const c_char,
    pub state_set: unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char) -> c_int,
}

// ---------------------------------------------------------------------------
// CallbackContext — engine-side state for trampoline functions
// ---------------------------------------------------------------------------

/// Engine-side context pointed to by `NodeCallbacks.opaque`.
///
/// Lives for the duration of a single `run()` call. Trampolines cast
/// `opaque` back to `&CallbackContext` to forward calls to [`NodeCtx`].
pub(crate) struct CallbackContext {
    ctx: NodeCtx,
    runtime: tokio::runtime::Handle,
    /// String cache: keeps CStrings alive so pointers returned to the dylib
    /// remain valid until the context is dropped.
    cached_strings: std::cell::RefCell<Vec<CString>>,
}

impl CallbackContext {
    pub(crate) fn new(ctx: NodeCtx, runtime: tokio::runtime::Handle) -> Self {
        Self {
            ctx,
            runtime,
            cached_strings: std::cell::RefCell::new(Vec::new()),
        }
    }

    /// Build a [`NodeCallbacks`] vtable pointing to this context.
    pub(crate) fn as_vtable(&mut self) -> NodeCallbacks {
        NodeCallbacks {
            opaque: self as *mut CallbackContext as *mut c_void,
            emit: emit_trampoline,
            record_llm_call: record_llm_call_trampoline,
            secret_get: secret_get_trampoline,
            state_get: state_get_trampoline,
            state_set: state_set_trampoline,
        }
    }
}

// ---------------------------------------------------------------------------
// Trampoline functions (C ABI → NodeCtx)
// ---------------------------------------------------------------------------

/// Emit an advisory event. Returns 0 on success, -1 on error.
pub(crate) unsafe extern "C" fn emit_trampoline(
    opaque: *mut c_void,
    name: *const c_char,
    data_json: *const c_char,
) -> c_int {
    let cb = unsafe { &*(opaque as *const CallbackContext) };
    let name = match unsafe { CStr::from_ptr(name) }.to_str() {
        Ok(s) => s,
        Err(_) => return -1,
    };
    let data_str = match unsafe { CStr::from_ptr(data_json) }.to_str() {
        Ok(s) => s,
        Err(_) => return -1,
    };
    let data: Value = match serde_json::from_str(data_str) {
        Ok(v) => v,
        Err(_) => return -1,
    };
    cb.ctx.emit(name, data);
    0
}

/// Record a structured LLM invocation (critical channel). Returns 0 on success, -1 on error.
pub(crate) unsafe extern "C" fn record_llm_call_trampoline(
    opaque: *mut c_void,
    request_json: *const c_char,
    response_json: *const c_char,
) -> c_int {
    let cb = unsafe { &*(opaque as *const CallbackContext) };
    let req_str = match unsafe { CStr::from_ptr(request_json) }.to_str() {
        Ok(s) => s,
        Err(_) => return -1,
    };
    let resp_str = match unsafe { CStr::from_ptr(response_json) }.to_str() {
        Ok(s) => s,
        Err(_) => return -1,
    };
    let request: LlmRequest = match serde_json::from_str(req_str) {
        Ok(r) => r,
        Err(_) => return -1,
    };
    let response: LlmResponse = match serde_json::from_str(resp_str) {
        Ok(r) => r,
        Err(_) => return -1,
    };
    match cb
        .runtime
        .block_on(cb.ctx.record_llm_call(request, response))
    {
        Ok(()) => 0,
        Err(_) => -1,
    }
}

/// Get a secret by key. Returns null if not found. Caller must NOT free.
pub(crate) unsafe extern "C" fn secret_get_trampoline(
    opaque: *mut c_void,
    key: *const c_char,
) -> *const c_char {
    let cb = unsafe { &*(opaque as *const CallbackContext) };
    let key_str = match unsafe { CStr::from_ptr(key) }.to_str() {
        Ok(s) => s,
        Err(_) => return std::ptr::null(),
    };
    match cb.runtime.block_on(cb.ctx.secret(key_str)) {
        Ok(value) => {
            let c_string = match CString::new(value) {
                Ok(s) => s,
                Err(_) => return std::ptr::null(),
            };
            let ptr = c_string.as_ptr();
            cb.cached_strings.borrow_mut().push(c_string);
            ptr
        }
        Err(_) => std::ptr::null(),
    }
}

/// Get state value by key. Returns null if not found. Caller must NOT free.
pub(crate) unsafe extern "C" fn state_get_trampoline(
    opaque: *mut c_void,
    key: *const c_char,
) -> *const c_char {
    let cb = unsafe { &*(opaque as *const CallbackContext) };
    let key_str = match unsafe { CStr::from_ptr(key) }.to_str() {
        Ok(s) => s,
        Err(_) => return std::ptr::null(),
    };
    match cb.runtime.block_on(cb.ctx.state_get(key_str)) {
        Ok(Some(value)) => {
            let json_str = match serde_json::to_string(&value) {
                Ok(s) => s,
                Err(_) => return std::ptr::null(),
            };
            let c_string = match CString::new(json_str) {
                Ok(s) => s,
                Err(_) => return std::ptr::null(),
            };
            let ptr = c_string.as_ptr();
            cb.cached_strings.borrow_mut().push(c_string);
            ptr
        }
        _ => std::ptr::null(),
    }
}

/// Set state value by key. Returns 0 on success, -1 on error.
pub(crate) unsafe extern "C" fn state_set_trampoline(
    opaque: *mut c_void,
    key: *const c_char,
    value_json: *const c_char,
) -> c_int {
    let cb = unsafe { &*(opaque as *const CallbackContext) };
    let key_str = match unsafe { CStr::from_ptr(key) }.to_str() {
        Ok(s) => s,
        Err(_) => return -1,
    };
    let value_str = match unsafe { CStr::from_ptr(value_json) }.to_str() {
        Ok(s) => s,
        Err(_) => return -1,
    };
    let value: Value = match serde_json::from_str(value_str) {
        Ok(v) => v,
        Err(_) => return -1,
    };
    match cb.runtime.block_on(cb.ctx.state_set(key_str, value)) {
        Ok(()) => 0,
        Err(_) => -1,
    }
}

// ---------------------------------------------------------------------------
// DylibNodeHandler
// ---------------------------------------------------------------------------

// Type aliases for dylib function pointer signatures.
type MetaFn = unsafe extern "C" fn() -> *const c_char;
type RunFn =
    unsafe extern "C" fn(*const c_char, *const c_char, *const NodeCallbacks) -> *const c_char;
type FreeFn = unsafe extern "C" fn(*const c_char);
type ValidateFn = unsafe extern "C" fn(*const c_char) -> *const c_char;

/// A node handler backed by a native shared library.
///
/// The library is kept loaded for the handler's lifetime via `Arc<Library>`.
/// Function pointers are cached at load time.
///
/// # Safety
///
/// - All FFI calls are wrapped in `catch_unwind`.
/// - `run()` executes on a blocking thread via `spawn_blocking`.
/// - Null checks and UTF-8 validation on every returned pointer.
pub struct DylibNodeHandler {
    /// Keeps the shared library loaded for the handler's lifetime.
    _library: Arc<libloading::Library>,
    /// Cached metadata (parsed once at load time).
    cached_meta: NodeMeta,
    /// Path to the loaded library (for error messages).
    path: PathBuf,
    // Function pointers loaded from the library:
    #[allow(dead_code)]
    meta_fn: MetaFn,
    run_fn: RunFn,
    free_fn: FreeFn,
    validate_fn: Option<ValidateFn>,
}

// Safety: libloading::Library is Send+Sync when wrapped in Arc.
// Function pointers are inherently Send+Sync.
unsafe impl Send for DylibNodeHandler {}
unsafe impl Sync for DylibNodeHandler {}

impl DylibNodeHandler {
    /// Load a dylib node handler from the given shared library path.
    ///
    /// Looks up required symbols (`node_meta`, `node_run`, `node_free`) and
    /// optional `node_validate_config`. Calls `node_meta()` to parse and cache
    /// the node's metadata.
    pub fn load(path: &Path) -> Result<Self, DylibError> {
        // 1. Load the shared library.
        let library =
            unsafe { libloading::Library::new(path) }.map_err(|e| DylibError::LoadFailed {
                path: path.to_path_buf(),
                source: e,
            })?;

        // 2. Look up required symbols.
        let meta_fn = Self::load_symbol::<MetaFn>(&library, path, "node_meta")?;
        let run_fn = Self::load_symbol::<RunFn>(&library, path, "node_run")?;
        let free_fn = Self::load_symbol::<FreeFn>(&library, path, "node_free")?;

        // 3. Look up optional symbol.
        let validate_fn = Self::try_load_symbol::<ValidateFn>(&library, "node_validate_config");

        // 4. Call node_meta() to get and cache metadata.
        let meta_ptr =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe { meta_fn() }))
                .map_err(|e| DylibError::Panic {
                    function: "node_meta".into(),
                    message: format!("{e:?}"),
                })?;

        if meta_ptr.is_null() {
            return Err(DylibError::NullPointer {
                function: "node_meta".into(),
            });
        }

        let meta_str = unsafe { CStr::from_ptr(meta_ptr) }
            .to_str()
            .map_err(|_| DylibError::InvalidUtf8)?
            .to_string();

        // Free the meta string via node_free.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            free_fn(meta_ptr);
        }));

        let cached_meta: NodeMeta = serde_json::from_str(&meta_str)
            .map_err(|e| DylibError::MetaParseFailed { source: e })?;

        let library = Arc::new(library);

        Ok(Self {
            _library: library,
            cached_meta,
            path: path.to_path_buf(),
            meta_fn,
            run_fn,
            free_fn,
            validate_fn,
        })
    }

    fn load_symbol<T: Copy>(
        library: &libloading::Library,
        path: &Path,
        name: &str,
    ) -> Result<T, DylibError> {
        unsafe { library.get::<T>(name.as_bytes()) }
            .map(|sym| *sym)
            .map_err(|_| DylibError::SymbolNotFound {
                path: path.to_path_buf(),
                symbol: name.to_string(),
            })
    }

    fn try_load_symbol<T: Copy>(library: &libloading::Library, name: &str) -> Option<T> {
        unsafe { library.get::<T>(name.as_bytes()) }
            .map(|sym| *sym)
            .ok()
    }

    /// Helper: parse the result string from node_run, free it, and convert to
    /// either a successful Value or a NodeError.
    fn parse_run_result(
        result_ptr: *const c_char,
        free_fn: FreeFn,
        _path: &Path,
    ) -> Result<Value, NodeError> {
        if result_ptr.is_null() {
            return Err(NodeError::Fatal {
                message: "dylib node_run returned null".into(),
            });
        }

        let result_str = unsafe { CStr::from_ptr(result_ptr) }
            .to_str()
            .map_err(|_| NodeError::Fatal {
                message: "dylib returned invalid UTF-8".into(),
            })?
            .to_string();

        // Free the result string.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            free_fn(result_ptr);
        }));

        let value: Value = serde_json::from_str(&result_str).map_err(|e| NodeError::Fatal {
            message: format!("failed to parse dylib result JSON: {e}"),
        })?;

        // Check if the dylib returned an error structure.
        if let Some(ty) = value.get("type").and_then(|v| v.as_str()) {
            if ty == "error" {
                let message = value
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown dylib error")
                    .to_string();
                let retryable = value
                    .get("retryable")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                return if retryable {
                    Err(NodeError::Retryable { message })
                } else {
                    Err(NodeError::Fatal { message })
                };
            }
        }

        Ok(value)
    }
}

#[async_trait::async_trait]
impl NodeHandler for DylibNodeHandler {
    fn meta(&self) -> NodeMeta {
        self.cached_meta.clone()
    }

    async fn run(&self, inputs: Value, config: &Value, ctx: &NodeCtx) -> Result<Value, NodeError> {
        // 1. Serialize inputs and config to CStrings.
        let inputs_json =
            CString::new(
                serde_json::to_string(&inputs).map_err(|e| NodeError::Fatal {
                    message: format!("serialize inputs: {e}"),
                })?,
            )
            .map_err(|e| NodeError::Fatal {
                message: format!("CString inputs: {e}"),
            })?;

        let config_json =
            CString::new(serde_json::to_string(config).map_err(|e| NodeError::Fatal {
                message: format!("serialize config: {e}"),
            })?)
            .map_err(|e| NodeError::Fatal {
                message: format!("CString config: {e}"),
            })?;

        // 2. Build CallbackContext — we need to clone the NodeCtx fields.
        //    Since NodeCtx isn't Clone, we rebuild one with the same providers.
        //    For now, we use a workaround: the dylib handler receives &NodeCtx
        //    and we need to create a CallbackContext that outlives spawn_blocking.
        //    We use the current tokio handle to allow blocking on async calls.
        let runtime = tokio::runtime::Handle::current();

        // We need to send these across the spawn_blocking boundary.
        let run_fn = self.run_fn;
        let free_fn = self.free_fn;
        let path = self.path.clone();

        // Build a NodeCtx clone for the callback context.
        // NodeCtx::new is pub(crate) so we can access it from within the crate.
        let cb_ctx = NodeCtx::new(
            ctx.run_id().to_string(),
            ctx.node_instance_id().to_string(),
            ctx.secrets_provider(),
            ctx.state_store(),
            ctx.queue_provider(),
            ctx.event_sender(),
            ctx.llm_sender(),
            ctx.http().clone(),
            ctx.tool_definitions_arc(),
            None, // Human input not needed for dylib nodes
            ctx.llm_providers_arc(),
            #[cfg(feature = "mcp")]
            ctx.mcp_registry_arc(),
        );

        // 3. Execute on a blocking thread.
        tokio::task::spawn_blocking(move || {
            let mut cb_context = CallbackContext::new(cb_ctx, runtime);
            let vtable = cb_context.as_vtable();

            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
                run_fn(inputs_json.as_ptr(), config_json.as_ptr(), &vtable)
            }));

            match result {
                Ok(result_ptr) => Self::parse_run_result(result_ptr, free_fn, &path),
                Err(e) => Err(NodeError::Fatal {
                    message: format!("dylib panicked during node_run: {e:?}"),
                }),
            }
        })
        .await
        .map_err(|e| NodeError::Fatal {
            message: format!("dylib spawn_blocking failed: {e}"),
        })?
    }

    fn validate_config(&self, config: &Value) -> Result<(), Vec<String>> {
        let validate_fn = match self.validate_fn {
            Some(f) => f,
            None => return Ok(()),
        };

        let config_json = match CString::new(serde_json::to_string(config).unwrap_or_default()) {
            Ok(s) => s,
            Err(e) => return Err(vec![format!("CString error: {e}")]),
        };

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            validate_fn(config_json.as_ptr())
        }));

        match result {
            Ok(ptr) => {
                if ptr.is_null() {
                    // Null means valid.
                    Ok(())
                } else {
                    let err_str = unsafe { CStr::from_ptr(ptr) }
                        .to_str()
                        .unwrap_or("invalid UTF-8 in validation error");
                    let errors = vec![err_str.to_string()];
                    // Free the error string.
                    let free_fn = self.free_fn;
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
                        free_fn(ptr);
                    }));
                    Err(errors)
                }
            }
            Err(e) => Err(vec![format!(
                "dylib panicked during validate_config: {e:?}"
            )]),
        }
    }
}

// ---------------------------------------------------------------------------
// Manifest + Loader
// ---------------------------------------------------------------------------

/// Parsed `node.json` manifest for a dylib node.
#[derive(Debug, Deserialize)]
struct DylibManifest {
    #[allow(dead_code)]
    node_type: String,
    library: String,
    #[allow(dead_code)]
    description: Option<String>,
}

/// Scans a directory for dylib node manifests and loads them.
pub struct DylibNodeLoader;

impl DylibNodeLoader {
    /// Scan a directory for node manifests and load them.
    ///
    /// Each node is a subdirectory containing:
    /// - `node.json` — manifest with `node_type` and `library` (relative path)
    /// - The shared library file (`.so`/`.dll`/`.dylib`)
    ///
    /// Failures for individual nodes are logged as warnings but do not fail the scan.
    pub fn scan(dir: &Path) -> Result<Vec<DylibNodeHandler>, DylibError> {
        let mut handlers = Vec::new();

        let entries = std::fs::read_dir(dir)?;
        for entry in entries {
            let entry = entry?;
            let path = entry.path();

            if !path.is_dir() {
                continue;
            }

            let manifest_path = path.join("node.json");
            if !manifest_path.exists() {
                continue;
            }

            match Self::load_from_manifest(&manifest_path) {
                Ok(handler) => handlers.push(handler),
                Err(e) => {
                    tracing::warn!(
                        path = %manifest_path.display(),
                        error = %e,
                        "skipping dylib node due to load error"
                    );
                }
            }
        }

        Ok(handlers)
    }

    fn load_from_manifest(manifest_path: &Path) -> Result<DylibNodeHandler, DylibError> {
        let manifest_content = std::fs::read_to_string(manifest_path)?;
        let manifest: DylibManifest =
            serde_json::from_str(&manifest_content).map_err(|e| DylibError::ManifestError {
                path: manifest_path.to_path_buf(),
                message: format!("invalid JSON: {e}"),
            })?;

        let parent = manifest_path
            .parent()
            .ok_or_else(|| DylibError::ManifestError {
                path: manifest_path.to_path_buf(),
                message: "manifest has no parent directory".into(),
            })?;

        let library_path = parent.join(&manifest.library);
        if !library_path.exists() {
            return Err(DylibError::ManifestError {
                path: manifest_path.to_path_buf(),
                message: format!("library not found: {}", library_path.display()),
            });
        }

        DylibNodeHandler::load(&library_path)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node_ctx::TestNodeCtx;
    use serde_json::json;
    use std::collections::BTreeMap;
    use tempfile::TempDir;

    // -- Trampoline tests (test the C ABI callback layer directly) -----------

    #[tokio::test]
    async fn test_callback_vtable_construction() {
        let (ctx, _inspector) = TestNodeCtx::builder()
            .run_id("run-1")
            .node_id("node-1")
            .build();

        let runtime = tokio::runtime::Handle::current();
        let mut cb = CallbackContext::new(ctx, runtime);
        let vtable = cb.as_vtable();

        // All function pointers should be non-null (they point to our trampoline fns).
        assert!(!vtable.opaque.is_null());
        // The function pointers themselves can't be null — they're fn items.
    }

    #[tokio::test]
    async fn test_emit_trampoline() {
        let (ctx, inspector) = TestNodeCtx::builder()
            .run_id("run-1")
            .node_id("node-1")
            .build();

        let runtime = tokio::runtime::Handle::current();
        let mut cb = CallbackContext::new(ctx, runtime);
        let vtable = cb.as_vtable();

        let name = CString::new("test.event").unwrap();
        let data = CString::new(r#"{"key":"value"}"#).unwrap();

        let result = unsafe { (vtable.emit)(vtable.opaque, name.as_ptr(), data.as_ptr()) };
        assert_eq!(result, 0);

        let events = inspector.emitted_events().await;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].name, "test.event");
        assert_eq!(events[0].data, json!({"key": "value"}));
    }

    #[tokio::test]
    async fn test_record_llm_call_trampoline() {
        let (ctx, inspector) = TestNodeCtx::builder()
            .run_id("run-1")
            .node_id("llm-node")
            .build();

        let runtime = tokio::runtime::Handle::current();

        // Trampolines call runtime.block_on(), so they must run on a
        // blocking thread (just like in production via spawn_blocking).
        let result = tokio::task::spawn_blocking(move || {
            let mut cb = CallbackContext::new(ctx, runtime);
            let vtable = cb.as_vtable();

            let request = LlmRequest {
                provider: "openai".into(),
                model: "gpt-4o".into(),
                messages: json!([{"role": "user", "content": "hello"}]),
                tools: None,
                temperature: Some(0.7),
                top_p: None,
                max_tokens: Some(4096),
                stop_sequences: None,
                response_format: None,
                seed: None,
                extra_params: BTreeMap::new(),
            };
            let response = LlmResponse {
                content: json!("Hello!"),
                tool_calls: None,
                model_used: "gpt-4o".into(),
                input_tokens: Some(10),
                output_tokens: Some(5),
                total_tokens: Some(15),
                finish_reason: "stop".into(),
                latency_ms: 200,
                provider_request_id: None,
                cost: None,
            };

            let req_json = CString::new(serde_json::to_string(&request).unwrap()).unwrap();
            let resp_json = CString::new(serde_json::to_string(&response).unwrap()).unwrap();

            unsafe {
                (vtable.record_llm_call)(vtable.opaque, req_json.as_ptr(), resp_json.as_ptr())
            }
        })
        .await
        .unwrap();

        assert_eq!(result, 0);

        let calls = inspector.recorded_llm_calls().await;
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].request.model, "gpt-4o");
        assert_eq!(calls[0].response.finish_reason, "stop");
        assert_eq!(calls[0].run_id, "run-1");
        assert_eq!(calls[0].node_id, "llm-node");
    }

    #[tokio::test]
    async fn test_secret_get_trampoline() {
        let (ctx, _inspector) = TestNodeCtx::builder()
            .run_id("run-1")
            .node_id("node-1")
            .secret("API_KEY", "sk-test-123")
            .build();

        let runtime = tokio::runtime::Handle::current();

        // Trampolines call runtime.block_on(), must run on blocking thread.
        let (found_value, missing_is_null) = tokio::task::spawn_blocking(move || {
            let mut cb = CallbackContext::new(ctx, runtime);
            let vtable = cb.as_vtable();

            // Found key.
            let key = CString::new("API_KEY").unwrap();
            let ptr = unsafe { (vtable.secret_get)(vtable.opaque, key.as_ptr()) };
            assert!(!ptr.is_null());
            let value = unsafe { CStr::from_ptr(ptr) }.to_str().unwrap().to_string();

            // Missing key — returns null.
            let missing_key = CString::new("MISSING").unwrap();
            let ptr = unsafe { (vtable.secret_get)(vtable.opaque, missing_key.as_ptr()) };
            let is_null = ptr.is_null();

            (value, is_null)
        })
        .await
        .unwrap();

        assert_eq!(found_value, "sk-test-123");
        assert!(missing_is_null);
    }

    #[tokio::test]
    async fn test_state_roundtrip_trampoline() {
        let (ctx, _inspector) = TestNodeCtx::builder()
            .run_id("run-1")
            .node_id("node-1")
            .build();

        let runtime = tokio::runtime::Handle::current();

        // Trampolines call runtime.block_on(), must run on blocking thread.
        let retrieved = tokio::task::spawn_blocking(move || {
            let mut cb = CallbackContext::new(ctx, runtime);
            let vtable = cb.as_vtable();

            // Set state.
            let key = CString::new("counter").unwrap();
            let value = CString::new("42").unwrap();
            let result = unsafe { (vtable.state_set)(vtable.opaque, key.as_ptr(), value.as_ptr()) };
            assert_eq!(result, 0);

            // Get state.
            let ptr = unsafe { (vtable.state_get)(vtable.opaque, key.as_ptr()) };
            assert!(!ptr.is_null());
            unsafe { CStr::from_ptr(ptr) }.to_str().unwrap().to_string()
        })
        .await
        .unwrap();

        assert_eq!(retrieved, "42");
    }

    // -- Manifest / loader tests ---------------------------------------------

    #[test]
    fn test_manifest_parsing() {
        let dir = TempDir::new().unwrap();
        let node_dir = dir.path().join("my_node");
        std::fs::create_dir(&node_dir).unwrap();

        let manifest = json!({
            "node_type": "my_custom_node",
            "library": "libmy_node.so",
            "description": "A test node"
        });
        std::fs::write(node_dir.join("node.json"), manifest.to_string()).unwrap();

        // Loading will fail because there's no real .so, but manifest parsing should
        // succeed — the error should be about the missing library file, not manifest parsing.
        let result = DylibNodeLoader::scan(dir.path());
        // The scan should succeed (returns empty vec) because individual load failures
        // are logged as warnings and skipped.
        let handlers = result.unwrap();
        assert_eq!(handlers.len(), 0); // No real .so file to load
    }

    #[test]
    fn test_manifest_missing_fields() {
        let dir = TempDir::new().unwrap();
        let node_dir = dir.path().join("bad_node");
        std::fs::create_dir(&node_dir).unwrap();

        // Missing required "library" field.
        let manifest = json!({ "node_type": "bad" });
        std::fs::write(node_dir.join("node.json"), manifest.to_string()).unwrap();

        let handlers = DylibNodeLoader::scan(dir.path()).unwrap();
        assert_eq!(handlers.len(), 0); // Skipped due to manifest error
    }

    #[test]
    fn test_scan_skips_non_directories() {
        let dir = TempDir::new().unwrap();

        // Create a regular file — should be skipped.
        std::fs::write(dir.path().join("not_a_dir.json"), "{}").unwrap();

        // Create a subdirectory without node.json — should be skipped.
        let sub = dir.path().join("empty_dir");
        std::fs::create_dir(&sub).unwrap();

        // Create a valid directory structure but with invalid library path.
        let valid_dir = dir.path().join("valid_node");
        std::fs::create_dir(&valid_dir).unwrap();
        let manifest = json!({
            "node_type": "test",
            "library": "libtest.so"
        });
        std::fs::write(valid_dir.join("node.json"), manifest.to_string()).unwrap();

        let handlers = DylibNodeLoader::scan(dir.path()).unwrap();
        // No real .so files, so all handlers fail to load — but scan itself succeeds.
        assert_eq!(handlers.len(), 0);
    }

    #[test]
    fn test_manifest_invalid_json() {
        let dir = TempDir::new().unwrap();
        let node_dir = dir.path().join("invalid_json");
        std::fs::create_dir(&node_dir).unwrap();

        std::fs::write(node_dir.join("node.json"), "not valid json {{{").unwrap();

        let handlers = DylibNodeLoader::scan(dir.path()).unwrap();
        assert_eq!(handlers.len(), 0);
    }
}
