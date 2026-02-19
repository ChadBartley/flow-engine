"""LangGraph observability example with ClearGate.

Demonstrates automatic capture of graph state transitions
using CleargateLangGraphHandler.

Requires: Ollama running locally with qwen3:4b model.
"""

import json
from langchain_ollama import ChatOllama
from langchain_core.messages import HumanMessage
from langgraph.graph import StateGraph, MessagesState, START, END
from cleargate import observe
from cleargate.langgraph import CleargateLangGraphHandler


def chatbot(state: MessagesState) -> MessagesState:
    """Simple chatbot node that calls the LLM."""
    llm = ChatOllama(model="qwen3:4b")
    response = llm.invoke(state["messages"])
    return {"messages": [response]}


def build_graph() -> StateGraph:
    """Build a simple single-node chat graph."""
    graph = StateGraph(MessagesState)
    graph.add_node("chatbot", chatbot)
    graph.add_edge(START, "chatbot")
    graph.add_edge("chatbot", END)
    return graph.compile()


def main():
    with observe("langgraph-example") as session:
        handler = CleargateLangGraphHandler(session=session)
        graph = build_graph()

        print("Asking: Tell me a short joke.")
        result = graph.invoke(
            {"messages": [HumanMessage(content="Tell me a short joke.")]},
            config={"callbacks": [handler]},
        )

        last_message = result["messages"][-1]
        print(f"Response: {last_message.content}")

        events = session.get_events()
        print(f"\nCaptured {len(json.loads(events))} events")
        print(f"Run ID: {session.run_id}")


if __name__ == "__main__":
    main()
