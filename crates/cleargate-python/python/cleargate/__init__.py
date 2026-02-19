"""Cleargate — LLM observability, replay, and testing.

Usage::

    import cleargate

    # Context manager style
    with cleargate.observe("my-flow") as session:
        session.record_llm_call("llm-1", request_dict, response_dict)

    # Decorator style
    @cleargate.trace
    def my_agent(query: str):
        ...

    # Framework-specific handlers (import from submodules):
    #   from cleargate.langchain import CleargateLangChainHandler
    #   from cleargate.langgraph import CleargateLangGraphHandler
    #   from cleargate.semantic_kernel import CleargateSKFilter
"""

from __future__ import annotations

import contextvars
import functools
import logging
import os
import time
from typing import Any, Callable, Optional, TypeVar

from cleargate._native import AdapterSession, ObserverSession

# Configure cleargate logger based on CLEARGATE_DEBUG env var.
# NullHandler prevents propagation to root logger by default.
_logger = logging.getLogger("cleargate")
_logger.addHandler(logging.NullHandler())
if os.environ.get("CLEARGATE_DEBUG", "").strip() not in ("", "0", "false"):
    _logger.setLevel(logging.DEBUG)
    _handler = logging.StreamHandler()
    _handler.setFormatter(logging.Formatter("%(name)s %(levelname)s: %(message)s"))
    _logger.addHandler(_handler)

__all__ = [
    "ObserverSession",
    "AdapterSession",
    "observe",
    "trace",
    "get_active_session",
]

F = TypeVar("F", bound=Callable[..., Any])

# Active session context variable for decorator/SDK wrapper integration.
_active_session: contextvars.ContextVar[Optional[ObserverSession]] = (
    contextvars.ContextVar("cleargate_active_session", default=None)
)


def get_active_session() -> Optional[ObserverSession]:
    """Get the currently active observer session, if any."""
    return _active_session.get()


class observe:
    """Context manager for recording a flow execution.

    Usage::

        with cleargate.observe("my-flow") as session:
            session.record_step("init", {"ready": True})
    """

    def __init__(self, name: str, *, store_path: Optional[str] = None):
        self._name = name
        self._store_path = store_path
        self._session: Optional[ObserverSession] = None
        self._token: Optional[contextvars.Token] = None

    def __enter__(self) -> ObserverSession:
        self._session = ObserverSession.start(self._name, self._store_path)
        self._token = _active_session.set(self._session)
        return self._session

    def __exit__(self, exc_type, exc_val, exc_tb):
        if self._token is not None:
            _active_session.reset(self._token)
        if self._session is not None:
            status = "failed" if exc_type is not None else "completed"
            try:
                self._session.finish(status)
            except Exception:
                pass
        return False


def trace(
    func: Optional[F] = None,
    *,
    name: Optional[str] = None,
    auto_patch: bool = False,
) -> Any:
    """Decorator for auto-recording function execution.

    Can be used with or without arguments::

        @cleargate.trace
        def my_func():
            ...

        @cleargate.trace(name="custom_name")
        def my_func():
            ...

    If no active session exists, creates one for the top-level call.
    Nested ``@trace`` calls record steps within the existing session.
    """
    if func is None:
        # Called with arguments: @cleargate.trace(name="x")
        return functools.partial(trace, name=name, auto_patch=auto_patch)

    step_name = name or func.__name__

    @functools.wraps(func)
    def wrapper(*args, **kwargs):
        session = _active_session.get()
        is_root = session is None

        if is_root:
            session = ObserverSession.start(step_name)
            token = _active_session.set(session)
        else:
            token = None

        if auto_patch:
            try:
                from cleargate.sdk_wrapper import patch_all

                patch_all()
            except ImportError:
                pass

        start = time.monotonic()
        try:
            result = func(*args, **kwargs)
            if not is_root:
                duration_ms = int((time.monotonic() - start) * 1000)
                session.record_step(
                    step_name,
                    {"status": "completed", "duration_ms": duration_ms},
                )
            return result
        except Exception:
            if not is_root:
                duration_ms = int((time.monotonic() - start) * 1000)
                session.record_step(
                    step_name,
                    {"status": "failed", "duration_ms": duration_ms},
                )
            raise
        finally:
            if is_root:
                try:
                    session.finish("completed")
                except Exception:
                    pass
                if token is not None:
                    _active_session.reset(token)

    return wrapper
