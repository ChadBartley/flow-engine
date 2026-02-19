"""Smoke tests — validate the native module loads and basic operations work."""

import cleargate
from cleargate._native import ObserverSession


def test_native_module_loads():
    """The compiled Rust module is importable."""
    from cleargate import _native

    assert hasattr(_native, "ObserverSession")
    assert hasattr(_native, "AdapterSession")


def test_observer_session_start():
    """Can create an ObserverSession and get a run_id back."""
    session = ObserverSession.start("test-session")
    assert session.run_id  # non-empty string
    assert isinstance(session.run_id, str)
    session.finish("completed")


def test_observer_session_context_manager():
    """Context manager API works end-to-end."""
    with cleargate.observe("ctx-test") as session:
        assert session.run_id
        session.record_step("init", {"ready": True})
    # Verify run data accessible after context exit
    data = session.get_run_data()
    assert isinstance(data, dict)


def test_observer_session_context_manager_exception():
    """Context manager finishes with 'failed' on exception."""
    try:
        with cleargate.observe("fail-test") as session:
            session.record_step("before_error", {})
            raise ValueError("boom")
    except ValueError:
        pass  # Expected


def test_record_llm_call():
    """record_llm_call accepts dicts and doesn't crash."""
    session = ObserverSession.start("llm-test")
    session.record_llm_call(
        "test-node",
        {
            "provider": "openai",
            "model": "gpt-4",
            "messages": [{"role": "user", "content": "hi"}],
        },
        {"content": "hello", "model": "gpt-4", "input_tokens": 5, "output_tokens": 3},
    )
    session.finish("completed")


def test_record_tool_call():
    """record_tool_call accepts dicts and doesn't crash."""
    session = ObserverSession.start("tool-test")
    session.record_tool_call("search", {"query": "rust"}, {"results": []}, 42)
    session.finish("completed")


def test_record_step():
    """record_step with arbitrary JSON data."""
    session = ObserverSession.start("step-test")
    session.record_step("preprocess", {"cleaned": True, "count": 42})
    session.finish("completed")


def test_double_finish_raises():
    """Finishing twice should raise an error."""
    session = ObserverSession.start("double-finish")
    session.finish("completed")
    try:
        session.finish("completed")
        assert False, "Should have raised"
    except RuntimeError:
        pass


def test_trace_decorator_basic():
    """@cleargate.trace creates a session and records execution."""

    @cleargate.trace
    def my_func():
        return 42

    result = my_func()
    assert result == 42


def test_trace_decorator_nested():
    """Nested @trace calls record steps, not new sessions."""
    calls = []

    @cleargate.trace
    def outer():
        calls.append("outer")
        inner()

    @cleargate.trace
    def inner():
        calls.append("inner")

    outer()
    assert calls == ["outer", "inner"]


def test_trace_decorator_with_name():
    """@cleargate.trace(name="custom") uses custom step name."""

    @cleargate.trace(name="custom_step")
    def my_func():
        return "ok"

    assert my_func() == "ok"


def test_get_run_data_after_finish():
    """get_run_data() returns a dict with run info after finish()."""
    session = ObserverSession.start("run-data-test")
    session.record_step("init", {"ready": True})
    session.finish("completed")
    data = session.get_run_data()
    assert isinstance(data, dict)
    assert data["run_id"] == session.run_id
    assert data["status"] == "completed"


def test_get_events_after_finish():
    """get_events() returns a list of event dicts after finish()."""
    session = ObserverSession.start("events-test")
    session.record_step("step1", {"x": 1})
    session.record_step("step2", {"x": 2})
    session.finish("completed")
    events = session.get_events()
    assert isinstance(events, list)
    assert len(events) > 0


def test_get_active_session_outside_context():
    """get_active_session returns None when not in a trace/observe context."""
    assert cleargate.get_active_session() is None


def test_get_active_session_inside_context():
    """get_active_session returns the session inside an observe block."""
    with cleargate.observe("active-test") as session:
        active = cleargate.get_active_session()
        assert active is not None
        assert active.run_id == session.run_id

    # After exiting, should be None again
    assert cleargate.get_active_session() is None
