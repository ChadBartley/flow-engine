"""Integration tests with local Ollama via OpenAI-compatible API.

Requires: ollama running locally with ministral-3:3b pulled.
Skip with: pytest tests/test_integration.py -v -k "not integration"
"""

import pytest
import cleargate
from cleargate.sdk_wrapper import patch_all


OLLAMA_BASE_URL = "http://localhost:11434/v1"
OLLAMA_MODEL = "ministral-3:3b"


def _ollama_available():
    try:
        import httpx

        r = httpx.get("http://localhost:11434/api/tags", timeout=2)
        return r.status_code == 200
    except Exception:
        return False


skip_no_ollama = pytest.mark.skipif(
    not _ollama_available(), reason="Ollama not running"
)


@pytest.fixture(autouse=True)
def reset_patches():
    """Reset SDK patches between tests."""
    from cleargate import sdk_wrapper

    sdk_wrapper._patched_openai = False
    sdk_wrapper._patched_anthropic = False
    yield


@skip_no_ollama
def test_ollama_auto_capture():
    """Ollama call via OpenAI SDK is auto-captured via SDK wrapper."""
    import openai

    patch_all()
    client = openai.OpenAI(base_url=OLLAMA_BASE_URL, api_key="ollama")

    with cleargate.observe("ollama-integration") as session:
        response = client.chat.completions.create(
            model=OLLAMA_MODEL,
            messages=[{"role": "user", "content": "Say 'test' and nothing else."}],
            max_tokens=10,
        )
        print(f"Response: {response.choices[0].message.content}")
        print(f"Run ID: {session.run_id}")

    data = session.get_run_data()
    assert isinstance(data, dict)
    assert data["run_id"] == session.run_id
    print(f"Run data keys: {list(data.keys())}")


@skip_no_ollama
def test_trace_decorator_with_ollama():
    """@cleargate.trace(auto_patch=True) captures Ollama calls via OpenAI SDK."""
    import openai

    @cleargate.trace(auto_patch=True)
    def my_agent(query: str) -> str:
        client = openai.OpenAI(base_url=OLLAMA_BASE_URL, api_key="ollama")
        response = client.chat.completions.create(
            model=OLLAMA_MODEL,
            messages=[{"role": "user", "content": query}],
            max_tokens=10,
        )
        return response.choices[0].message.content

    result = my_agent("Say 'hello'")
    print(f"Agent returned: {result}")
    # Note: @trace manages its own session internally; run data not directly accessible here


@skip_no_ollama
def test_ollama_streaming_capture():
    """Streaming responses from Ollama are captured via SDK wrapper."""
    import openai

    patch_all()
    client = openai.OpenAI(base_url=OLLAMA_BASE_URL, api_key="ollama")

    with cleargate.observe("ollama-streaming") as session:
        stream = client.chat.completions.create(
            model=OLLAMA_MODEL,
            messages=[{"role": "user", "content": "Say 'hi'"}],
            max_tokens=10,
            stream=True,
        )
        chunks = []
        for chunk in stream:
            if chunk.choices and chunk.choices[0].delta.content:
                chunks.append(chunk.choices[0].delta.content)
        print(f"Streamed: {''.join(chunks)}")
        print(f"Run ID: {session.run_id}")

    data = session.get_run_data()
    assert isinstance(data, dict)
    assert data["run_id"] == session.run_id
    print(f"Run data keys: {list(data.keys())}")
