//! C ABI functions for engine orchestration (EngineBuilder → Engine → ExecutionHandle).
//!
//! Handle-based API matching the existing observer/adapter pattern.
//! All structured data is passed as JSON strings.

use std::collections::HashMap;
use std::os::raw::c_char;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, OnceLock};

use cleargate_flow_engine::engine::{Engine, EngineBuilder};
use cleargate_flow_engine::executor::ExecutorConfig;
use cleargate_flow_engine::types::GraphDef;
use cleargate_flow_engine::write_event::WriteEvent;
use tokio::sync::{broadcast, oneshot};

use super::{block_on, cstr_to_str, set_last_error, string_to_c, NEXT_HANDLE};

// ---------------------------------------------------------------------------
// Handle storage
// ---------------------------------------------------------------------------

struct EngineBuilderWrapper {
    builder: Option<EngineBuilder>,
}

struct EngineWrapper {
    engine: Arc<Engine>,
}

struct ExecutionWrapper {
    run_id: String,
    events: broadcast::Receiver<WriteEvent>,
    cancel: Option<oneshot::Sender<()>>,
}

fn engine_builders() -> &'static Mutex<HashMap<u64, EngineBuilderWrapper>> {
    static BUILDERS: OnceLock<Mutex<HashMap<u64, EngineBuilderWrapper>>> = OnceLock::new();
    BUILDERS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn engines() -> &'static Mutex<HashMap<u64, EngineWrapper>> {
    static ENGINES: OnceLock<Mutex<HashMap<u64, EngineWrapper>>> = OnceLock::new();
    ENGINES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn executions() -> &'static Mutex<HashMap<u64, ExecutionWrapper>> {
    static EXECUTIONS: OnceLock<Mutex<HashMap<u64, ExecutionWrapper>>> = OnceLock::new();
    EXECUTIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

// ---------------------------------------------------------------------------
// EngineBuilder FFI
// ---------------------------------------------------------------------------

/// Create a new engine builder. Returns a handle (>0) on success, 0 on error.
#[no_mangle]
pub extern "C" fn cleargate_engine_builder_new() -> u64 {
    let handle = NEXT_HANDLE.fetch_add(1, Ordering::Relaxed);
    engine_builders().lock().unwrap().insert(
        handle,
        EngineBuilderWrapper {
            builder: Some(Engine::builder()),
        },
    );
    handle
}

/// Set the flow store directory path. Returns 0 on success, -1 on error.
///
/// # Safety
/// `path` must be a valid null-terminated C string.
#[no_mangle]
pub unsafe extern "C" fn cleargate_engine_builder_flow_store_path(
    handle: u64,
    path: *const c_char,
) -> i32 {
    let Some(path_str) = cstr_to_str(path) else {
        return -1;
    };

    let mut builders = engine_builders().lock().unwrap();
    let Some(wrapper) = builders.get_mut(&handle) else {
        set_last_error("invalid builder handle".into());
        return -1;
    };
    let Some(builder) = wrapper.builder.take() else {
        set_last_error("builder already consumed".into());
        return -1;
    };

    match cleargate_flow_engine::defaults::FileFlowStore::new(PathBuf::from(path_str)) {
        Ok(store) => {
            wrapper.builder = Some(builder.flow_store(store));
            0
        }
        Err(e) => {
            set_last_error(format!("flow store error: {e}"));
            wrapper.builder = Some(builder); // put it back
            -1
        }
    }
}

/// Set the run store directory path. Returns 0 on success, -1 on error.
///
/// # Safety
/// `path` must be a valid null-terminated C string.
#[no_mangle]
pub unsafe extern "C" fn cleargate_engine_builder_run_store_path(
    handle: u64,
    path: *const c_char,
) -> i32 {
    let Some(path_str) = cstr_to_str(path) else {
        return -1;
    };

    let mut builders = engine_builders().lock().unwrap();
    let Some(wrapper) = builders.get_mut(&handle) else {
        set_last_error("invalid builder handle".into());
        return -1;
    };
    let Some(builder) = wrapper.builder.take() else {
        set_last_error("builder already consumed".into());
        return -1;
    };

    match cleargate_flow_engine::defaults::FileRunStore::new(PathBuf::from(path_str)) {
        Ok(store) => {
            wrapper.builder = Some(builder.run_store(store));
            0
        }
        Err(e) => {
            set_last_error(format!("run store error: {e}"));
            wrapper.builder = Some(builder);
            -1
        }
    }
}

/// Set the run store database URL. Returns 0 on success, -1 on error.
///
/// # Safety
/// `url` must be a valid null-terminated C string.
#[no_mangle]
pub unsafe extern "C" fn cleargate_engine_builder_run_store_url(
    handle: u64,
    url: *const c_char,
) -> i32 {
    let Some(url_str) = cstr_to_str(url) else {
        return -1;
    };

    let mut builders = engine_builders().lock().unwrap();
    let Some(wrapper) = builders.get_mut(&handle) else {
        set_last_error("invalid builder handle".into());
        return -1;
    };
    let Some(builder) = wrapper.builder.take() else {
        set_last_error("builder already consumed".into());
        return -1;
    };

    let db = match block_on(cleargate_storage_oss::connect(url_str)) {
        Ok(db) => db,
        Err(e) => {
            set_last_error(format!("DB connect failed: {e}"));
            wrapper.builder = Some(builder);
            return -1;
        }
    };
    if let Err(e) = block_on(cleargate_storage_oss::run_migrations(&db)) {
        set_last_error(format!("DB migration failed: {e}"));
        wrapper.builder = Some(builder);
        return -1;
    }

    let store = cleargate_storage_oss::SeaOrmRunStore::new(Arc::new(db));
    wrapper.builder = Some(builder.run_store(store));
    0
}

/// Set max traversals for cycle detection. Returns 0 on success, -1 on error.
#[no_mangle]
pub extern "C" fn cleargate_engine_builder_max_traversals(handle: u64, value: u32) -> i32 {
    let mut builders = engine_builders().lock().unwrap();
    let Some(wrapper) = builders.get_mut(&handle) else {
        set_last_error("invalid builder handle".into());
        return -1;
    };
    let Some(builder) = wrapper.builder.take() else {
        set_last_error("builder already consumed".into());
        return -1;
    };

    wrapper.builder = Some(builder.executor_config(ExecutorConfig {
        max_traversals: value,
    }));
    0
}

/// Enable or disable crash recovery. Returns 0 on success, -1 on error.
#[no_mangle]
pub extern "C" fn cleargate_engine_builder_crash_recovery(handle: u64, enabled: bool) -> i32 {
    let mut builders = engine_builders().lock().unwrap();
    let Some(wrapper) = builders.get_mut(&handle) else {
        set_last_error("invalid builder handle".into());
        return -1;
    };
    let Some(builder) = wrapper.builder.take() else {
        set_last_error("builder already consumed".into());
        return -1;
    };

    wrapper.builder = Some(builder.crash_recovery(enabled));
    0
}

/// Build the engine. Returns an engine handle (>0) on success, 0 on error.
/// Consumes the builder.
#[no_mangle]
pub extern "C" fn cleargate_engine_builder_build(builder_handle: u64) -> u64 {
    let builder = {
        let mut builders = engine_builders().lock().unwrap();
        let Some(wrapper) = builders.get_mut(&builder_handle) else {
            set_last_error("invalid builder handle".into());
            return 0;
        };
        match wrapper.builder.take() {
            Some(b) => b,
            None => {
                set_last_error("builder already consumed".into());
                return 0;
            }
        }
    };
    // Remove the builder entry since it's consumed.
    engine_builders().lock().unwrap().remove(&builder_handle);

    let engine = match block_on(builder.build()) {
        Ok(e) => e,
        Err(e) => {
            set_last_error(format!("engine build failed: {e}"));
            return 0;
        }
    };

    let handle = NEXT_HANDLE.fetch_add(1, Ordering::Relaxed);
    engines().lock().unwrap().insert(
        handle,
        EngineWrapper {
            engine: Arc::new(engine),
        },
    );
    handle
}

/// Destroy a builder handle without building.
#[no_mangle]
pub extern "C" fn cleargate_engine_builder_destroy(handle: u64) {
    engine_builders().lock().unwrap().remove(&handle);
}

// ---------------------------------------------------------------------------
// Engine FFI
// ---------------------------------------------------------------------------

/// Execute a flow by ID. Returns an execution handle (>0) on success, 0 on error.
///
/// # Safety
/// `flow_id` must be a valid null-terminated C string. `inputs_json` may be null
/// (defaults to `{}`).
#[no_mangle]
pub unsafe extern "C" fn cleargate_engine_execute(
    engine_handle: u64,
    flow_id: *const c_char,
    inputs_json: *const c_char,
) -> u64 {
    let Some(flow_id_str) = cstr_to_str(flow_id) else {
        set_last_error("invalid flow_id".into());
        return 0;
    };

    let inputs: serde_json::Value = if let Some(json_str) = cstr_to_str(inputs_json) {
        match serde_json::from_str(json_str) {
            Ok(v) => v,
            Err(e) => {
                set_last_error(format!("invalid inputs JSON: {e}"));
                return 0;
            }
        }
    } else {
        serde_json::json!({})
    };

    let engine = {
        let guard = engines().lock().unwrap();
        let Some(wrapper) = guard.get(&engine_handle) else {
            set_last_error("invalid engine handle".into());
            return 0;
        };
        Arc::clone(&wrapper.engine)
    };

    let exec_handle = match block_on(engine.execute(flow_id_str, inputs, None)) {
        Ok(h) => h,
        Err(e) => {
            set_last_error(format!("execute failed: {e}"));
            return 0;
        }
    };

    let handle = NEXT_HANDLE.fetch_add(1, Ordering::Relaxed);
    executions().lock().unwrap().insert(
        handle,
        ExecutionWrapper {
            run_id: exec_handle.run_id,
            events: exec_handle.events,
            cancel: Some(exec_handle.cancel),
        },
    );
    handle
}

/// Execute a graph definition directly. Returns an execution handle (>0) on success, 0 on error.
///
/// # Safety
/// `graph_json` and `inputs_json` must be valid null-terminated C strings. `inputs_json` may be null.
#[no_mangle]
pub unsafe extern "C" fn cleargate_engine_execute_graph(
    engine_handle: u64,
    graph_json: *const c_char,
    inputs_json: *const c_char,
) -> u64 {
    let Some(graph_str) = cstr_to_str(graph_json) else {
        set_last_error("invalid graph_json".into());
        return 0;
    };

    let graph: GraphDef = match serde_json::from_str(graph_str) {
        Ok(g) => g,
        Err(e) => {
            set_last_error(format!("invalid graph JSON: {e}"));
            return 0;
        }
    };

    let inputs: serde_json::Value = if let Some(json_str) = cstr_to_str(inputs_json) {
        match serde_json::from_str(json_str) {
            Ok(v) => v,
            Err(e) => {
                set_last_error(format!("invalid inputs JSON: {e}"));
                return 0;
            }
        }
    } else {
        serde_json::json!({})
    };

    let engine = {
        let guard = engines().lock().unwrap();
        let Some(wrapper) = guard.get(&engine_handle) else {
            set_last_error("invalid engine handle".into());
            return 0;
        };
        Arc::clone(&wrapper.engine)
    };

    let exec_handle = match block_on(engine.execute_graph(
        &graph,
        "dotnet-adhoc",
        inputs,
        cleargate_flow_engine::types::TriggerSource::Api {
            request_id: uuid::Uuid::new_v4().to_string(),
        },
        None,
    )) {
        Ok(h) => h,
        Err(e) => {
            set_last_error(format!("execute_graph failed: {e}"));
            return 0;
        }
    };

    let handle = NEXT_HANDLE.fetch_add(1, Ordering::Relaxed);
    executions().lock().unwrap().insert(
        handle,
        ExecutionWrapper {
            run_id: exec_handle.run_id,
            events: exec_handle.events,
            cancel: Some(exec_handle.cancel),
        },
    );
    handle
}

/// Deliver input to a node waiting for human-in-the-loop. Returns 0 on success, -1 on error.
///
/// # Safety
/// All string pointers must be valid null-terminated C strings.
#[no_mangle]
pub unsafe extern "C" fn cleargate_engine_provide_input(
    engine_handle: u64,
    run_id: *const c_char,
    node_id: *const c_char,
    input_json: *const c_char,
) -> i32 {
    let (Some(run_id_str), Some(node_id_str), Some(input_str)) = (
        cstr_to_str(run_id),
        cstr_to_str(node_id),
        cstr_to_str(input_json),
    ) else {
        return -1;
    };

    let Ok(input_val) = serde_json::from_str::<serde_json::Value>(input_str) else {
        set_last_error("invalid input JSON".into());
        return -1;
    };

    let engine = {
        let guard = engines().lock().unwrap();
        let Some(wrapper) = guard.get(&engine_handle) else {
            set_last_error("invalid engine handle".into());
            return -1;
        };
        Arc::clone(&wrapper.engine)
    };

    match block_on(engine.provide_input(run_id_str, node_id_str, input_val)) {
        Ok(()) => 0,
        Err(e) => {
            set_last_error(format!("provide_input failed: {e}"));
            -1
        }
    }
}

/// Get the node catalog as a JSON array. Caller must free with `cleargate_free_string`.
#[no_mangle]
pub extern "C" fn cleargate_engine_node_catalog(engine_handle: u64) -> *mut c_char {
    let engine = {
        let guard = engines().lock().unwrap();
        let Some(wrapper) = guard.get(&engine_handle) else {
            set_last_error("invalid engine handle".into());
            return std::ptr::null_mut();
        };
        Arc::clone(&wrapper.engine)
    };

    let catalog = engine.node_catalog();
    match serde_json::to_string(&catalog) {
        Ok(s) => string_to_c(s),
        Err(e) => {
            set_last_error(format!("serialize failed: {e}"));
            std::ptr::null_mut()
        }
    }
}

/// Shut down the engine gracefully.
#[no_mangle]
pub extern "C" fn cleargate_engine_shutdown(engine_handle: u64) {
    let engine = {
        let guard = engines().lock().unwrap();
        let Some(wrapper) = guard.get(&engine_handle) else {
            return;
        };
        Arc::clone(&wrapper.engine)
    };
    block_on(engine.shutdown());
}

/// Destroy an engine handle.
#[no_mangle]
pub extern "C" fn cleargate_engine_destroy(engine_handle: u64) {
    engines().lock().unwrap().remove(&engine_handle);
}

// ---------------------------------------------------------------------------
// Execution FFI
// ---------------------------------------------------------------------------

/// Get the run ID for an execution. Caller must free with `cleargate_free_string`.
#[no_mangle]
pub extern "C" fn cleargate_execution_get_run_id(handle: u64) -> *mut c_char {
    let guard = executions().lock().unwrap();
    let Some(wrapper) = guard.get(&handle) else {
        return std::ptr::null_mut();
    };
    string_to_c(wrapper.run_id.clone())
}

/// Poll for the next event. Returns a JSON string or null if the stream is closed.
/// Caller must free with `cleargate_free_string`.
#[no_mangle]
pub extern "C" fn cleargate_execution_next_event(handle: u64) -> *mut c_char {
    let mut guard = executions().lock().unwrap();
    let Some(wrapper) = guard.get_mut(&handle) else {
        return std::ptr::null_mut();
    };

    loop {
        match block_on(wrapper.events.recv()) {
            Ok(event) => {
                drop(guard);
                return match serde_json::to_string(&event) {
                    Ok(s) => string_to_c(s),
                    Err(e) => {
                        set_last_error(format!("serialize failed: {e}"));
                        std::ptr::null_mut()
                    }
                };
            }
            Err(broadcast::error::RecvError::Closed) => {
                return std::ptr::null_mut();
            }
            Err(broadcast::error::RecvError::Lagged(_)) => {
                continue;
            }
        }
    }
}

/// Cancel a running execution. Returns 0 on success, -1 on error.
#[no_mangle]
pub extern "C" fn cleargate_execution_cancel(handle: u64) -> i32 {
    let mut guard = executions().lock().unwrap();
    let Some(wrapper) = guard.get_mut(&handle) else {
        set_last_error("invalid execution handle".into());
        return -1;
    };
    match wrapper.cancel.take() {
        Some(sender) => {
            let _ = sender.send(());
            0
        }
        None => {
            set_last_error("already cancelled".into());
            -1
        }
    }
}

/// Block until the execution completes. Returns all events as a JSON array.
/// Caller must free with `cleargate_free_string`.
#[no_mangle]
pub extern "C" fn cleargate_execution_wait(handle: u64) -> *mut c_char {
    let mut all_events: Vec<serde_json::Value> = Vec::new();

    loop {
        let mut guard = executions().lock().unwrap();
        let Some(wrapper) = guard.get_mut(&handle) else {
            break;
        };
        match block_on(wrapper.events.recv()) {
            Ok(event) => {
                drop(guard);
                if let Ok(val) = serde_json::to_value(&event) {
                    all_events.push(val);
                }
            }
            Err(broadcast::error::RecvError::Closed) => break,
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
        }
    }

    match serde_json::to_string(&all_events) {
        Ok(s) => string_to_c(s),
        Err(e) => {
            set_last_error(format!("serialize failed: {e}"));
            std::ptr::null_mut()
        }
    }
}

/// Destroy an execution handle.
#[no_mangle]
pub extern "C" fn cleargate_execution_destroy(handle: u64) {
    executions().lock().unwrap().remove(&handle);
}
