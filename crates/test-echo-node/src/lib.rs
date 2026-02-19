//! Test dylib node for integration testing.
//!
//! Implements the ClearGate C ABI: echoes input to output and records
//! a mock LLM invocation via the callbacks vtable.

use std::ffi::{c_int, c_void, CStr, CString};
use std::os::raw::c_char;

/// Engine-provided callback vtable. Must match the C ABI layout in
/// `cleargate-core/src/flow/dylib.rs`.
#[repr(C)]
pub struct NodeCallbacks {
    pub opaque: *mut c_void,
    pub emit: unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char) -> c_int,
    pub record_llm_call: unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char) -> c_int,
    pub secret_get: unsafe extern "C" fn(*mut c_void, *const c_char) -> *const c_char,
    pub state_get: unsafe extern "C" fn(*mut c_void, *const c_char) -> *const c_char,
    pub state_set: unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char) -> c_int,
}

/// Return node metadata as a JSON string.
///
/// # Safety
/// The returned pointer must be freed by calling `node_free`.
#[no_mangle]
pub unsafe extern "C" fn node_meta() -> *const c_char {
    let meta = serde_json::json!({
        "node_type": "test_echo",
        "label": "Test Echo",
        "category": "test",
        "inputs": [{
            "name": "input",
            "port_type": "json",
            "required": true,
            "description": "Input data"
        }],
        "outputs": [{
            "name": "output",
            "port_type": "json",
            "required": true,
            "description": "Echoed data"
        }],
        "config_schema": {},
        "ui": {},
        "execution": {}
    });
    let json = serde_json::to_string(&meta).unwrap();
    CString::new(json).unwrap().into_raw()
}

/// Execute: echo input to output, call record_llm_call.
///
/// # Safety
/// `input_ptr` and `config_ptr` must be valid null-terminated UTF-8.
/// `callbacks` must be a valid engine-provided vtable pointer.
/// The returned pointer must be freed by calling `node_free`.
#[no_mangle]
pub unsafe extern "C" fn node_run(
    input_ptr: *const c_char,
    _config_ptr: *const c_char,
    callbacks: *const NodeCallbacks,
) -> *const c_char {
    let input_str = CStr::from_ptr(input_ptr).to_str().unwrap_or("{}");

    // Record a mock LLM invocation via the callback.
    if !callbacks.is_null() {
        let request = serde_json::json!({
            "provider": "test",
            "model": "echo-v1",
            "messages": [{"role": "user", "content": "echo test"}],
            "temperature": 0.0,
            "extra_params": {}
        });
        let response = serde_json::json!({
            "content": "echoed",
            "model_used": "echo-v1",
            "input_tokens": 5,
            "output_tokens": 3,
            "total_tokens": 8,
            "finish_reason": "stop",
            "latency_ms": 1
        });
        let req_json = CString::new(serde_json::to_string(&request).unwrap()).unwrap();
        let resp_json = CString::new(serde_json::to_string(&response).unwrap()).unwrap();
        let opaque = (*callbacks).opaque;
        ((*callbacks).record_llm_call)(opaque, req_json.as_ptr(), resp_json.as_ptr());
    }

    // Echo: return input as output.
    CString::new(input_str).unwrap().into_raw()
}

/// Free a string allocated by this library.
///
/// # Safety
/// `ptr` must have been returned by `node_meta` or `node_run`.
#[no_mangle]
pub unsafe extern "C" fn node_free(ptr: *const c_char) {
    if !ptr.is_null() {
        drop(CString::from_raw(ptr as *mut c_char));
    }
}
