use crate::pricing::{ModelPricing, PricingCalculator};
use crate::traits::*;
use async_trait::async_trait;
use futures::stream::{self, Stream};
use futures::StreamExt;
use rand::Rng;
use std::pin::Pin;
use std::time::Duration;

/// MockProvider provides zero-overhead benchmarking of gateway performance
///
/// This provider returns instant in-memory responses without any external HTTP calls,
/// allowing you to measure pure gateway overhead without network latency or external
/// service dependencies.
#[derive(Debug, Clone)]
pub struct MockProvider {
    name: String,
    latency_ms: u64,
    latency_variance_ms: u64,
    num_chunks: usize,
    pricing: PricingCalculator,
}

impl MockProvider {
    /// Create a new MockProvider with configurable latency
    ///
    /// # Arguments
    /// * `name` - Provider name (e.g., "mock-openai-4", "mock-anthropic")
    /// * `latency_ms` - Base simulated latency in milliseconds (0 for instant responses)
    /// * `latency_variance_ms` - Maximum variation from base latency (adds randomness)
    /// * `num_chunks` - Number of chunks to split streaming responses into (default: 20)
    /// * `models` - List of (model_name, pricing) tuples for cost tracking
    pub fn new(
        name: String,
        latency_ms: u64,
        latency_variance_ms: u64,
        num_chunks: usize,
        models: Vec<(String, ModelPricing)>,
    ) -> Self {
        let mut pricing = PricingCalculator::new();
        for (model, model_pricing) in models {
            pricing.add_model(model, model_pricing);
        }

        Self {
            name,
            latency_ms,
            latency_variance_ms,
            num_chunks: if num_chunks == 0 { 20 } else { num_chunks },
            pricing,
        }
    }

    /// Calculate actual latency with variation
    fn calculate_latency(&self) -> u64 {
        if self.latency_variance_ms == 0 {
            return self.latency_ms;
        }
        let mut rng = rand::rng();
        let variance = rng.random_range(0..=self.latency_variance_ms);
        // Variance can be added or subtracted (50% chance each)
        if rng.random_bool(0.5) {
            self.latency_ms.saturating_add(variance)
        } else {
            self.latency_ms
                .saturating_sub(variance.min(self.latency_ms))
        }
    }

    /// Create a MockProvider with instant responses (0ms latency)
    pub fn instant(models: Vec<(String, ModelPricing)>) -> Self {
        Self::new("mock".to_string(), 0, 0, 20, models)
    }

    /// Create a MockProvider with realistic network latency (200ms)
    pub fn realistic(models: Vec<(String, ModelPricing)>) -> Self {
        Self::new("mock".to_string(), 200, 0, 20, models)
    }

    /// Generate a mock response matching OpenAI format
    ///
    /// Token calculation:
    /// - prompt_tokens: Estimated from message content (~4 chars per token)
    /// - completion_tokens: Based on max_tokens if provided, otherwise random range
    fn generate_response(&self, req: &CompletionRequest) -> CompletionResponse {
        // Calculate prompt tokens from actual message content
        // Heuristic: ~4 characters per token (OpenAI/Anthropic average)
        let total_chars: usize = req.messages.iter().map(|msg| msg.content.len()).sum();
        let prompt_tokens = (total_chars / 4).max(1) as u64;

        // Calculate completion tokens
        // If max_tokens is specified, use 50-100% of it
        // Otherwise, return a random value between 10-50 tokens
        let completion_tokens = if let Some(max) = req.max_tokens {
            // Use 50-100% of max_tokens to simulate realistic variance
            let min = max / 2;
            (min + (max - min) / 2) as u64
        } else {
            // Default range for unspecified max_tokens
            30
        };

        CompletionResponse {
            content: "Mock response for benchmarking".to_string(),
            prompt_tokens,
            completion_tokens,
            model: req.model.clone(),
            provider: self.name.clone(),
            tool_calls: None,
            finish_reason: "stop".to_string(),
        }
    }
}

#[async_trait]
impl LLMProvider for MockProvider {
    async fn complete(&self, req: CompletionRequest) -> ProviderResult<CompletionResponse> {
        // Simulate latency with variation if configured
        let actual_latency = self.calculate_latency();
        if actual_latency > 0 {
            tokio::time::sleep(Duration::from_millis(actual_latency)).await;
        }

        Ok(self.generate_response(&req))
    }

    async fn stream(
        &self,
        req: CompletionRequest,
    ) -> ProviderResult<Pin<Box<dyn Stream<Item = ProviderResult<StreamChunk>> + Send>>> {
        // Calculate actual latency with variation for this stream
        let actual_latency = self.calculate_latency();
        let num_chunks = self.num_chunks;
        let chunk_delay = if actual_latency > 0 {
            actual_latency / num_chunks as u64
        } else {
            0
        };

        // Calculate realistic token counts based on input
        let total_chars: usize = req.messages.iter().map(|msg| msg.content.len()).sum();
        let prompt_tokens = (total_chars / 4).max(1) as u64;

        let completion_tokens = if let Some(max) = req.max_tokens {
            let min = max / 2;
            (min + (max - min) / 2) as u64
        } else {
            30
        };

        // Split the mock response into chunks
        let full_response = "Mock response for benchmarking";
        let chars_per_chunk = full_response.len() / num_chunks;

        let mut chunks = Vec::new();
        for i in 0..num_chunks {
            let start = i * chars_per_chunk;
            let end = if i == num_chunks - 1 {
                full_response.len()
            } else {
                (i + 1) * chars_per_chunk
            };

            let content = full_response[start..end].to_string();
            let is_final = i == num_chunks - 1;

            chunks.push(StreamChunk {
                content,
                is_final,
                prompt_tokens: if is_final { Some(prompt_tokens) } else { None },
                completion_tokens: if is_final {
                    Some(completion_tokens)
                } else {
                    None
                },
            });
        }

        // Create a stream that yields chunks with configured delay
        let stream = stream::iter(chunks.into_iter().enumerate().map(move |(i, chunk)| {
            async move {
                // Add delay between chunks if configured
                if chunk_delay > 0 && i > 0 {
                    tokio::time::sleep(Duration::from_millis(chunk_delay)).await;
                }
                Ok(chunk)
            }
        }))
        .then(|fut| fut);

        Ok(Box::pin(stream))
    }

    fn get_pricing(&self, model: &str) -> Option<ModelPricing> {
        self.pricing.get_pricing(model).cloned()
    }

    fn name(&self) -> &str {
        &self.name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_instant_response() {
        let provider = MockProvider::instant(vec![(
            "mock-model".to_string(),
            ModelPricing {
                prompt: 0.0001,
                completion: 0.0002,
            },
        )]);

        let req = CompletionRequest {
            model: "mock-model".to_string(),
            messages: vec![Message {
                role: "user".to_string(),
                content: "test".to_string(),
                tool_calls: None,
                tool_call_id: None,
                name: None,
            }],
            temperature: None,
            max_tokens: None,
            stream: false,
            tools: None,
        };

        let start = std::time::Instant::now();
        let response = provider.complete(req).await.unwrap();
        let elapsed = start.elapsed();

        assert_eq!(response.content, "Mock response for benchmarking");
        assert_eq!(response.provider, "mock");
        assert!(elapsed.as_millis() < 10, "Should be instant");
    }

    #[tokio::test]
    async fn test_latency_simulation() {
        let provider = MockProvider::new("mock".to_string(), 100, 0, 20, vec![]);

        let req = CompletionRequest {
            model: "mock-model".to_string(),
            messages: vec![],
            temperature: None,
            max_tokens: None,
            stream: false,
            tools: None,
        };

        let start = std::time::Instant::now();
        provider.complete(req).await.unwrap();
        let elapsed = start.elapsed();

        assert!(elapsed.as_millis() >= 100, "Should have 100ms latency");
        assert!(elapsed.as_millis() < 150, "Should not exceed 150ms");
    }

    #[tokio::test]
    async fn test_streaming() {
        use futures::StreamExt;

        let provider = MockProvider::instant(vec![]);

        let req = CompletionRequest {
            model: "mock-model".to_string(),
            messages: vec![],
            temperature: None,
            max_tokens: None,
            stream: true,
            tools: None,
        };

        let mut stream = provider.stream(req).await.unwrap();
        let mut chunks_received = 0;
        let mut full_content = String::new();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.unwrap();
            full_content.push_str(&chunk.content);
            chunks_received += 1;

            if chunk.is_final {
                assert_eq!(chunks_received, 20);
                assert!(chunk.prompt_tokens.is_some());
                assert!(chunk.completion_tokens.is_some());
            }
        }

        assert_eq!(full_content, "Mock response for benchmarking");
    }

    #[tokio::test]
    async fn test_pricing() {
        let provider = MockProvider::instant(vec![(
            "mock-model".to_string(),
            ModelPricing {
                prompt: 0.0001,
                completion: 0.0002,
            },
        )]);

        let pricing = provider.get_pricing("mock-model").unwrap();
        assert_eq!(pricing.prompt, 0.0001);
        assert_eq!(pricing.completion, 0.0002);
    }
}
