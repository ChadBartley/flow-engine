"""LangGraph callback handler for Cleargate observability.

Usage::

    from cleargate.langgraph import CleargateLangGraphHandler

    with CleargateLangGraphHandler("my-graph") as handler:
        app = graph.compile()
        result = app.invoke({"messages": [...]}, config={"callbacks": [handler]})

    print(handler.get_run_data())
"""

from __future__ import annotations

import logging
import time
from typing import Any, Dict, List, Optional

from langchain_core.callbacks import BaseCallbackHandler
from langchain_core.outputs import LLMResult

from cleargate import AdapterSession
from cleargate.langchain import _messages_to_dicts, _safe_serialize


def _extract_model_from_repr(serialized: Dict[str, Any]) -> Optional[str]:
    """Extract model name from repr string (e.g. ``model='ministral-3:3b'``)."""
    repr_str = serialized.get("repr")
    if not repr_str or not isinstance(repr_str, str):
        return None
    parts = repr_str.split("model=")
    if len(parts) < 2:
        return None
    after = parts[1]
    if not after or after[0] not in ("'", '"'):
        return None
    quote = after[0]
    model = after[1:].split(quote)[0]
    return model if model else None


logger = logging.getLogger("cleargate.langgraph")


class CleargateLangGraphHandler(BaseCallbackHandler):
    """Drop-in LangGraph callback handler that records to Cleargate.

    Extends LangChain callbacks with node/edge tracking for graph execution.
    """

    def __init__(
        self,
        session_name: str = "langgraph",
        *,
        session: Optional[AdapterSession] = None,
        store_path: Optional[str] = None,
    ):
        super().__init__()
        if session is not None:
            self._session = session
            self._owns_session = False
        else:
            self._session = AdapterSession.start(
                "langgraph", session_name, store_path=store_path
            )
            self._owns_session = True
        self._llm_start_times: Dict[str, float] = {}
        self._current_node: Optional[str] = None

    def on_chain_start(
        self,
        serialized: Dict[str, Any],
        inputs: Dict[str, Any],
        *,
        run_id: Any,
        **kwargs: Any,
    ) -> None:
        serialized = serialized or {}
        name = (
            serialized.get("id", [None])[-1]
            if isinstance(serialized.get("id"), list)
            else None
        ) or serialized.get("name", "chain")

        tags = kwargs.get("tags", [])
        node_name = None
        for tag in tags:
            if isinstance(tag, str) and tag.startswith("graph:step:"):
                node_name = tag.split(":", 2)[-1]
                break

        if node_name:
            if self._current_node and self._current_node != node_name:
                self._session.on_event(
                    {
                        "callback": "edge",
                        "from": self._current_node,
                        "to": node_name,
                    }
                )
            self._current_node = node_name
            self._session.on_event(
                {
                    "callback": "node_start",
                    "node": node_name,
                    "state": _safe_serialize(inputs),
                }
            )
        else:
            self._session.on_event(
                {
                    "callback": "node_start",
                    "node": name,
                    "state": _safe_serialize(inputs),
                }
            )

    def on_chain_end(
        self,
        outputs: Dict[str, Any],
        *,
        run_id: Any,
        **kwargs: Any,
    ) -> None:
        tags = kwargs.get("tags", [])
        node_name = None
        for tag in tags:
            if isinstance(tag, str) and tag.startswith("graph:step:"):
                node_name = tag.split(":", 2)[-1]
                break

        self._session.on_event(
            {
                "callback": "node_end",
                "node": node_name or self._current_node or "chain",
                "state": _safe_serialize(outputs),
            }
        )

    def on_llm_start(
        self,
        serialized: Dict[str, Any],
        prompts: List[str],
        *,
        run_id: Any,
        **kwargs: Any,
    ) -> None:
        serialized = serialized or {}
        rid = str(run_id)
        self._llm_start_times[rid] = time.monotonic()
        model = (
            serialized.get("kwargs", {}).get("model_name")
            or serialized.get("kwargs", {}).get("model")
            or "unknown"
        )
        self._session.on_event(
            {
                "callback": "on_llm_start",
                "node": self._current_node or "llm",
                "request": {"model": model, "messages": prompts},
            }
        )

    def on_chat_model_start(
        self,
        serialized: Dict[str, Any],
        messages: List[List[Any]],
        *,
        run_id: Any,
        **kwargs: Any,
    ) -> None:
        """Called for chat models instead of ``on_llm_start``."""
        serialized = serialized or {}
        rid = str(run_id)
        self._llm_start_times[rid] = time.monotonic()
        model = (
            serialized.get("kwargs", {}).get("model_name")
            or serialized.get("kwargs", {}).get("model")
            or _extract_model_from_repr(serialized)
            or "unknown"
        )

        self._session.on_event(
            {
                "callback": "on_llm_start",
                "node": self._current_node or "llm",
                "request": {"model": model, "messages": _messages_to_dicts(messages)},
            }
        )

    def on_llm_end(
        self,
        response: LLMResult,
        *,
        run_id: Any,
        **kwargs: Any,
    ) -> None:
        rid = str(run_id)
        start = self._llm_start_times.pop(rid, None)
        duration_ms = int((time.monotonic() - start) * 1000) if start else 0
        self._session.on_event(
            {
                "callback": "on_llm_end",
                "node": self._current_node or "llm",
                "response": _safe_serialize(response),
                "duration_ms": duration_ms,
            }
        )

    def on_tool_start(
        self,
        serialized: Dict[str, Any],
        input_str: str,
        *,
        run_id: Any,
        **kwargs: Any,
    ) -> None:
        serialized = serialized or {}
        tool_name = serialized.get("name", "unknown")
        self._session.on_event(
            {
                "callback": "node_start",
                "node": tool_name,
                "state": {"input": input_str},
            }
        )

    def on_tool_end(
        self,
        output: Any,
        *,
        run_id: Any,
        **kwargs: Any,
    ) -> None:
        self._session.on_event(
            {
                "callback": "node_end",
                "node": "tool",
                "state": {"output": _safe_serialize(output)},
            }
        )

    def emit_state_update(self, node: str, updates: Dict[str, Any]) -> None:
        """Manually emit a state update event."""
        self._session.on_event(
            {
                "callback": "state_update",
                "node": node,
                "updates": _safe_serialize(updates),
            }
        )

    def finish(self, status: str = "completed") -> None:
        if self._owns_session:
            self._session.finish(status)

    @property
    def run_id(self) -> str:
        return self._session.run_id

    def get_run_data(self) -> Any:
        """Return run summary: metadata, status, timing, aggregate LLM stats.

        Does not include the detailed event log — use ``get_events()`` for
        that. Together they provide the complete picture for DiffEngine and
        ReplayEngine.
        """
        return self._session.get_run_data()

    def get_events(self) -> Any:
        """Return the full event log (LLM calls, tool invocations, steps, etc.).

        This is the detailed record consumed by DiffEngine and ReplayEngine.
        """
        return self._session.get_events()

    def __enter__(self) -> "CleargateLangGraphHandler":
        return self

    def __exit__(self, exc_type: Any, exc_val: Any, exc_tb: Any) -> bool:
        status = "failed" if exc_type is not None else "completed"
        self.finish(status)
        return False
