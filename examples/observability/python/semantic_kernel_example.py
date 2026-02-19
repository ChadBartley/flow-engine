"""Semantic Kernel observability example with ClearGate.

Demonstrates auto-capture of SK function invocations
using CleargateSKFilter.

Requires: Ollama running locally with qwen3:4b model.
"""

import asyncio
import json
from semantic_kernel import Kernel
from semantic_kernel.connectors.ai.open_ai import OpenAIChatCompletion
from semantic_kernel.contents import ChatHistory
from cleargate import observe
from cleargate.semantic_kernel import CleargateSKFilter


async def main():
    with observe("semantic-kernel-example") as session:
        kernel = Kernel()

        # Ollama exposes an OpenAI-compatible API
        chat_service = OpenAIChatCompletion(
            ai_model_id="qwen3:4b",
            api_key="ollama",
            base_url="http://localhost:11434/v1",
        )
        kernel.add_service(chat_service)

        # Add ClearGate filter for auto-capture
        sk_filter = CleargateSKFilter(session=session)
        kernel.add_filter("function_invocation", sk_filter)

        chat = ChatHistory()
        chat.add_user_message("What is the capital of France? Answer briefly.")

        result = await chat_service.get_chat_message_content(
            chat_history=chat,
            settings=None,
            kernel=kernel,
        )
        print(f"Response: {result}")

        events = session.get_events()
        print(f"\nCaptured {len(json.loads(events))} events")
        print(f"Run ID: {session.run_id}")


if __name__ == "__main__":
    asyncio.run(main())
