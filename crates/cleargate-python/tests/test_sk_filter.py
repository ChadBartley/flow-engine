"""Tests for CleargateSKFilter."""

import asyncio
from unittest.mock import MagicMock

import pytest


@pytest.fixture
def mock_adapter_session():
    session = MagicMock()
    session.run_id = "test-run-sk"
    session.get_run_data.return_value = {"run_id": "test-run-sk"}
    return session


def run_async(coro):
    return asyncio.get_event_loop().run_until_complete(coro)


class TestCleargateSKFilter:
    def test_init_with_session(self, mock_adapter_session):
        from cleargate.semantic_kernel import CleargateSKFilter

        f = CleargateSKFilter(session=mock_adapter_session)
        assert f.run_id == "test-run-sk"
        assert not f._owns_session

    def test_function_invoking(self, mock_adapter_session):
        from cleargate.semantic_kernel import CleargateSKFilter

        f = CleargateSKFilter(session=mock_adapter_session)

        ctx = MagicMock()
        ctx.function.plugin_name = "SearchPlugin"
        ctx.function.name = "search"
        ctx.arguments = {"query": "rust lang"}
        ctx.arguments.items.return_value = [("query", "rust lang")]

        run_async(f.on_function_invoking(ctx))

        event = mock_adapter_session.on_event.call_args[0][0]
        assert event["callback"] == "function_invoking"
        assert event["plugin_name"] == "SearchPlugin"
        assert event["function_name"] == "search"
        assert event["arguments"]["query"] == "rust lang"

    def test_function_invoked_with_duration(self, mock_adapter_session):
        from cleargate.semantic_kernel import CleargateSKFilter

        f = CleargateSKFilter(session=mock_adapter_session)

        ctx_start = MagicMock()
        ctx_start.function.plugin_name = "Math"
        ctx_start.function.name = "add"
        ctx_start.arguments = {}
        ctx_start.arguments.items.return_value = []

        run_async(f.on_function_invoking(ctx_start))
        mock_adapter_session.on_event.reset_mock()

        ctx_end = MagicMock()
        ctx_end.function.plugin_name = "Math"
        ctx_end.function.name = "add"
        ctx_end.result = "42"

        run_async(f.on_function_invoked(ctx_end))

        event = mock_adapter_session.on_event.call_args[0][0]
        assert event["callback"] == "function_invoked"
        assert event["duration_ms"] >= 0
        assert event["result"] == "42"

    def test_prompt_rendering(self, mock_adapter_session):
        from cleargate.semantic_kernel import CleargateSKFilter

        f = CleargateSKFilter(session=mock_adapter_session)

        ctx = MagicMock()
        ctx.template = "Tell me about {{$topic}}"
        ctx.rendered_prompt = None

        run_async(f.on_prompt_rendering(ctx))

        event = mock_adapter_session.on_event.call_args[0][0]
        assert event["callback"] == "prompt_rendering"
        assert "topic" in event["template"]

    def test_prompt_rendered(self, mock_adapter_session):
        from cleargate.semantic_kernel import CleargateSKFilter

        f = CleargateSKFilter(session=mock_adapter_session)

        ctx = MagicMock()
        ctx.rendered_prompt = "Tell me about Rust"

        run_async(f.on_prompt_rendered(ctx))

        event = mock_adapter_session.on_event.call_args[0][0]
        assert event["callback"] == "prompt_rendered"
        assert event["rendered_prompt"] == "Tell me about Rust"

    def test_context_manager(self, mock_adapter_session):
        from cleargate.semantic_kernel import CleargateSKFilter

        f = CleargateSKFilter(session=mock_adapter_session)
        f._owns_session = True

        with f as sk:
            assert sk is f

        mock_adapter_session.finish.assert_called_once_with("completed")

    def test_context_manager_on_error(self, mock_adapter_session):
        from cleargate.semantic_kernel import CleargateSKFilter

        f = CleargateSKFilter(session=mock_adapter_session)
        f._owns_session = True

        with pytest.raises(RuntimeError):
            with f:
                raise RuntimeError("fail")

        mock_adapter_session.finish.assert_called_once_with("failed")

    def test_get_run_data(self, mock_adapter_session):
        from cleargate.semantic_kernel import CleargateSKFilter

        f = CleargateSKFilter(session=mock_adapter_session)
        assert f.get_run_data()["run_id"] == "test-run-sk"
