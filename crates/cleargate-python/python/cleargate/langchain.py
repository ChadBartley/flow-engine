"""LangChain callback handler for Cleargate observability.

Usage::

    from cleargate.langchain import CleargateLangChainHandler

    with CleargateLangChainHandler("my-chain") as handler:
        llm = ChatOpenAI(model="gpt-4o", callbacks=[handler])
        result = llm.invoke("Hello")

    print(handler.run_id)
    print(handler.get_run_data())
"""

from __future__ import annotations

import json
import logging
import time
from typing import Any, Dict, List, Optional

from langchain_core.callbacks import BaseCallbackHandler
from langchain_core.outputs import LLMResult

from cleargate import AdapterSession

logger = logging.getLogger("cleargate.langchain")


def _safe_serialize(obj: Any) -> Any:
    """Convert LangChain objects to JSON-safe dicts.

    Uses json round-trip with ``default=str`` to handle arbitrary objects
    without risking infinite recursion on circular references.
    """
    try:
        return json.loads(json.dumps(obj, default=str))
    except (TypeError, ValueError):
        return str(obj)


class CleargateLangChainHandler(BaseCallbackHandler):
    """Drop-in LangChain callback handler that records to Cleargate."""

    def __init__(
        self,
        session_name: str = "langchain",
        *,
        session: Optional[AdapterSession] = None,
    ):
        super().__init__()
        if session is not None:
            self._session = session
            self._owns_session = False
        else:
            self._session = AdapterSession.start("langchain", session_name)
            self._owns_session = True
        self._llm_start_times: Dict[str, float] = {}

    def on_llm_start(
        self,
        serialized: Dict[str, Any],
        prompts: List[str],
        *,
        run_id: Any,
        **kwargs: Any,
    ) -> None:
        rid = str(run_id)
        logger.debug("on_llm_start run_id=%s", rid)
        self._llm_start_times[rid] = time.monotonic()
        payload = {
            "callback": "on_llm_start",
            "run_id": rid,
            "serialized": _safe_serialize(serialized),
            "prompts": prompts,
        }
        logger.debug("on_llm_start serialized, calling on_event")
        self._session.on_event(payload)
        logger.debug("on_llm_start on_event returned")

    def on_llm_end(
        self,
        response: LLMResult,
        *,
        run_id: Any,
        **kwargs: Any,
    ) -> None:
        rid = str(run_id)
        logger.debug("on_llm_end run_id=%s", rid)
        start = self._llm_start_times.pop(rid, None)
        duration_ms = int((time.monotonic() - start) * 1000) if start else 0
        logger.debug("on_llm_end serializing response type=%s", type(response).__name__)
        payload = {
            "callback": "on_llm_end",
            "run_id": rid,
            "response": _safe_serialize(response),
            "duration_ms": duration_ms,
        }
        logger.debug("on_llm_end serialized, calling on_event")
        self._session.on_event(payload)
        logger.debug("on_llm_end on_event returned")

    def on_tool_start(
        self,
        serialized: Dict[str, Any],
        input_str: str,
        *,
        run_id: Any,
        **kwargs: Any,
    ) -> None:
        logger.debug("on_tool_start run_id=%s tool=%s", run_id, serialized.get("name"))
        self._session.on_event(
            {
                "callback": "on_tool_start",
                "run_id": str(run_id),
                "serialized": _safe_serialize(serialized),
                "input_str": input_str,
            }
        )
        logger.debug("on_tool_start on_event returned")

    def on_tool_end(
        self,
        output: Any,
        *,
        run_id: Any,
        **kwargs: Any,
    ) -> None:
        logger.debug("on_tool_end run_id=%s", run_id)
        self._session.on_event(
            {
                "callback": "on_tool_end",
                "run_id": str(run_id),
                "output": _safe_serialize(output),
            }
        )
        logger.debug("on_tool_end on_event returned")

    def on_chain_start(
        self,
        serialized: Dict[str, Any],
        inputs: Dict[str, Any],
        *,
        run_id: Any,
        **kwargs: Any,
    ) -> None:
        logger.debug("on_chain_start run_id=%s", run_id)
        self._session.on_event(
            {
                "callback": "on_chain_start",
                "run_id": str(run_id),
                "serialized": _safe_serialize(serialized),
                "inputs": _safe_serialize(inputs),
            }
        )
        logger.debug("on_chain_start on_event returned")

    def on_chain_end(
        self,
        outputs: Dict[str, Any],
        *,
        run_id: Any,
        **kwargs: Any,
    ) -> None:
        logger.debug("on_chain_end run_id=%s", run_id)
        self._session.on_event(
            {
                "callback": "on_chain_end",
                "run_id": str(run_id),
                "outputs": _safe_serialize(outputs),
            }
        )
        logger.debug("on_chain_end on_event returned")

    def finish(self, status: str = "completed") -> None:
        """Finish the session. Called automatically by __exit__."""
        logger.debug("finish status=%s owns=%s", status, self._owns_session)
        if self._owns_session:
            self._session.finish(status)
        logger.debug("finish completed")

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

    def __enter__(self) -> "CleargateLangChainHandler":
        return self

    def __exit__(self, exc_type: Any, exc_val: Any, exc_tb: Any) -> bool:
        status = "failed" if exc_type is not None else "completed"
        self.finish(status)
        return False
