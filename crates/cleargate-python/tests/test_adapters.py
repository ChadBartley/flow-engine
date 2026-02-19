"""Test framework adapter sessions via Python."""

from cleargate._native import AdapterSession


def test_adapter_session_langchain():
    """LangChain adapter session can process events."""
    session = AdapterSession.start("langchain")
    assert session.run_id

    # Simulate LangChain on_llm_start callback
    session.on_event(
        {
            "callback": "on_llm_start",
            "run_id": "lc-run-1",
            "serialized": {"kwargs": {"model_name": "gpt-4o"}},
            "prompts": ["What is Rust?"],
        }
    )

    # Simulate on_llm_end
    session.on_event(
        {
            "callback": "on_llm_end",
            "run_id": "lc-run-1",
            "response": {
                "generations": [[{"text": "Rust is a systems programming language."}]],
                "llm_output": {
                    "model_name": "gpt-4o",
                    "token_usage": {
                        "prompt_tokens": 10,
                        "completion_tokens": 20,
                        "total_tokens": 30,
                    },
                },
            },
            "duration_ms": 500,
        }
    )

    session.finish("completed")

    data = session.get_run_data()
    assert isinstance(data, dict)
    assert data["run_id"] == session.run_id

    events = session.get_events()
    assert isinstance(events, list)
    assert len(events) > 0


def test_adapter_session_langgraph():
    """LangGraph adapter session processes node transitions."""
    session = AdapterSession.start("langgraph")

    session.on_event({"callback": "node_start", "node": "agent", "state": {}})
    session.on_event({"callback": "edge", "from": "agent", "to": "tools"})
    session.on_event({"callback": "node_end", "node": "agent", "state": {"done": True}})

    session.finish("completed")

    data = session.get_run_data()
    assert isinstance(data, dict)
    assert data["run_id"] == session.run_id


def test_adapter_session_semantic_kernel():
    """Semantic Kernel adapter session processes function events."""
    session = AdapterSession.start("semantic_kernel")

    session.on_event(
        {
            "callback": "function_invoking",
            "plugin_name": "SearchPlugin",
            "function_name": "search",
            "arguments": {"query": "rust"},
        }
    )
    session.on_event(
        {
            "callback": "function_invoked",
            "plugin_name": "SearchPlugin",
            "function_name": "search",
            "result": {"text": "found it"},
            "duration_ms": 100,
        }
    )

    session.finish("completed")

    data = session.get_run_data()
    assert isinstance(data, dict)
    assert data["run_id"] == session.run_id


def test_adapter_session_context_manager():
    """AdapterSession works as a context manager."""
    with AdapterSession.start("langchain") as session:
        session.on_event(
            {
                "callback": "on_chain_start",
                "serialized": {"id": ["langchain", "chains", "LLMChain"]},
                "inputs": {"question": "hi"},
            }
        )

    data = session.get_run_data()
    assert isinstance(data, dict)


def test_adapter_session_unknown_framework():
    """Unknown framework name raises an error."""
    try:
        AdapterSession.start("unknown_framework")
        assert False, "Should have raised"
    except RuntimeError as e:
        assert "unknown framework" in str(e).lower()
