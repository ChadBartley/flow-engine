"""Tests for CleargateLangChainHandler and CleargateLangGraphHandler."""

import uuid
from unittest.mock import MagicMock

import pytest


@pytest.fixture
def mock_adapter_session():
    """Create a mock AdapterSession."""
    session = MagicMock()
    session.run_id = "test-run-123"
    session.get_run_data.return_value = {"run_id": "test-run-123", "events": []}
    return session


class TestCleargateLangChainHandler:
    def test_init_with_session(self, mock_adapter_session):
        from cleargate.langchain import CleargateLangChainHandler

        handler = CleargateLangChainHandler(session=mock_adapter_session)
        assert handler.run_id == "test-run-123"
        assert not handler._owns_session

    def test_on_llm_start(self, mock_adapter_session):
        from cleargate.langchain import CleargateLangChainHandler

        handler = CleargateLangChainHandler(session=mock_adapter_session)
        run_id = uuid.uuid4()
        handler.on_llm_start(
            serialized={
                "id": ["langchain", "llms", "openai", "ChatOpenAI"],
                "kwargs": {"model_name": "gpt-4o"},
            },
            prompts=["Hello"],
            run_id=run_id,
        )

        mock_adapter_session.on_event.assert_called_once()
        event = mock_adapter_session.on_event.call_args[0][0]
        assert event["callback"] == "on_llm_start"
        assert event["run_id"] == str(run_id)
        assert event["prompts"] == ["Hello"]
        assert event["serialized"]["kwargs"]["model_name"] == "gpt-4o"

    def test_on_llm_end_with_duration(self, mock_adapter_session):
        from cleargate.langchain import CleargateLangChainHandler

        handler = CleargateLangChainHandler(session=mock_adapter_session)
        run_id = uuid.uuid4()

        # Start first to track duration
        handler.on_llm_start(
            serialized={"kwargs": {}},
            prompts=["Hi"],
            run_id=run_id,
        )
        mock_adapter_session.on_event.reset_mock()

        # Mock LLMResult
        response = MagicMock()
        response.dict.return_value = {
            "generations": [[{"text": "Hello!"}]],
            "llm_output": {"model_name": "gpt-4o", "token_usage": {}},
        }

        handler.on_llm_end(response=response, run_id=run_id)

        event = mock_adapter_session.on_event.call_args[0][0]
        assert event["callback"] == "on_llm_end"
        assert event["duration_ms"] >= 0

    def test_on_tool_start(self, mock_adapter_session):
        from cleargate.langchain import CleargateLangChainHandler

        handler = CleargateLangChainHandler(session=mock_adapter_session)
        run_id = uuid.uuid4()
        handler.on_tool_start(
            serialized={"name": "calculator"},
            input_str="2+2",
            run_id=run_id,
        )

        event = mock_adapter_session.on_event.call_args[0][0]
        assert event["callback"] == "on_tool_start"
        assert event["input_str"] == "2+2"

    def test_on_tool_end(self, mock_adapter_session):
        from cleargate.langchain import CleargateLangChainHandler

        handler = CleargateLangChainHandler(session=mock_adapter_session)
        handler.on_tool_end(output="4", run_id=uuid.uuid4())

        event = mock_adapter_session.on_event.call_args[0][0]
        assert event["callback"] == "on_tool_end"
        assert event["output"] == "4"

    def test_on_chain_start_end(self, mock_adapter_session):
        from cleargate.langchain import CleargateLangChainHandler

        handler = CleargateLangChainHandler(session=mock_adapter_session)
        run_id = uuid.uuid4()

        handler.on_chain_start(
            serialized={"id": ["langchain", "chains", "RetrievalQA"]},
            inputs={"query": "what is rust?"},
            run_id=run_id,
        )
        event = mock_adapter_session.on_event.call_args[0][0]
        assert event["callback"] == "on_chain_start"
        assert event["inputs"]["query"] == "what is rust?"

        handler.on_chain_end(
            outputs={"result": "A systems language"},
            run_id=run_id,
        )
        event = mock_adapter_session.on_event.call_args[0][0]
        assert event["callback"] == "on_chain_end"

    def test_context_manager(self, mock_adapter_session):
        from cleargate.langchain import CleargateLangChainHandler

        handler = CleargateLangChainHandler(session=mock_adapter_session)
        handler._owns_session = True

        with handler as h:
            assert h is handler

        mock_adapter_session.finish.assert_called_once_with("completed")

    def test_context_manager_on_error(self, mock_adapter_session):
        from cleargate.langchain import CleargateLangChainHandler

        handler = CleargateLangChainHandler(session=mock_adapter_session)
        handler._owns_session = True

        with pytest.raises(ValueError):
            with handler:
                raise ValueError("boom")

        mock_adapter_session.finish.assert_called_once_with("failed")

    def test_get_run_data(self, mock_adapter_session):
        from cleargate.langchain import CleargateLangChainHandler

        handler = CleargateLangChainHandler(session=mock_adapter_session)
        data = handler.get_run_data()
        assert data["run_id"] == "test-run-123"


class TestSafeSerialize:
    def test_primitives(self):
        from cleargate.langchain import _safe_serialize

        assert _safe_serialize("hello") == "hello"
        assert _safe_serialize(42) == 42
        assert _safe_serialize(True) is True
        assert _safe_serialize(None) is None

    def test_dict(self):
        from cleargate.langchain import _safe_serialize

        assert _safe_serialize({"a": 1}) == {"a": 1}

    def test_list(self):
        from cleargate.langchain import _safe_serialize

        assert _safe_serialize([1, "two"]) == [1, "two"]

    def test_non_serializable_falls_back_to_str(self):
        from cleargate.langchain import _safe_serialize

        class Opaque:
            def __repr__(self):
                return "Opaque()"

        result = _safe_serialize({"key": Opaque()})
        assert result == {"key": "Opaque()"}

    def test_nested_objects(self):
        from cleargate.langchain import _safe_serialize

        result = _safe_serialize({"a": {"b": [1, 2, 3]}})
        assert result == {"a": {"b": [1, 2, 3]}}


class TestCleargateLangGraphHandler:
    def test_init_with_session(self, mock_adapter_session):
        from cleargate.langgraph import CleargateLangGraphHandler

        handler = CleargateLangGraphHandler(session=mock_adapter_session)
        assert handler.run_id == "test-run-123"

    def test_node_tracking_via_tags(self, mock_adapter_session):
        from cleargate.langgraph import CleargateLangGraphHandler

        handler = CleargateLangGraphHandler(session=mock_adapter_session)
        run_id = uuid.uuid4()

        handler.on_chain_start(
            serialized={"id": ["langgraph", "graph"]},
            inputs={"messages": []},
            run_id=run_id,
            tags=["graph:step:agent"],
        )

        event = mock_adapter_session.on_event.call_args[0][0]
        assert event["callback"] == "node_start"
        assert event["node"] == "agent"
        assert handler._current_node == "agent"

    def test_edge_transition(self, mock_adapter_session):
        from cleargate.langgraph import CleargateLangGraphHandler

        handler = CleargateLangGraphHandler(session=mock_adapter_session)

        # First node
        handler.on_chain_start(
            serialized={},
            inputs={},
            run_id=uuid.uuid4(),
            tags=["graph:step:agent"],
        )
        mock_adapter_session.on_event.reset_mock()

        # Second node — should emit edge + node_start
        handler.on_chain_start(
            serialized={},
            inputs={},
            run_id=uuid.uuid4(),
            tags=["graph:step:tools"],
        )

        calls = mock_adapter_session.on_event.call_args_list
        assert len(calls) == 2
        assert calls[0][0][0]["callback"] == "edge"
        assert calls[0][0][0]["from"] == "agent"
        assert calls[0][0][0]["to"] == "tools"
        assert calls[1][0][0]["callback"] == "node_start"

    def test_llm_within_node(self, mock_adapter_session):
        from cleargate.langgraph import CleargateLangGraphHandler

        handler = CleargateLangGraphHandler(session=mock_adapter_session)
        handler._current_node = "agent"

        handler.on_llm_start(
            serialized={"kwargs": {"model": "gpt-4o"}},
            prompts=["Hello"],
            run_id=uuid.uuid4(),
        )

        event = mock_adapter_session.on_event.call_args[0][0]
        assert event["callback"] == "on_llm_start"
        assert event["node"] == "agent"
        assert event["request"]["model"] == "gpt-4o"

    def test_state_update(self, mock_adapter_session):
        from cleargate.langgraph import CleargateLangGraphHandler

        handler = CleargateLangGraphHandler(session=mock_adapter_session)
        handler.emit_state_update("agent", {"messages": ["new msg"]})

        event = mock_adapter_session.on_event.call_args[0][0]
        assert event["callback"] == "state_update"
        assert event["node"] == "agent"

    def test_context_manager(self, mock_adapter_session):
        from cleargate.langgraph import CleargateLangGraphHandler

        handler = CleargateLangGraphHandler(session=mock_adapter_session)
        handler._owns_session = True

        with handler as h:
            assert h is handler

        mock_adapter_session.finish.assert_called_once_with("completed")
