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


def _serialize_llm_result(result: Any) -> Dict[str, Any]:
    """Extract structured data from a LangChain ``LLMResult``.

    LLMResult is not directly JSON-serializable, so we pull the fields
    the Rust adapter expects: ``generations``, ``llm_output``.
    """
    out: Dict[str, Any] = {}

    # Extract generations — each is a list of ChatGeneration/ChatGenerationChunk
    gens = getattr(result, "generations", None)
    if gens:
        serialized_gens = []
        for gen_list in gens:
            serialized_gen = []
            for gen in gen_list:
                entry: Dict[str, Any] = {"text": getattr(gen, "text", "")}
                gen_info = getattr(gen, "generation_info", None)
                if gen_info:
                    entry["generation_info"] = _safe_serialize(gen_info)
                msg = getattr(gen, "message", None)
                if msg:
                    metadata = getattr(msg, "response_metadata", {})
                    if metadata:
                        entry["generation_info"] = _safe_serialize(metadata)
                    usage = getattr(msg, "usage_metadata", None)
                    if usage:
                        entry["usage_metadata"] = _safe_serialize(usage)
                serialized_gen.append(entry)
            serialized_gens.append(serialized_gen)
        out["generations"] = serialized_gens

    # Extract llm_output (may be None for some providers like Ollama)
    llm_output = getattr(result, "llm_output", None)
    if llm_output:
        out["llm_output"] = _safe_serialize(llm_output)

    return out


# Map LangChain message type names to standard OpenAI-style roles.
# The redactor and DiffEngine expect {"role": "user"|"assistant"|"system"|"tool", "content": ...}.
_ROLE_MAP = {
    "human": "user",
    "HumanMessage": "user",
    "HumanMessageChunk": "user",
    "ai": "assistant",
    "AIMessage": "assistant",
    "AIMessageChunk": "assistant",
    "system": "system",
    "SystemMessage": "system",
    "SystemMessageChunk": "system",
    "tool": "tool",
    "ToolMessage": "tool",
    "ToolMessageChunk": "tool",
    "function": "function",
    "chat": "assistant",
}


def _messages_to_dicts(messages: List[List[Any]]) -> List[Dict[str, Any]]:
    """Convert LangChain message objects to ``{role, content, ...}`` dicts.

    Normalizes role names to the standard set expected by the write pipeline
    redactor (``user``, ``assistant``, ``system``, ``tool``).
    """
    result = []
    for msg_list in messages:
        for msg in msg_list:
            lc_type = getattr(msg, "type", "unknown")
            role = _ROLE_MAP.get(lc_type, lc_type)
            content = getattr(msg, "content", "")
            entry: Dict[str, Any] = {"role": role, "content": content}
            tool_calls = getattr(msg, "tool_calls", None)
            if tool_calls:
                entry["tool_calls"] = [
                    {
                        "id": tc.get("id", "")
                        if isinstance(tc, dict)
                        else getattr(tc, "id", ""),
                        "tool_name": tc.get("name", "")
                        if isinstance(tc, dict)
                        else getattr(tc, "name", ""),
                        "arguments": _safe_serialize(
                            tc.get("args", {})
                            if isinstance(tc, dict)
                            else getattr(tc, "args", {})
                        ),
                    }
                    for tc in tool_calls
                ]
            tool_call_id = getattr(msg, "tool_call_id", None)
            if tool_call_id:
                entry["tool_call_id"] = tool_call_id
            result.append(entry)
    return result


class CleargateLangChainHandler(BaseCallbackHandler):
    """Drop-in LangChain callback handler that records to Cleargate."""

    def __init__(
        self,
        session_name: str = "langchain",
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
                "langchain", session_name, store_path=store_path
            )
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

    def on_chat_model_start(
        self,
        serialized: Dict[str, Any],
        messages: List[List[Any]],
        *,
        run_id: Any,
        **kwargs: Any,
    ) -> None:
        """Called for chat models instead of ``on_llm_start``.

        Receives structured message objects rather than flattened prompt
        strings, producing a ``{role, content}`` array in the event.
        """
        rid = str(run_id)
        logger.debug("on_chat_model_start run_id=%s", rid)
        self._llm_start_times[rid] = time.monotonic()

        structured_messages = _messages_to_dicts(messages)

        payload = {
            "callback": "on_llm_start",
            "run_id": rid,
            "serialized": _safe_serialize(serialized),
            "prompts": structured_messages,
        }
        self._session.on_event(payload)
        logger.debug("on_chat_model_start on_event returned")

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
            "response": _serialize_llm_result(response),
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
