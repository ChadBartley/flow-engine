"""Semantic Kernel filter for Cleargate observability.

Usage::

    from cleargate.semantic_kernel import CleargateSKFilter

    with CleargateSKFilter("my-sk-app") as sk_filter:
        kernel.add_filter("function_invoking", sk_filter.on_function_invoking)
        kernel.add_filter("function_invoked", sk_filter.on_function_invoked)
        result = await kernel.invoke(plugin["func"], arg="value")

    print(sk_filter.get_run_data())
"""

from __future__ import annotations

import json
import time
from typing import Any, Dict, Optional

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


class CleargateSKFilter:
    """Semantic Kernel filter that records events to Cleargate."""

    def __init__(
        self,
        session_name: str = "semantic_kernel",
        *,
        session: Optional[AdapterSession] = None,
    ):
        if session is not None:
            self._session = session
            self._owns_session = False
        else:
            self._session = AdapterSession.start("semantic_kernel", session_name)
            self._owns_session = True
        self._function_start_times: Dict[str, float] = {}

    async def on_function_invoking(self, context: Any) -> None:
        """SK function_invoking filter hook."""
        plugin = getattr(context.function, "plugin_name", "unknown")
        function = getattr(context.function, "name", "unknown")
        key = f"{plugin}.{function}"
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

    async def on_function_invoked(self, context: Any) -> None:
        """SK function_invoked filter hook."""
        plugin = getattr(context.function, "plugin_name", "unknown")
        function = getattr(context.function, "name", "unknown")
        key = f"{plugin}.{function}"
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

    async def on_prompt_rendering(self, context: Any) -> None:
        """SK prompt_rendering filter hook."""
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

    async def on_prompt_rendered(self, context: Any) -> None:
        """SK prompt_rendered filter hook."""
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
