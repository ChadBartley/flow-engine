#!/usr/bin/env python3
"""Custom LLM Provider

Demonstrates wrapping an HTTP LLM API (Ollama) as a custom Python LLM
provider. The provider implements:
  - complete(request_dict) -> response_dict
  - name (property)
"""

import json
import os
import shutil
import tempfile
import urllib.request

from cleargate import EngineBuilder


class OllamaProvider:
    """Wraps the Ollama HTTP API as a Cleargate LLM provider."""

    def __init__(self, base_url: str = "http://localhost:11434"):
        self._base_url = base_url

    @property
    def name(self) -> str:
        return "ollama"

    def complete(self, request: dict) -> dict:
        """Call the Ollama chat API and return a response dict."""
        model = request.get("model", "llama3.2:1b")
        messages = request.get("messages", [])

        # Convert to Ollama API format
        ollama_messages = []
        for msg in messages if isinstance(messages, list) else []:
            if isinstance(msg, dict):
                ollama_messages.append(
                    {
                        "role": msg.get("role", "user"),
                        "content": str(msg.get("content", "")),
                    }
                )

        # If no messages, create a default user message from the input
        if not ollama_messages:
            ollama_messages = [{"role": "user", "content": "Hello"}]

        payload = json.dumps(
            {
                "model": model,
                "messages": ollama_messages,
                "stream": False,
            }
        ).encode()

        req = urllib.request.Request(
            f"{self._base_url}/api/chat",
            data=payload,
            headers={"Content-Type": "application/json"},
        )

        try:
            with urllib.request.urlopen(req, timeout=30) as resp:
                data = json.loads(resp.read())
        except Exception as e:
            return {
                "content": f"Error calling Ollama: {e}",
                "model": model,
                "finish_reason": "error",
                "input_tokens": 0,
                "output_tokens": 0,
                "total_tokens": 0,
                "latency_ms": 0,
            }

        msg = data.get("message", {})
        return {
            "content": msg.get("content", ""),
            "model": data.get("model", model),
            "finish_reason": "stop",
            "input_tokens": data.get("prompt_eval_count", 0),
            "output_tokens": data.get("eval_count", 0),
            "total_tokens": (
                data.get("prompt_eval_count", 0) + data.get("eval_count", 0)
            ),
            "latency_ms": int(data.get("total_duration", 0) / 1_000_000),
        }


def main():
    flow_src = os.path.join(
        os.path.dirname(__file__), "..", "flows", "simple-responder.json"
    )

    with tempfile.TemporaryDirectory() as tmpdir:
        flow_dir = os.path.join(tmpdir, "flows")
        run_dir = os.path.join(tmpdir, "runs")

        flow_dest = os.path.join(flow_dir, "simple-responder")
        os.makedirs(flow_dest, exist_ok=True)
        shutil.copy(flow_src, os.path.join(flow_dest, "flow.json"))

        builder = EngineBuilder()
        builder.flow_store_path(flow_dir)
        builder.run_store_path(run_dir)
        builder.crash_recovery(False)

        # Register the custom Ollama provider
        builder.llm_provider("ollama", OllamaProvider())
        engine = builder.build()

        print("Executing simple-responder with custom Ollama provider...")
        handle = engine.execute(
            "simple-responder",
            {
                "messages": [
                    {"role": "user", "content": "What is 2+2? Answer in one word."}
                ]
            },
        )
        print(f"Run ID: {handle.run_id}")

        for event in handle:
            for event_type, payload in event.items():
                print(f"  [{event_type}]")
                if event_type == "NodeCompleted" and isinstance(payload, dict):
                    outputs = payload.get("outputs", {})
                    content = outputs.get("content", "")
                    if content:
                        print(f"    LLM response: {content[:200]}")

        engine.shutdown()
        print("\nDone!")


if __name__ == "__main__":
    main()
