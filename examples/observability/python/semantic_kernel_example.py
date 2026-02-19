"""Semantic Kernel observability example with ClearGate.

Demonstrates auto-capture of SK function invocations
using CleargateSKFilter.

Requires: Ollama running locally with qwen3:4b model.
"""

import asyncio
from openai import AsyncOpenAI
from semantic_kernel import Kernel
from semantic_kernel.connectors.ai.open_ai import OpenAIChatCompletion, OpenAIChatPromptExecutionSettings
from semantic_kernel.contents import ChatHistory
from cleargate.semantic_kernel import CleargateSKFilter


async def main():
    with CleargateSKFilter("semantic-kernel-example", store_path="sqlite://cleargate_runs.db?mode=rwc") as sk_filter:
        kernel = Kernel()

        # Ollama exposes an OpenAI-compatible API — pass a pre-configured client
        client = AsyncOpenAI(api_key="ollama", base_url="http://localhost:11434/v1")
        chat_service = OpenAIChatCompletion(
            ai_model_id="qwen3:4b",
            async_client=client,
        )
        kernel.add_service(chat_service)

        # Add ClearGate filter for auto-capture
        kernel.add_filter("function_invocation", sk_filter.function_invocation_filter)

        chat = ChatHistory()
        chat.add_user_message("What is the capital of France? Answer briefly.")

        settings = OpenAIChatPromptExecutionSettings(ai_model_id="qwen3:4b")
        result = await sk_filter.invoke_chat(
            chat_service,
            chat_history=chat,
            settings=settings,
            kernel=kernel,
        )
        print(f"Response: {result}")

    events = sk_filter.get_events()
    print(f"\nCaptured {len(events)} events")
    print(f"Run ID: {sk_filter.run_id}")


if __name__ == "__main__":
    asyncio.run(main())
