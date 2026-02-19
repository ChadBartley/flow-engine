"""Semantic Kernel filter for Cleargate observability.

Usage::

    from cleargate.semantic_kernel import CleargateSKFilter

    with CleargateSKFilter("my-sk-app") as sk_filter:
        kernel.add_filter("function_invocation", sk_filter.function_invocation_filter)
        kernel.add_filter("auto_function_invocation", sk_filter.auto_function_invocation_filter)

        # Use invoke_chat to capture LLM request/response events
        result = await sk_filter.invoke_chat(
            chat_service, chat_history=chat, settings=settings, kernel=kernel,
        )

    print(sk_filter.get_run_data())
"""

from __future__ import annotations

import json
import time
from typing import Any, Callable, Coroutine, Dict, List, Optional

from cleargate import AdapterSession


def _safe_serialize(obj: Any) -> Any:
    """Convert SK objects to JSON-safe values.

    Uses json round-trip with ``default=str`` to handle arbitrary objects
    without risking infinite recursion on circular references.
    """
    try:
        return json.loads(json.dumps(obj, default=str))
    except (TypeError, ValueError):
        return str(obj)


def _chat_history_to_dicts(chat_history: Any) -> List[Dict[str, Any]]:
    """Convert SK ``ChatHistory`` to a list of ``{role, content}`` dicts.

    Handles three item types that SK embeds in message ``items``:

    - ``FunctionCallContent`` (on assistant messages) → ``tool_calls``
    - ``FunctionResultContent`` (on tool messages) → ``content`` + ``tool_call_id``
    - ``TextContent`` → already captured via ``msg.content``
    """
    messages_list = getattr(chat_history, "messages", None)
    if not messages_list:
        return []
    result = []
    for msg in messages_list:
        role = getattr(msg, "role", None)
        role_str = role.value if hasattr(role, "value") else str(role)
        content = getattr(msg, "content", "") or ""
        entry: Dict[str, Any] = {"role": role_str, "content": content}

        items = getattr(msg, "items", None)
        if items:
            tool_calls = []
            for item in items:
                item_type = type(item).__name__

                if item_type == "FunctionResultContent":
                    # Tool result message — extract the actual result as content
                    res = getattr(item, "result", None)
                    if res is not None:
                        entry["content"] = str(res)
                    tc_id = getattr(item, "id", None)
                    if tc_id:
                        entry["tool_call_id"] = tc_id
                elif item_type == "FunctionCallContent":
                    # Assistant message requesting tool calls
                    tc_id = getattr(item, "id", None)
                    tc_name = getattr(item, "name", None) or getattr(
                        item, "function_name", None
                    )
                    tc_args = getattr(item, "arguments", None)
                    if tc_name:
                        tool_calls.append(
                            {
                                "id": tc_id or "",
                                "tool_name": tc_name,
                                "arguments": _safe_serialize(tc_args)
                                if tc_args
                                else {},
                            }
                        )

            if tool_calls:
                entry["tool_calls"] = tool_calls

        result.append(entry)
    return result


class CleargateSKFilter:
    """Semantic Kernel filter that records events to Cleargate.

    SK filters use a middleware pattern: each filter is an async callable
    with signature ``(context, next)`` that must call ``await next(context)``
    to continue the pipeline.

    Register filters with ``kernel.add_filter``::

        kernel.add_filter("function_invocation", sk_filter.function_invocation_filter)
        kernel.add_filter("prompt_rendering", sk_filter.prompt_render_filter)

    SK does not expose an LLM-level filter, so use :meth:`invoke_chat` to
    wrap ``get_chat_message_content`` and capture LLM request/response events.
    """

    def __init__(
        self,
        session_name: str = "semantic_kernel",
        *,
        session: Optional[AdapterSession] = None,
        store_path: Optional[str] = None,
    ):
        if session is not None:
            self._session = session
            self._owns_session = False
        else:
            self._session = AdapterSession.start(
                "semantic_kernel", session_name, store_path=store_path
            )
            self._owns_session = True
        self._function_start_times: Dict[str, float] = {}
        self._llm_round_start: Optional[float] = None
        self._model: str = "unknown"
        self._service_id: str = "chat"

    async def auto_function_invocation_filter(
        self,
        context: Any,
        next: Callable[..., Coroutine[Any, Any, None]],
    ) -> None:
        """SK ``auto_function_invocation`` filter — captures intermediate LLM calls.

        Fires each time SK auto-invokes a tool during ``FunctionChoiceBehavior.Auto()``.
        At this point the chat history contains the assistant message with tool calls,
        so we emit ``on_llm_end`` for the LLM round that produced them.
        After the tool executes, we emit ``on_llm_start`` for the next round.
        """
        # The chat_history at this point contains the assistant message
        # that requested tool calls — emit on_llm_end for that round.
        chat_history = getattr(context, "chat_history", None)
        now = time.monotonic()

        if chat_history and self._llm_round_start is not None:
            # Find the last assistant message (the one with tool calls)
            messages = getattr(chat_history, "messages", [])
            assistant_content = ""
            tool_calls = []
            for msg in reversed(messages):
                role = getattr(msg, "role", None)
                role_str = role.value if hasattr(role, "value") else str(role)
                if role_str == "assistant":
                    assistant_content = getattr(msg, "content", "") or ""
                    items = getattr(msg, "items", None)
                    if items:
                        for item in items:
                            tc_name = getattr(item, "name", None) or getattr(
                                item, "function_name", None
                            )
                            if tc_name:
                                tool_calls.append(
                                    {
                                        "id": getattr(item, "id", "") or "",
                                        "tool_name": tc_name,
                                        "arguments": _safe_serialize(
                                            getattr(item, "arguments", {})
                                        ),
                                    }
                                )
                    break

            duration_ms = int((now - self._llm_round_start) * 1000)
            response: Dict[str, Any] = {
                "content": assistant_content,
                "model": self._model,
            }
            if tool_calls:
                response["tool_calls"] = tool_calls
            self._session.on_event(
                {
                    "callback": "on_llm_end",
                    "service_id": self._service_id,
                    "response": response,
                    "duration_ms": duration_ms,
                }
            )

        # Execute the tool
        await next(context)

        # After tool execution, emit on_llm_start for the next LLM round
        # (SK will call the LLM again with updated chat history including tool results)
        if chat_history:
            self._llm_round_start = time.monotonic()
            self._session.on_event(
                {
                    "callback": "on_llm_start",
                    "service_id": self._service_id,
                    "request": {
                        "model": self._model,
                        "provider": "semantic_kernel",
                        "messages": _chat_history_to_dicts(chat_history),
                    },
                }
            )

    async def invoke_chat(
        self,
        chat_service: Any,
        *,
        chat_history: Any,
        settings: Any = None,
        kernel: Any = None,
    ) -> Any:
        """Invoke ``get_chat_message_content`` with LLM event capture.

        Wraps the chat completion call to emit ``on_llm_start`` and
        ``on_llm_end`` events, since SK does not provide an LLM-level filter.
        """
        self._model = getattr(settings, "ai_model_id", None) or getattr(
            chat_service, "ai_model_id", "unknown"
        )
        self._service_id = getattr(chat_service, "service_id", "chat") or "chat"
        messages = _chat_history_to_dicts(chat_history)

        # Emit on_llm_start for the first LLM round
        self._session.on_event(
            {
                "callback": "on_llm_start",
                "service_id": self._service_id,
                "request": {
                    "model": self._model,
                    "provider": "semantic_kernel",
                    "messages": messages,
                },
            }
        )

        self._llm_round_start = time.monotonic()
        kwargs: Dict[str, Any] = {"chat_history": chat_history}
        if settings is not None:
            kwargs["settings"] = settings
        if kernel is not None:
            kwargs["kernel"] = kernel

        result = await chat_service.get_chat_message_content(**kwargs)

        # Emit on_llm_end for the final LLM round
        duration_ms = (
            int((time.monotonic() - self._llm_round_start) * 1000)
            if self._llm_round_start
            else 0
        )
        content = getattr(result, "content", "") or str(result)

        usage = {}
        metadata = getattr(result, "metadata", {})
        if isinstance(metadata, dict):
            if "usage" in metadata:
                usage = _safe_serialize(metadata["usage"])

        self._session.on_event(
            {
                "callback": "on_llm_end",
                "service_id": self._service_id,
                "response": {
                    "content": content,
                    "model": self._model,
                    "usage": usage,
                },
                "duration_ms": duration_ms,
            }
        )

        self._llm_round_start = None
        return result

    async def function_invocation_filter(
        self,
        context: Any,
        next: Callable[..., Coroutine[Any, Any, None]],
    ) -> None:
        """SK ``function_invocation`` filter — records tool invocation events."""
        plugin = getattr(context.function, "plugin_name", "unknown")
        function = getattr(context.function, "name", "unknown")
        key = f"{plugin}.{function}"

        # --- before invocation ---
        self._function_start_times[key] = time.monotonic()

        args = {}
        if hasattr(context, "arguments") and context.arguments:
            for k, v in context.arguments.items():
                args[k] = _safe_serialize(v)

        self._session.on_event(
            {
                "callback": "function_invoking",
                "plugin_name": plugin,
                "function_name": function,
                "arguments": args,
            }
        )

        # --- invoke the actual function (and any downstream filters) ---
        await next(context)

        # --- after invocation ---
        start = self._function_start_times.pop(key, None)
        duration_ms = int((time.monotonic() - start) * 1000) if start else 0

        result = _safe_serialize(getattr(context, "result", None))
        self._session.on_event(
            {
                "callback": "function_invoked",
                "plugin_name": plugin,
                "function_name": function,
                "result": result,
                "duration_ms": duration_ms,
            }
        )

    async def prompt_render_filter(
        self,
        context: Any,
        next: Callable[..., Coroutine[Any, Any, None]],
    ) -> None:
        """SK ``prompt_rendering`` filter — records before/after events."""
        template = _safe_serialize(
            getattr(context, "rendered_prompt", None)
            or getattr(context, "template", None)
        )
        self._session.on_event(
            {
                "callback": "prompt_rendering",
                "template": template,
            }
        )

        await next(context)

        rendered = _safe_serialize(getattr(context, "rendered_prompt", None))
        self._session.on_event(
            {
                "callback": "prompt_rendered",
                "rendered_prompt": rendered,
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

    def __enter__(self) -> "CleargateSKFilter":
        return self

    def __exit__(self, exc_type: Any, exc_val: Any, exc_tb: Any) -> bool:
        status = "failed" if exc_type is not None else "completed"
        self.finish(status)
        return False
