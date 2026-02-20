"""Basic observability example using ObserverSession directly.

Records LLM calls, tool calls, and steps manually.
Requires: Ollama running locally with qwen3:4b model.
"""

import json
import requests
from cleargate import observe


def call_ollama(prompt: str, model: str = "qwen3:4b") -> dict:
    """Call Ollama's chat completion API."""
    request_body = {
        "model": model,
        "messages": [{"role": "user", "content": prompt}],
        "stream": False,
    }
    resp = requests.post("http://localhost:11434/api/chat", json=request_body)
    resp.raise_for_status()
    return resp.json()


def main():
    with observe(
        "basic-example", store_path="sqlite://cleargate_runs.db?mode=rwc"
    ) as session:
        # Record a step
        session.record_step("setup", json.dumps({"model": "qwen3:4b"}))

        # Make an LLM call and record it
        prompt = "What is 2 + 2? Answer in one word."
        request_data = json.dumps(
            {
                "model": "qwen3:4b",
                "messages": [{"role": "user", "content": prompt}],
            }
        )

        result = call_ollama(prompt)

        response_data = json.dumps(
            {
                "model": result.get("model", "qwen3:4b"),
                "message": result.get("message", {}),
            }
        )

        session.record_llm_call("ollama-chat", request_data, response_data)

        # Record a tool call
        session.record_tool_call(
            "calculator",
            json.dumps({"expression": "2 + 2"}),
            json.dumps({"result": 4}),
            5,
        )

        # Print run data
        run_data = session.get_run_data()
        print(f"Run ID: {session.run_id}")
        print(f"Run data: {json.dumps(run_data, indent=2)}")
        print("Run data persisted to cleargate_runs.db")


if __name__ == "__main__":
    main()
