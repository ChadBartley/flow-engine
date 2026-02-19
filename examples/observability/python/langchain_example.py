"""LangChain observability example with ClearGate.

Uses Ollama as the LLM provider and demonstrates automatic
capture of LLM calls and tool invocations via CleargateLangChainHandler.

Requires: Ollama running locally with qwen3:4b model.
"""

import json
from langchain_ollama import ChatOllama
from langchain_core.tools import tool
from langchain_core.messages import HumanMessage, ToolMessage
from cleargate.langchain import CleargateLangChainHandler


@tool
def add(a: int, b: int) -> int:
    """Add two numbers together."""
    return a + b


@tool
def multiply(a: int, b: int) -> int:
    """Multiply two numbers together."""
    return a * b


TOOLS = [add, multiply]
TOOLS_BY_NAME = {t.name: t for t in TOOLS}


def main():
    with CleargateLangChainHandler("langchain-example", store_path="sqlite://cleargate_runs.db?mode=rwc") as handler:
        llm = ChatOllama(model="qwen3:4b", callbacks=[handler])
        llm_with_tools = llm.bind_tools(TOOLS)

        messages = [HumanMessage(content="What is 3 + 4? Use the add tool.")]

        # Agent loop: call LLM, execute any tool calls, repeat until done
        while True:
            response = llm_with_tools.invoke(messages, config={"callbacks": [handler]})
            messages.append(response)

            if not response.tool_calls:
                break

            for tc in response.tool_calls:
                tool_fn = TOOLS_BY_NAME[tc["name"]]
                result = tool_fn.invoke(tc["args"], config={"callbacks": [handler]})
                messages.append(ToolMessage(content=str(result), tool_call_id=tc["id"]))

        print(f"Response: {response.content}")

    # Print captured events after session is finished
    events = handler.get_events()
    print(f"\nCaptured {len(events)} events")
    for i, event in enumerate(events):
        print(f"\n--- Event {i + 1} ---")
        print(json.dumps(event, indent=2))
    print(f"\nRun ID: {handler.run_id}")


if __name__ == "__main__":
    main()
