//! Cleargate .NET bindings via C ABI (P/Invoke).
//!
//! Exposes `extern "C"` functions for observer and adapter sessions,
//! plus engine orchestration (behind the `engine` feature).
//! Uses `OnceLock<Runtime>` + `block_on()` pattern (same as Python crate).
//! Handle-based session management via `HashMap<u64, Wrapper>` behind `Mutex`.

#[cfg(feature = "engine")]
mod engine_ffi;

use std::collections::{BTreeMap, HashMap};
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use cleargate_flow_engine::write_event::WriteEvent;

use cleargate_adapters::{
    AdapterSession, FrameworkAdapter, LangChainAdapter, LangGraphAdapter, SemanticKernelAdapter,
};
use cleargate_flow_engine::defaults::InMemoryRunStore;
use cleargate_flow_engine::observer::{ObserverConfig, ObserverSession};
use cleargate_flow_engine::traits::RunStore;
use cleargate_flow_engine::types::{LlmRequest, LlmResponse, RunStatus};

// ---------------------------------------------------------------------------
// Runtime
// ---------------------------------------------------------------------------

static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

pub(crate) fn runtime() -> &'static tokio::runtime::Runtime {
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("failed to create tokio runtime")
    })
}

pub(crate) fn block_on<F: std::future::Future>(fut: F) -> F::Output {
    runtime().block_on(fut)
}

// ---------------------------------------------------------------------------
// Handle storage
// ---------------------------------------------------------------------------

pub(crate) static NEXT_HANDLE: AtomicU64 = AtomicU64::new(1);

fn observer_sessions() -> &'static Mutex<HashMap<u64, ObserverWrapper>> {
    static SESSIONS: OnceLock<Mutex<HashMap<u64, ObserverWrapper>>> = OnceLock::new();
    SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn adapter_sessions() -> &'static Mutex<HashMap<u64, AdapterWrapper>> {
    static SESSIONS: OnceLock<Mutex<HashMap<u64, AdapterWrapper>>> = OnceLock::new();
    SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

struct ObserverWrapper {
    session: Option<ObserverSession>,
    store: Arc<dyn RunStore>,
    run_id: String,
}

struct AdapterWrapper {
    session: Option<AdapterSession>,
    store: Arc<dyn RunStore>,
    run_id: String,
}

// ---------------------------------------------------------------------------
// Thread-local error
// ---------------------------------------------------------------------------

thread_local! {
    static LAST_ERROR: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

pub(crate) fn set_last_error(msg: String) {
    LAST_ERROR.with(|e| *e.borrow_mut() = Some(msg));
}

/// Get the last error message. Caller must free with `cleargate_free_string`.
/// Returns null if no error has been recorded on this thread.
///
/// # Safety
/// The returned pointer must be freed with `cleargate_free_string`.
#[no_mangle]
pub extern "C" fn cleargate_last_error() -> *mut c_char {
    LAST_ERROR.with(|e| match e.borrow_mut().take() {
        Some(msg) => string_to_c(msg),
        None => std::ptr::null_mut(),
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Convert a C string pointer to a `&str`. Returns `None` if null or invalid UTF-8.
pub(crate) unsafe fn cstr_to_str<'a>(ptr: *const c_char) -> Option<&'a str> {
    if ptr.is_null() {
        return None;
    }
    CStr::from_ptr(ptr).to_str().ok()
}

fn status_from_str(s: &str) -> RunStatus {
    match s {
        "completed" => RunStatus::Completed,
        "failed" => RunStatus::Failed,
        "cancelled" => RunStatus::Cancelled,
        _ => RunStatus::Completed,
    }
}

pub(crate) fn string_to_c(s: String) -> *mut c_char {
    match CString::new(s) {
        Ok(cs) => cs.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

fn json_to_llm_request(raw: &serde_json::Value) -> LlmRequest {
    LlmRequest {
        provider: raw
            .get("provider")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .into(),
        model: raw
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .into(),
        messages: raw
            .get("messages")
            .cloned()
            .unwrap_or_else(|| serde_json::json!([])),
        tools: raw.get("tools").and_then(|v| v.as_array().cloned()),
        temperature: raw.get("temperature").and_then(|v| v.as_f64()),
        top_p: raw.get("top_p").and_then(|v| v.as_f64()),
        max_tokens: raw
            .get("max_tokens")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32),
        stop_sequences: None,
        response_format: raw.get("response_format").cloned(),
        seed: raw.get("seed").and_then(|v| v.as_u64()),
        extra_params: BTreeMap::new(),
    }
}

fn json_to_llm_response(raw: &serde_json::Value) -> LlmResponse {
    LlmResponse {
        content: raw
            .get("content")
            .cloned()
            .unwrap_or_else(|| serde_json::json!("")),
        tool_calls: None,
        model_used: raw
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .into(),
        input_tokens: raw
            .get("input_tokens")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32),
        output_tokens: raw
            .get("output_tokens")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32),
        total_tokens: raw
            .get("total_tokens")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32),
        finish_reason: raw
            .get("finish_reason")
            .and_then(|v| v.as_str())
            .unwrap_or("stop")
            .into(),
        latency_ms: raw.get("latency_ms").and_then(|v| v.as_u64()).unwrap_or(0),
        provider_request_id: None,
        cost: None,
    }
}

// ---------------------------------------------------------------------------
// Observer functions
// ---------------------------------------------------------------------------

/// Start an observer session. Returns a handle (>0) on success, 0 on error.
///
/// # Safety
/// `name` must be a valid null-terminated C string. `store_path` may be null.
#[no_mangle]
pub unsafe extern "C" fn cleargate_observer_start(
    name: *const c_char,
    _store_path: *const c_char,
) -> u64 {
    let Some(name_str) = cstr_to_str(name) else {
        return 0;
    };

    let store: Arc<dyn RunStore> = if let Some(url) = cstr_to_str(_store_path) {
        let db = match block_on(cleargate_storage_oss::connect(url)) {
            Ok(db) => db,
            Err(e) => {
                set_last_error(format!("DB connect failed: {e}"));
                return 0;
            }
        };
        if let Err(e) = block_on(cleargate_storage_oss::run_migrations(&db)) {
            set_last_error(format!("DB migration failed: {e}"));
            return 0;
        }
        Arc::new(cleargate_storage_oss::SeaOrmRunStore::new(Arc::new(db)))
    } else {
        Arc::new(InMemoryRunStore::new())
    };
    let config = ObserverConfig {
        run_store: Some(store.clone()),
        ..Default::default()
    };

    let (session, _handle) = match block_on(ObserverSession::start(name_str, config)) {
        Ok(v) => v,
        Err(e) => {
            set_last_error(format!("observer start failed: {e}"));
            return 0;
        }
    };

    let run_id = session.run_id();
    let handle = NEXT_HANDLE.fetch_add(1, Ordering::Relaxed);

    observer_sessions().lock().unwrap().insert(
        handle,
        ObserverWrapper {
            session: Some(session),
            store,
            run_id,
        },
    );

    handle
}

/// Record an LLM call. Returns 0 on success, -1 on error.
///
/// # Safety
/// All string pointers must be valid null-terminated C strings or null.
#[no_mangle]
pub unsafe extern "C" fn cleargate_observer_record_llm_call(
    handle: u64,
    node_id: *const c_char,
    request_json: *const c_char,
    response_json: *const c_char,
) -> i32 {
    let (Some(node_id_str), Some(req_str), Some(resp_str)) = (
        cstr_to_str(node_id),
        cstr_to_str(request_json),
        cstr_to_str(response_json),
    ) else {
        return -1;
    };

    let Ok(req_val) = serde_json::from_str::<serde_json::Value>(req_str) else {
        return -1;
    };
    let Ok(resp_val) = serde_json::from_str::<serde_json::Value>(resp_str) else {
        return -1;
    };

    let req = json_to_llm_request(&req_val);
    let resp = json_to_llm_response(&resp_val);

    let sessions = observer_sessions().lock().unwrap();
    let Some(wrapper) = sessions.get(&handle) else {
        return -1;
    };
    let Some(session) = wrapper.session.as_ref() else {
        return -1;
    };

    // Take a pointer and drop the guard to avoid holding lock across block_on.
    let session_ptr = session as *const ObserverSession;
    drop(sessions);

    match block_on(unsafe { &*session_ptr }.record_llm_call_for_node(node_id_str, req, resp)) {
        Ok(()) => 0,
        Err(e) => {
            set_last_error(format!("record_llm_call failed: {e}"));
            -1
        }
    }
}

/// Record a tool call. Returns 0 on success, -1 on error.
///
/// # Safety
/// All string pointers must be valid null-terminated C strings or null.
#[no_mangle]
pub unsafe extern "C" fn cleargate_observer_record_tool_call(
    handle: u64,
    tool_name: *const c_char,
    inputs_json: *const c_char,
    outputs_json: *const c_char,
    duration_ms: u64,
) -> i32 {
    let (Some(tool_str), Some(in_str), Some(out_str)) = (
        cstr_to_str(tool_name),
        cstr_to_str(inputs_json),
        cstr_to_str(outputs_json),
    ) else {
        return -1;
    };

    let Ok(inputs_val) = serde_json::from_str::<serde_json::Value>(in_str) else {
        return -1;
    };
    let Ok(outputs_val) = serde_json::from_str::<serde_json::Value>(out_str) else {
        return -1;
    };

    let sessions = observer_sessions().lock().unwrap();
    let Some(wrapper) = sessions.get(&handle) else {
        return -1;
    };
    let Some(session) = wrapper.session.as_ref() else {
        return -1;
    };

    let session_ptr = session as *const ObserverSession;
    drop(sessions);

    match block_on(unsafe { &*session_ptr }.record_tool_call(
        tool_str,
        inputs_val,
        outputs_val,
        duration_ms,
    )) {
        Ok(()) => 0,
        Err(e) => {
            set_last_error(format!("record_tool_call failed: {e}"));
            -1
        }
    }
}

/// Record a named step. Returns 0 on success, -1 on error.
///
/// # Safety
/// All string pointers must be valid null-terminated C strings or null.
#[no_mangle]
pub unsafe extern "C" fn cleargate_observer_record_step(
    handle: u64,
    step_name: *const c_char,
    data_json: *const c_char,
) -> i32 {
    let (Some(step_str), Some(data_str)) = (cstr_to_str(step_name), cstr_to_str(data_json)) else {
        return -1;
    };

    let Ok(data_val) = serde_json::from_str::<serde_json::Value>(data_str) else {
        return -1;
    };

    let sessions = observer_sessions().lock().unwrap();
    let Some(wrapper) = sessions.get(&handle) else {
        return -1;
    };
    let Some(session) = wrapper.session.as_ref() else {
        return -1;
    };

    let session_ptr = session as *const ObserverSession;
    drop(sessions);

    match block_on(unsafe { &*session_ptr }.record_step(step_str, data_val)) {
        Ok(()) => 0,
        Err(e) => {
            set_last_error(format!("record_step failed: {e}"));
            -1
        }
    }
}

/// Finish the observer session. Returns 0 on success, -1 on error.
///
/// # Safety
/// `status` must be a valid null-terminated C string or null.
#[no_mangle]
pub unsafe extern "C" fn cleargate_observer_finish(handle: u64, status: *const c_char) -> i32 {
    let status_str = cstr_to_str(status).unwrap_or("completed");
    let run_status = status_from_str(status_str);

    let session = {
        let mut sessions = observer_sessions().lock().unwrap();
        let Some(wrapper) = sessions.get_mut(&handle) else {
            return -1;
        };
        match wrapper.session.take() {
            Some(s) => s,
            None => return -1,
        }
    };

    match block_on(session.finish(run_status)) {
        Ok(()) => 0,
        Err(e) => {
            set_last_error(format!("observer finish failed: {e}"));
            -1
        }
    }
}

/// Get run data as a JSON string. Caller must free with `cleargate_free_string`.
/// Returns null on error.
///
/// # Safety
/// `handle` must be a valid observer handle.
#[no_mangle]
pub extern "C" fn cleargate_observer_get_run_data(handle: u64) -> *mut c_char {
    let sessions = observer_sessions().lock().unwrap();
    let Some(wrapper) = sessions.get(&handle) else {
        return std::ptr::null_mut();
    };
    let store = wrapper.store.clone();
    let run_id = wrapper.run_id.clone();
    drop(sessions);

    let Ok(Some(record)) = block_on(store.get_run(&run_id)) else {
        return std::ptr::null_mut();
    };

    let Ok(json_str) = serde_json::to_string(&record) else {
        return std::ptr::null_mut();
    };

    string_to_c(json_str)
}

/// Get the run ID as a string. Caller must free with `cleargate_free_string`.
/// Returns null on error.
///
/// # Safety
/// `handle` must be a valid observer handle.
#[no_mangle]
pub extern "C" fn cleargate_observer_get_run_id(handle: u64) -> *mut c_char {
    let sessions = observer_sessions().lock().unwrap();
    let Some(wrapper) = sessions.get(&handle) else {
        return std::ptr::null_mut();
    };
    let run_id = wrapper.run_id.clone();
    drop(sessions);

    string_to_c(run_id)
}

/// Get the full event log as a JSON array. Caller must free with `cleargate_free_string`.
/// Returns null on error.
///
/// # Safety
/// `handle` must be a valid observer handle.
#[no_mangle]
pub extern "C" fn cleargate_observer_get_events(handle: u64) -> *mut c_char {
    let sessions = observer_sessions().lock().unwrap();
    let Some(wrapper) = sessions.get(&handle) else {
        set_last_error("invalid observer handle".into());
        return std::ptr::null_mut();
    };
    let store = wrapper.store.clone();
    let run_id = wrapper.run_id.clone();
    drop(sessions);

    let events: Vec<WriteEvent> = match block_on(store.events(&run_id)) {
        Ok(e) => e,
        Err(e) => {
            set_last_error(format!("events failed: {e}"));
            return std::ptr::null_mut();
        }
    };

    match serde_json::to_string(&events) {
        Ok(s) => string_to_c(s),
        Err(e) => {
            set_last_error(format!("serialize failed: {e}"));
            std::ptr::null_mut()
        }
    }
}

/// Destroy an observer handle without finishing. Use this to clean up if the
/// session will not be finished normally (e.g. on error paths).
///
/// # Safety
/// `handle` must be a valid observer handle or already removed (no-op).
#[no_mangle]
pub extern "C" fn cleargate_observer_destroy(handle: u64) {
    observer_sessions().lock().unwrap().remove(&handle);
}

// ---------------------------------------------------------------------------
// Adapter functions
// ---------------------------------------------------------------------------

/// Start an adapter session. Returns a handle (>0) on success, 0 on error.
/// `framework`: "langchain", "langgraph", or "semantic_kernel".
///
/// When `store_path` is non-null (e.g. `"sqlite://runs.db?mode=rwc"`),
/// events are persisted to the database. Otherwise uses in-memory storage.
///
/// # Safety
/// `framework`, `name`, and `store_path` must be valid null-terminated C strings or null.
#[no_mangle]
pub unsafe extern "C" fn cleargate_adapter_start(
    framework: *const c_char,
    name: *const c_char,
    store_path: *const c_char,
) -> u64 {
    let Some(fw_str) = cstr_to_str(framework) else {
        return 0;
    };

    let adapter: Box<dyn FrameworkAdapter> = match fw_str {
        "langchain" => Box::new(LangChainAdapter),
        "langgraph" => Box::new(LangGraphAdapter),
        "semantic_kernel" => Box::new(SemanticKernelAdapter),
        _ => return 0,
    };

    let session_name = cstr_to_str(name).unwrap_or(fw_str);

    let store: Arc<dyn RunStore> = if let Some(url) = cstr_to_str(store_path) {
        let db = match block_on(cleargate_storage_oss::connect(url)) {
            Ok(db) => db,
            Err(e) => {
                set_last_error(format!("DB connect failed: {e}"));
                return 0;
            }
        };
        if let Err(e) = block_on(cleargate_storage_oss::run_migrations(&db)) {
            set_last_error(format!("DB migration failed: {e}"));
            return 0;
        }
        Arc::new(cleargate_storage_oss::SeaOrmRunStore::new(Arc::new(db)))
    } else {
        Arc::new(InMemoryRunStore::new())
    };
    let config = ObserverConfig {
        run_store: Some(store.clone()),
        ..Default::default()
    };

    let (session, _handle) = match block_on(AdapterSession::start(session_name, adapter, config)) {
        Ok(v) => v,
        Err(e) => {
            set_last_error(format!("adapter start failed: {e}"));
            return 0;
        }
    };

    let run_id = session.run_id();
    let handle = NEXT_HANDLE.fetch_add(1, Ordering::Relaxed);

    adapter_sessions().lock().unwrap().insert(
        handle,
        AdapterWrapper {
            session: Some(session),
            store,
            run_id,
        },
    );

    handle
}

/// Process a framework event. Returns 0 on success, -1 on error.
///
/// # Safety
/// `event_json` must be a valid null-terminated C string.
#[no_mangle]
pub unsafe extern "C" fn cleargate_adapter_on_event(handle: u64, event_json: *const c_char) -> i32 {
    let Some(json_str) = cstr_to_str(event_json) else {
        return -1;
    };

    let Ok(json_val) = serde_json::from_str::<serde_json::Value>(json_str) else {
        return -1;
    };

    let sessions = adapter_sessions().lock().unwrap();
    let Some(wrapper) = sessions.get(&handle) else {
        return -1;
    };
    let Some(session) = wrapper.session.as_ref() else {
        return -1;
    };

    let session_ptr = session as *const AdapterSession;
    drop(sessions);

    match block_on(unsafe { &*session_ptr }.on_event(json_val)) {
        Ok(()) => 0,
        Err(e) => {
            set_last_error(format!("on_event failed: {e}"));
            -1
        }
    }
}

/// Finish the adapter session. Returns 0 on success, -1 on error.
///
/// # Safety
/// `status` must be a valid null-terminated C string or null.
#[no_mangle]
pub unsafe extern "C" fn cleargate_adapter_finish(handle: u64, status: *const c_char) -> i32 {
    let status_str = cstr_to_str(status).unwrap_or("completed");
    let run_status = status_from_str(status_str);

    let session = {
        let mut sessions = adapter_sessions().lock().unwrap();
        let Some(wrapper) = sessions.get_mut(&handle) else {
            return -1;
        };
        match wrapper.session.take() {
            Some(s) => s,
            None => return -1,
        }
    };

    match block_on(session.finish(run_status)) {
        Ok(()) => 0,
        Err(e) => {
            set_last_error(format!("adapter finish failed: {e}"));
            -1
        }
    }
}

/// Get adapter run data as a JSON string. Caller must free with `cleargate_free_string`.
/// Returns null on error.
///
/// # Safety
/// `handle` must be a valid adapter handle.
#[no_mangle]
pub extern "C" fn cleargate_adapter_get_run_data(handle: u64) -> *mut c_char {
    let sessions = adapter_sessions().lock().unwrap();
    let Some(wrapper) = sessions.get(&handle) else {
        return std::ptr::null_mut();
    };
    let store = wrapper.store.clone();
    let run_id = wrapper.run_id.clone();
    drop(sessions);

    let Ok(Some(record)) = block_on(store.get_run(&run_id)) else {
        return std::ptr::null_mut();
    };

    let Ok(json_str) = serde_json::to_string(&record) else {
        return std::ptr::null_mut();
    };

    string_to_c(json_str)
}

/// Get the adapter run ID as a string. Caller must free with `cleargate_free_string`.
/// Returns null on error.
///
/// # Safety
/// `handle` must be a valid adapter handle.
#[no_mangle]
pub extern "C" fn cleargate_adapter_get_run_id(handle: u64) -> *mut c_char {
    let sessions = adapter_sessions().lock().unwrap();
    let Some(wrapper) = sessions.get(&handle) else {
        return std::ptr::null_mut();
    };
    let run_id = wrapper.run_id.clone();
    drop(sessions);

    string_to_c(run_id)
}

/// Get the full event log as a JSON array. Caller must free with `cleargate_free_string`.
/// Returns null on error.
///
/// # Safety
/// `handle` must be a valid adapter handle.
#[no_mangle]
pub extern "C" fn cleargate_adapter_get_events(handle: u64) -> *mut c_char {
    let sessions = adapter_sessions().lock().unwrap();
    let Some(wrapper) = sessions.get(&handle) else {
        set_last_error("invalid adapter handle".into());
        return std::ptr::null_mut();
    };
    let store = wrapper.store.clone();
    let run_id = wrapper.run_id.clone();
    drop(sessions);

    let events: Vec<WriteEvent> = match block_on(store.events(&run_id)) {
        Ok(e) => e,
        Err(e) => {
            set_last_error(format!("events failed: {e}"));
            return std::ptr::null_mut();
        }
    };

    match serde_json::to_string(&events) {
        Ok(s) => string_to_c(s),
        Err(e) => {
            set_last_error(format!("serialize failed: {e}"));
            std::ptr::null_mut()
        }
    }
}

/// Destroy an adapter handle without finishing.
///
/// # Safety
/// `handle` must be a valid adapter handle or already removed (no-op).
#[no_mangle]
pub extern "C" fn cleargate_adapter_destroy(handle: u64) {
    adapter_sessions().lock().unwrap().remove(&handle);
}

// ---------------------------------------------------------------------------
// Memory management
// ---------------------------------------------------------------------------

/// Free a string previously returned by a `cleargate_*` function.
///
/// # Safety
/// `ptr` must have been allocated by `CString::into_raw()` from this library,
/// or be null (in which case this is a no-op).
#[no_mangle]
pub unsafe extern "C" fn cleargate_free_string(ptr: *mut c_char) {
    if !ptr.is_null() {
        drop(CString::from_raw(ptr));
    }
}
