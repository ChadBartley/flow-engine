"""Semantic Kernel full observability example with ClearGate.

Demonstrates tool-calling agent with full event capture, showing
the complete value of ClearGate observability for SK-based agents.

Requires: Ollama running locally with ministral-3:3b model.
"""

import asyncio
import json
from typing import Annotated

from openai import AsyncOpenAI
from semantic_kernel import Kernel
from semantic_kernel.connectors.ai.open_ai import (
    OpenAIChatCompletion,
    OpenAIChatPromptExecutionSettings,
)
from semantic_kernel.connectors.ai.function_choice_behavior import FunctionChoiceBehavior
from semantic_kernel.contents import ChatHistory
from semantic_kernel.functions.kernel_function_decorator import kernel_function

from cleargate.semantic_kernel import CleargateSKFilter


class WeatherPlugin:
    """Simulated weather lookup plugin."""

    @kernel_function(name="get_weather", description="Get the current weather for a city.")
    def get_weather(self, city: Annotated[str, "The city to get weather for"]) -> str:
        weather = {
            "london": "Cloudy, 12°C, 80% humidity",
            "paris": "Sunny, 18°C, 45% humidity",
            "tokyo": "Rainy, 15°C, 90% humidity",
            "new york": "Partly cloudy, 22°C, 55% humidity",
        }
        return weather.get(city.lower(), f"Weather data not available for {city}")


class MathPlugin:
    """Simple arithmetic plugin."""

    @kernel_function(name="add", description="Add two numbers together.")
    def add(
        self,
        a: Annotated[int, "First number"],
        b: Annotated[int, "Second number"],
    ) -> int:
        return a + b

    @kernel_function(name="multiply", description="Multiply two numbers together.")
    def multiply(
        self,
        a: Annotated[int, "First number"],
        b: Annotated[int, "Second number"],
    ) -> int:
        return a * b


async def main():
    with CleargateSKFilter(
        "sk-full-example",
        store_path="sqlite://cleargate_runs.db?mode=rwc",
    ) as sk_filter:
        kernel = Kernel()

        # Ollama via OpenAI-compatible API
        client = AsyncOpenAI(api_key="ollama", base_url="http://localhost:11434/v1")
        chat_service = OpenAIChatCompletion(
            ai_model_id="ministral-3:3b",
            async_client=client,
        )
        kernel.add_service(chat_service)

        # Register plugins
        kernel.add_plugin(WeatherPlugin(), plugin_name="weather")
        kernel.add_plugin(MathPlugin(), plugin_name="math")

        # Add ClearGate filters for auto-capture
        kernel.add_filter("function_invocation", sk_filter.function_invocation_filter)
        kernel.add_filter("auto_function_invocation", sk_filter.auto_function_invocation_filter)

        # Configure auto tool calling
        settings = OpenAIChatPromptExecutionSettings(
            ai_model_id="ministral-3:3b",
            function_choice_behavior=FunctionChoiceBehavior.Auto(),
        )

        chat = ChatHistory()
        query = (
            "Use the get_weather tool to look up the weather in Paris, "
            "and use the add tool to compute 3 + 4. "
            "Report both results."
        )
        chat.add_user_message(query)

        print(f"Query: {query}")
        print("-" * 60)

        result = await sk_filter.invoke_chat(
            chat_service,
            chat_history=chat,
            settings=settings,
            kernel=kernel,
        )

        print(f"\nResponse: {result}")

    # Print captured events after session is finished
    events = sk_filter.get_events()
    print(f"\nCaptured {len(events)} events")
    for i, event in enumerate(events):
        print(f"\n--- Event {i + 1} ---")
        print(json.dumps(event, indent=2, default=str))

    run_data = sk_filter.get_run_data()
    print(f"\n{'=' * 60}")
    print("RUN DATA")
    print("=" * 60)
    print(json.dumps(run_data, indent=2, default=str))
    print(f"\nRun ID: {sk_filter.run_id}")


if __name__ == "__main__":
    asyncio.run(main())
