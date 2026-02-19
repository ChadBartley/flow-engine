"""LangChain observability example with ClearGate.

Uses Ollama as the LLM provider and demonstrates automatic
capture of LLM calls and tool invocations via CleargateLangChainHandler.

Requires: Ollama running locally with qwen3:4b model.
"""

import json
from langchain_ollama import ChatOllama
from langchain_core.tools import tool
from langchain_core.messages import HumanMessage
from cleargate import observe
from cleargate.langchain import CleargateLangChainHandler


@tool
def add(a: int, b: int) -> int:
    """Add two numbers together."""
    return a + b


@tool
def multiply(a: int, b: int) -> int:
    """Multiply two numbers together."""
    return a * b


def main():
    with observe("langchain-example") as session:
        handler = CleargateLangChainHandler(session=session)

        llm = ChatOllama(model="qwen3:4b", callbacks=[handler])
        llm_with_tools = llm.bind_tools([add, multiply])

        print("Asking: What is 3 + 4?")
        response = llm_with_tools.invoke(
            [HumanMessage(content="What is 3 + 4? Use the add tool.")],
            config={"callbacks": [handler]},
        )
        print(f"Response: {response.content}")

        # Print captured events
        events = session.get_events()
        print(f"\nCaptured {len(json.loads(events))} events")
        print(f"Run ID: {session.run_id}")


if __name__ == "__main__":
    main()
