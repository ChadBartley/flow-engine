"""Auto-patch OpenAI and Anthropic SDKs to record LLM calls.

Usage::

    import cleargate
    from cleargate.sdk_wrapper import patch_all

    patch_all()

    # Now all openai/anthropic calls are automatically recorded
    # through the active cleargate session.

Or via the decorator::

    @cleargate.trace(auto_patch=True)
    def my_agent():
        ...
"""

from __future__ import annotations

import time
from typing import Any

_patched_openai = False
_patched_anthropic = False


def patch_all() -> None:
    """Patch all supported SDKs."""
    patch_openai()
    patch_anthropic()


def patch_openai() -> None:
    """Monkey-patch openai.chat.completions.create to record calls."""
    global _patched_openai
    if _patched_openai:
        return

    try:
        import openai  # noqa: F401
    except ImportError:
        return

    _patched_openai = True
    _patch_openai_sync()
    _patch_openai_async()


def patch_anthropic() -> None:
    """Monkey-patch anthropic.Anthropic().messages.create to record calls."""
    global _patched_anthropic
    if _patched_anthropic:
        return

    try:
        import anthropic  # noqa: F401
    except ImportError:
        return

    _patched_anthropic = True
    _patch_anthropic_sync()
    _patch_anthropic_async()


def _get_session():
    """Get the active cleargate session, if any."""
    from cleargate import get_active_session

    return get_active_session()


def _extract_openai_request(kwargs: dict) -> dict:
    """Extract request fields from openai.chat.completions.create kwargs."""
    return {
        "provider": "openai",
        "model": kwargs.get("model", "unknown"),
        "messages": [
            {"role": m.get("role", ""), "content": m.get("content", "")}
            for m in kwargs.get("messages", [])
        ],
        "temperature": kwargs.get("temperature"),
        "max_tokens": kwargs.get("max_tokens"),
        "tools": kwargs.get("tools"),
    }


def _extract_openai_response(response: Any) -> dict:
    """Extract response fields from openai ChatCompletion response."""
    result: dict = {
        "model": getattr(response, "model", "unknown"),
        "finish_reason": "stop",
    }

    # Extract content
    choices = getattr(response, "choices", [])
    if choices:
        choice = choices[0]
        message = getattr(choice, "message", None)
        if message:
            result["content"] = getattr(message, "content", "") or ""
        result["finish_reason"] = getattr(choice, "finish_reason", "stop") or "stop"

    # Extract usage
    usage = getattr(response, "usage", None)
    if usage:
        result["usage"] = {
            "prompt_tokens": getattr(usage, "prompt_tokens", None),
            "completion_tokens": getattr(usage, "completion_tokens", None),
            "total_tokens": getattr(usage, "total_tokens", None),
        }

    result["id"] = getattr(response, "id", None)
    return result


def _extract_openai_stream_response(chunks: list) -> dict:
    """Reconstruct a response from accumulated stream chunks."""
    content_parts = []
    model = "unknown"
    finish_reason = "stop"
    usage = {}

    for chunk in chunks:
        model = getattr(chunk, "model", model) or model
        for choice in getattr(chunk, "choices", []):
            delta = getattr(choice, "delta", None)
            if delta:
                c = getattr(delta, "content", None)
                if c:
                    content_parts.append(c)
            fr = getattr(choice, "finish_reason", None)
            if fr:
                finish_reason = fr

        u = getattr(chunk, "usage", None)
        if u:
            usage = {
                "prompt_tokens": getattr(u, "prompt_tokens", None),
                "completion_tokens": getattr(u, "completion_tokens", None),
                "total_tokens": getattr(u, "total_tokens", None),
            }

    return {
        "content": "".join(content_parts),
        "model": model,
        "finish_reason": finish_reason,
        "usage": usage,
    }


def _patch_openai_sync() -> None:
    """Patch the sync openai.chat.completions.create method."""
    try:
        from openai.resources.chat.completions import Completions
    except ImportError:
        return

    original = Completions.create

    def patched_create(self, *args, **kwargs):
        session = _get_session()
        stream = kwargs.get("stream", False)

        if not session:
            return original(self, *args, **kwargs)

        start = time.monotonic()
        request = _extract_openai_request(kwargs)

        if stream:
            response_iter = original(self, *args, **kwargs)
            return _wrap_openai_stream(response_iter, session, request, start)

        response = original(self, *args, **kwargs)
        duration_ms = int((time.monotonic() - start) * 1000)

        resp_data = _extract_openai_response(response)
        resp_data["latency_ms"] = duration_ms
        session.record_llm_call("openai", request, resp_data)

        return response

    Completions.create = patched_create


def _wrap_openai_stream(response_iter, session, request, start):
    """Wrap an OpenAI streaming response to record the full call on completion."""
    chunks = []
    for chunk in response_iter:
        chunks.append(chunk)
        yield chunk

    duration_ms = int((time.monotonic() - start) * 1000)
    resp_data = _extract_openai_stream_response(chunks)
    resp_data["latency_ms"] = duration_ms
    session.record_llm_call("openai", request, resp_data)


def _patch_openai_async() -> None:
    """Patch the async openai.chat.completions.create method."""
    try:
        from openai.resources.chat.completions import AsyncCompletions
    except ImportError:
        return

    original = AsyncCompletions.create

    async def patched_create(self, *args, **kwargs):
        session = _get_session()
        stream = kwargs.get("stream", False)

        if not session:
            return await original(self, *args, **kwargs)

        start = time.monotonic()
        request = _extract_openai_request(kwargs)

        if stream:
            response_iter = await original(self, *args, **kwargs)
            return _wrap_openai_async_stream(response_iter, session, request, start)

        response = await original(self, *args, **kwargs)
        duration_ms = int((time.monotonic() - start) * 1000)

        resp_data = _extract_openai_response(response)
        resp_data["latency_ms"] = duration_ms
        session.record_llm_call("openai", request, resp_data)

        return response

    AsyncCompletions.create = patched_create


async def _wrap_openai_async_stream(response_iter, session, request, start):
    """Wrap an async OpenAI streaming response."""
    chunks = []
    async for chunk in response_iter:
        chunks.append(chunk)
        yield chunk

    duration_ms = int((time.monotonic() - start) * 1000)
    resp_data = _extract_openai_stream_response(chunks)
    resp_data["latency_ms"] = duration_ms
    session.record_llm_call("openai", request, resp_data)


def _extract_anthropic_request(kwargs: dict) -> dict:
    """Extract request fields from anthropic.messages.create kwargs."""
    return {
        "provider": "anthropic",
        "model": kwargs.get("model", "unknown"),
        "messages": kwargs.get("messages", []),
        "max_tokens": kwargs.get("max_tokens"),
        "temperature": kwargs.get("temperature"),
        "tools": kwargs.get("tools"),
    }


def _extract_anthropic_response(response: Any) -> dict:
    """Extract response fields from anthropic Message response."""
    content_blocks = getattr(response, "content", [])
    text_parts = []
    for block in content_blocks:
        if getattr(block, "type", None) == "text":
            text_parts.append(getattr(block, "text", ""))

    usage = getattr(response, "usage", None)
    result: dict = {
        "content": "\n".join(text_parts),
        "model": getattr(response, "model", "unknown"),
        "finish_reason": getattr(response, "stop_reason", "end_turn") or "end_turn",
        "id": getattr(response, "id", None),
    }

    if usage:
        result["usage"] = {
            "input_tokens": getattr(usage, "input_tokens", None),
            "output_tokens": getattr(usage, "output_tokens", None),
        }

    return result


def _patch_anthropic_sync() -> None:
    """Patch the sync anthropic.messages.create method."""
    try:
        from anthropic.resources.messages import Messages
    except ImportError:
        return

    original = Messages.create

    def patched_create(self, *args, **kwargs):
        session = _get_session()
        stream = kwargs.get("stream", False)

        if not session:
            return original(self, *args, **kwargs)

        start = time.monotonic()
        request = _extract_anthropic_request(kwargs)

        if stream:
            # Anthropic streaming returns a context manager, not a simple iterator
            return original(self, *args, **kwargs)

        response = original(self, *args, **kwargs)
        duration_ms = int((time.monotonic() - start) * 1000)

        resp_data = _extract_anthropic_response(response)
        resp_data["latency_ms"] = duration_ms
        session.record_llm_call("anthropic", request, resp_data)

        return response

    Messages.create = patched_create


def _patch_anthropic_async() -> None:
    """Patch the async anthropic.messages.create method."""
    try:
        from anthropic.resources.messages import AsyncMessages
    except ImportError:
        return

    original = AsyncMessages.create

    async def patched_create(self, *args, **kwargs):
        session = _get_session()
        stream = kwargs.get("stream", False)

        if not session:
            return await original(self, *args, **kwargs)

        start = time.monotonic()
        request = _extract_anthropic_request(kwargs)

        if stream:
            return await original(self, *args, **kwargs)

        response = await original(self, *args, **kwargs)
        duration_ms = int((time.monotonic() - start) * 1000)

        resp_data = _extract_anthropic_response(response)
        resp_data["latency_ms"] = duration_ms
        session.record_llm_call("anthropic", request, resp_data)

        return response

    AsyncMessages.create = patched_create
