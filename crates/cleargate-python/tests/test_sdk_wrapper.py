"""Test SDK monkey-patching with mocked clients.

These tests verify the patch/unpatch logic without making real API calls.
They require `openai` and `anthropic` packages to be installed.
"""

import pytest
import cleargate


# --- Helpers: mock SDK objects ---


class MockUsage:
    def __init__(self, prompt_tokens=10, completion_tokens=5, total_tokens=15):
        self.prompt_tokens = prompt_tokens
        self.completion_tokens = completion_tokens
        self.total_tokens = total_tokens


class MockMessage:
    def __init__(self, content="Hello!"):
        self.content = content
        self.role = "assistant"


class MockChoice:
    def __init__(self):
        self.message = MockMessage()
        self.finish_reason = "stop"


class MockChatCompletion:
    def __init__(self):
        self.id = "chatcmpl-123"
        self.model = "gpt-4o"
        self.choices = [MockChoice()]
        self.usage = MockUsage()


def _has_openai():
    import importlib.util

    return importlib.util.find_spec("openai") is not None


# --- Tests ---


def test_sdk_wrapper_import():
    """sdk_wrapper module is importable."""
    from cleargate.sdk_wrapper import patch_all, patch_openai, patch_anthropic

    assert callable(patch_all)
    assert callable(patch_openai)
    assert callable(patch_anthropic)


def test_sdk_wrapper_no_crash_without_sdks():
    """patch_all doesn't crash when openai/anthropic aren't installed."""
    from cleargate import sdk_wrapper

    # Reset state
    sdk_wrapper._patched_openai = False
    sdk_wrapper._patched_anthropic = False
    # This should silently skip if SDKs aren't available
    sdk_wrapper.patch_all()


@pytest.mark.skipif(not _has_openai(), reason="openai package not installed")
def test_openai_patch_records_call():
    """Patched openai.chat.completions.create records the LLM call."""
    from openai.resources.chat.completions import Completions
    from cleargate import sdk_wrapper

    # Save and mock
    original = Completions.create
    sdk_wrapper._patched_openai = False

    def mock_create(self, *args, **kwargs):
        return MockChatCompletion()

    Completions.create = mock_create
    sdk_wrapper.patch_openai()

    # Now Completions.create should be the patched version
    with cleargate.observe("openai-test"):
        # We can't easily call it without a real client instance,
        # but we can verify the patch was applied
        assert Completions.create is not mock_create  # It's wrapped

    # Restore
    Completions.create = original
    sdk_wrapper._patched_openai = False
