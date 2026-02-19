"""Advanced agent observability example with ClearGate.

Demonstrates a multi-tool agent with full event capture, streaming output,
and persistent storage — showing the complete value of ClearGate observability
for debugging and understanding agent behavior.

Requires: Ollama running locally with ministral-3:3b model.
"""

import json
import sys
from collections import Counter

from langchain_ollama import ChatOllama
from langchain_core.tools import tool
from langchain_core.messages import HumanMessage, ToolMessage
from cleargate.langchain import CleargateLangChainHandler


@tool
def get_weather(city: str) -> str:
    """Get the current weather for a city."""
    # Simulated weather data
    weather = {
        "london": "Cloudy, 12°C, 80% humidity",
        "paris": "Sunny, 18°C, 45% humidity",
        "tokyo": "Rainy, 15°C, 90% humidity",
        "new york": "Partly cloudy, 22°C, 55% humidity",
    }
    return weather.get(city.lower(), f"Weather data not available for {city}")


@tool
def calculate(expression: str) -> str:
    """Evaluate a simple arithmetic expression (digits, +, -, *, /, parentheses only)."""
    allowed = set("0123456789+-*/.(). ")
    if not all(c in allowed for c in expression):
        return f"Invalid expression: {expression}"
    try:
        # SAFETY: input is restricted to arithmetic characters above.
        # This is an example only — production code should use a proper parser.
        result = eval(expression)  # noqa: S307
        return str(result)
    except Exception as e:
        return f"Error: {e}"


@tool
def unit_convert(value: float, from_unit: str, to_unit: str) -> str:
    """Convert between temperature units (celsius, fahrenheit, kelvin)."""
    conversions = {
        ("celsius", "fahrenheit"): lambda v: v * 9 / 5 + 32,
        ("fahrenheit", "celsius"): lambda v: (v - 32) * 5 / 9,
        ("celsius", "kelvin"): lambda v: v + 273.15,
        ("kelvin", "celsius"): lambda v: v - 273.15,
    }
    key = (from_unit.lower(), to_unit.lower())
    if key not in conversions:
        return f"Unsupported conversion: {from_unit} -> {to_unit}"
    result = conversions[key](value)
    return f"{value} {from_unit} = {result:.2f} {to_unit}"


TOOLS = [get_weather, calculate, unit_convert]
TOOLS_BY_NAME = {t.name: t for t in TOOLS}


def print_event_summary(events: list[dict]) -> None:
    """Print a formatted summary of captured events."""
    type_counts = Counter(e.get("event_type", "unknown") for e in events)

    print("\n" + "=" * 60)
    print("EVENT SUMMARY")
    print("=" * 60)
    print(f"Total events: {len(events)}")
    for event_type, count in sorted(type_counts.items()):
        print(f"  {event_type}: {count}")

    print("\n" + "-" * 60)
    print("EVENT DETAILS")
    print("-" * 60)

    for i, event in enumerate(events):
        event_type = event.get("type", "unknown")
        print(f"\n--- Event {i + 1}: {event_type} ---")
        print(json.dumps(event, indent=2, default=str))


def stream_agent_step(llm_with_tools, messages, handler):
    """Stream one agent step, printing tokens as they arrive.

    Returns the fully assembled AIMessage (with tool_calls populated).
    """
    collected = None
    token_count = 0
    for chunk in llm_with_tools.stream(
        messages, config={"callbacks": [handler]}
    ):
        # Accumulate chunks into a single message
        if collected is None:
            collected = chunk
        else:
            collected = collected + chunk

        token_count += 1

        # Print content tokens as they stream in
        if chunk.content:
            sys.stdout.write(chunk.content)
            sys.stdout.flush()
        elif token_count % 20 == 0:
            # Show progress dots while model is thinking (no visible content yet)
            sys.stdout.write(".")
            sys.stdout.flush()

    # Newline after streaming finishes
    print()

    return collected


def main():
    with CleargateLangChainHandler(
        "advanced-agent",
        store_path="sqlite://cleargate_runs.db?mode=rwc",
    ) as handler:
        llm = ChatOllama(model="ministral-3:3b", callbacks=[handler])
        llm_with_tools = llm.bind_tools(TOOLS)

        query = (
            "What's the weather in Paris? "
            "Convert the temperature from Celsius to Fahrenheit, "
            "then calculate 18 * 9 / 5 + 32 to verify."
        )
        messages = [HumanMessage(content=query)]

        print(f"Query: {query}")
        print("-" * 60)

        iteration = 0
        while True:
            iteration += 1
            print(f"\n[Agent iteration {iteration}] Thinking...", flush=True)

            response = stream_agent_step(llm_with_tools, messages, handler)
            messages.append(response)

            if not response.tool_calls:
                break

            for tc in response.tool_calls:
                print(f"  Tool call: {tc['name']}({json.dumps(tc['args'])})")
                tool_fn = TOOLS_BY_NAME[tc["name"]]
                result = tool_fn.invoke(
                    tc["args"], config={"callbacks": [handler]}
                )
                print(f"  Result: {result}")
                messages.append(
                    ToolMessage(content=str(result), tool_call_id=tc["id"])
                )

    # Session is finished after context manager exits — print captured data
    events = handler.get_events()
    print_event_summary(events)

    run_data = handler.get_run_data()
    print("\n" + "=" * 60)
    print("RUN DATA")
    print("=" * 60)
    print(json.dumps(run_data, indent=2, default=str))

    print(f"\nRun ID: {handler.run_id}")
    print("Run data persisted to cleargate_runs.db")


if __name__ == "__main__":
    main()
