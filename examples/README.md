# ClearGate Examples

Example configurations and SDK usage demonstrations.

## Contents

### [flow-engine-agents/](flow-engine-agents/)

Declarative flow configurations that define agent workflows as YAML DAGs. These are loaded and executed by the ClearGate flow engine.

### [observability/](observability/)

SDK usage examples demonstrating ObserverSession and framework adapters across Python, Node.js, and .NET. Each example records LLM interactions to ClearGate for replay and analysis.

## Running the Examples

Most observability examples expect a local Ollama instance. Install Ollama and pull a model:

```bash
ollama pull llama3.2
```

See each subdirectory's README for specific instructions.

## License

Apache-2.0
