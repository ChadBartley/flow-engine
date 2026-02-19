# Cleargate

LLM observability, replay, and testing for AI agent pipelines.

## Installation

```bash
pip install cleargate
```

With optional framework integrations:

```bash
pip install cleargate[langchain]    # LangChain/LangGraph support
pip install cleargate[openai]       # OpenAI SDK auto-patching
pip install cleargate[anthropic]    # Anthropic SDK auto-patching
pip install cleargate[all]          # Everything
```

## Quick Start

### Context Manager

```python
import cleargate

with cleargate.observe("my-flow") as session:
    session.record_llm_call("llm-1", request_dict, response_dict)
```

### Decorator

```python
@cleargate.trace
def my_agent(query: str):
    ...
```

### Full Agent Capture with LangChain

The fastest way to capture all events from a LangChain agent in a format
ready for Cleargate's DiffEngine and ReplayEngine:

```python
import json
from langchain.agents import create_agent
from langchain_ollama import ChatOllama
from langchain_core.tools import tool
from cleargate.langchain import CleargateLangChainHandler

@tool
def calculator(expression: str) -> str:
    """Evaluate a math expression. Input should be a valid Python math expression."""
    known = {
        "25 * 4": "100",
        "100 + 200": "300",
        "10 / 3": "3.3333333333333335",
    }
    return known.get(expression.strip(), "42")


@tool
def weather(city: str) -> str:
    """Get the current weather for a city."""
    data = {
        "new york": "72°F, sunny",
        "london": "58°F, cloudy",
        "tokyo": "80°F, humid",
    }
    return data.get(city.lower(), "65°F, partly cloudy")


TOOLS = [calculator, weather]

llm = ChatOllama(model="qwen3:4b", temperature=0)
agent = create_agent(llm, TOOLS)

question = "Use the weather tool to check Tokyo's weather, then use the calculator tool to compute 25 * 4."

with CleargateLangChainHandler("full-capture-agent") as handler:
    result = agent.invoke(
        {"messages": [("user", question)]},
        config={"callbacks": [handler]},
    )
    final_answer = result["messages"][-1].content

print(f"\nAnswer: {final_answer}")

# Run summary — metadata, status, timing, LLM token stats
data = handler.get_run_data()
print(f"\n{'='*60}")
print("RUN SUMMARY (get_run_data())")
print("=" * 60)
print(json.dumps(data, indent=2))

# Full event log — every LLM call, tool invocation, and step
# This is the detailed record used by DiffEngine and ReplayEngine
events = handler.get_events()
print(f"\nEVENTS ({len(events)} total)")
for i, event in enumerate(events):
    print(f"\n--- Event {i+1} ---")
    print(json.dumps(event, indent=2))
```

Together, `get_run_data()` (run summary) and `get_events()` (full event log)
give you the complete picture. The event log is what powers
`cleargate diff-runs` to compare two executions and `cleargate replay` to
re-execute individual nodes with different models or configurations.

### LangGraph Integration

```python
from cleargate.langgraph import CleargateLangGraphHandler

with CleargateLangGraphHandler("my-graph") as handler:
    app = graph.compile()
    result = app.invoke(
        {"messages": [("user", "hello")]},
        config={"callbacks": [handler]},
    )
```

### Semantic Kernel Integration

```python
from cleargate.semantic_kernel import CleargateSKFilter

sk_filter = CleargateSKFilter(name="my-kernel")
# Add as a filter to your Semantic Kernel instance
```

### SDK Auto-Patching

```python
from cleargate.sdk_wrapper import patch_all

patch_all()  # Patches openai and anthropic SDKs to auto-record calls
```

## Requirements

- Python 3.9+
- Rust toolchain (for building from source)

## License

Apache-2.0
