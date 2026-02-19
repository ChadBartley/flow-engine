# cleargate-providers

Unified LLM provider interface for ClearGate. Abstracts over multiple LLM backends with a consistent API for chat completions, streaming, and cost calculation.

## Supported Providers

| Provider | Features |
|----------|----------|
| **OpenAI** | GPT models, function calling, streaming |
| **Anthropic** | Claude models, tool use, streaming |
| **Gemini** | Google AI models, function calling |
| **Ollama** | Local model hosting, streaming |
| **OpenAI-compatible** | Any API following the OpenAI chat completions spec |
| **Mock** | Deterministic responses for testing |

## Key Features

- **Streaming**: All providers support streaming responses via a unified stream interface
- **Pricing Calculator**: Built-in token cost calculation per provider and model
- **Tool/Function Calling**: Normalized tool call interface across providers
- **Retry & Rate Limiting**: Configurable per-provider retry policies

## Usage

```rust
use cleargate_providers::{ProviderConfig, LlmProvider};

let provider = LlmProvider::from_config(ProviderConfig::OpenAi {
    api_key: std::env::var("OPENAI_API_KEY")?,
    model: "gpt-4o".into(),
})?;

let response = provider.chat(messages).await?;
println!("Cost: ${:.4}", response.usage.cost);
```

## License

Apache-2.0
