use crate::pricing::{ModelPricing, PricingCalculator};
use crate::traits::*;
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::Stream;
use futures::TryStreamExt;
use pin_project::pin_project;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::pin::Pin;
use std::task::{Context, Poll};

#[derive(Debug, Clone)]
pub struct OpenAICompatibleProvider {
    client: Client,
    base_url: String,
    api_key: Option<String>,
    api_key_header: String,
    api_key_format: String,
    pricing: PricingCalculator,
}

// Reuse OpenAI's request/response structures
#[derive(Debug, Serialize)]
struct OpenAIRequest {
    model: String,
    messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct OpenAIResponse {
    choices: Vec<OpenAIChoice>,
    usage: OpenAIUsage,
    model: String,
}

#[derive(Debug, Deserialize)]
struct OpenAIChoice {
    message: OpenAIMessage,
}

#[derive(Debug, Deserialize)]
struct OpenAIMessage {
    content: String,
}

#[derive(Debug, Deserialize)]
struct OpenAIUsage {
    prompt_tokens: u64,
    completion_tokens: u64,
}

#[derive(Debug, Deserialize)]
struct OpenAIStreamResponse {
    choices: Vec<OpenAIStreamChoice>,
    #[serde(default)]
    usage: Option<OpenAIUsage>,
}

#[derive(Debug, Deserialize)]
struct OpenAIStreamChoice {
    delta: OpenAIDelta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAIDelta {
    #[serde(default)]
    content: Option<String>,
}

impl StreamResponseTokens for OpenAIStreamResponse {
    fn extract_prompt_tokens(&self) -> Option<u64> {
        let tokens = self.usage.as_ref().map(|u| u.prompt_tokens);

        if let Some(count) = tokens {
            tracing::debug!(
                provider = "openai-compatible",
                prompt_tokens = count,
                "Extracted prompt tokens from stream response"
            );
        }

        tokens
    }

    fn extract_completion_tokens(&self) -> Option<u64> {
        let tokens = self.usage.as_ref().map(|u| u.completion_tokens);

        if let Some(count) = tokens {
            tracing::debug!(
                provider = "openai-compatible",
                completion_tokens = count,
                "Extracted completion tokens from stream response"
            );
        }

        tokens
    }
}

impl OpenAICompatibleProvider {
    /// Create a new OpenAICompatibleProvider
    ///
    /// # Arguments
    /// * `base_url` - Base URL for the OpenAI-compatible API (required)
    /// * `api_key` - Optional API key
    /// * `api_key_header` - Header name for API key (default: "Authorization")
    /// * `api_key_format` - Format string for API key (default: "Bearer {key}", use {key} as placeholder)
    /// * `models` - List of models with their pricing information
    pub fn new(
        base_url: String,
        api_key: Option<String>,
        api_key_header: Option<String>,
        api_key_format: Option<String>,
        models: Vec<(String, ModelPricing)>,
    ) -> Self {
        let mut pricing = PricingCalculator::new();
        for (model, model_pricing) in models {
            pricing.add_model(model, model_pricing);
        }

        Self {
            client: Client::new(),
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            api_key_header: api_key_header.unwrap_or_else(|| "Authorization".to_string()),
            api_key_format: api_key_format.unwrap_or_else(|| "Bearer {key}".to_string()),
            pricing,
        }
    }

    fn get_api_url(&self) -> String {
        // Handle cases where base_url already includes the path
        if self.base_url.contains("/v1/chat/completions") {
            self.base_url.clone()
        } else if self.base_url.ends_with("/v1") {
            format!("{}/chat/completions", self.base_url)
        } else {
            format!("{}/v1/chat/completions", self.base_url)
        }
    }

    fn format_api_key(&self) -> Option<String> {
        self.api_key
            .as_ref()
            .map(|key| self.api_key_format.replace("{key}", key))
    }
}

#[async_trait]
impl LLMProvider for OpenAICompatibleProvider {
    async fn complete(&self, req: CompletionRequest) -> ProviderResult<CompletionResponse> {
        let openai_req = OpenAIRequest {
            model: req.model.clone(),
            messages: req.messages,
            temperature: req.temperature,
            max_tokens: req.max_tokens,
            stream: None,
        };

        let mut request = self
            .client
            .post(self.get_api_url())
            .header("Content-Type", "application/json")
            .json(&openai_req);

        // Add authorization header if API key is provided
        if let Some(auth_value) = self.format_api_key() {
            request = request.header(&self.api_key_header, auth_value);
        }

        let response = request
            .send()
            .await
            .map_err(|e| ProviderError::RequestFailed(e.to_string()))?;

        if !response.status().is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(ProviderError::ApiError(error_text));
        }

        let openai_response: OpenAIResponse = response
            .json()
            .await
            .map_err(|e| ProviderError::InvalidResponse(e.to_string()))?;

        Ok(CompletionResponse {
            content: openai_response
                .choices
                .first()
                .map(|c| c.message.content.clone())
                .unwrap_or_default(),
            prompt_tokens: openai_response.usage.prompt_tokens,
            completion_tokens: openai_response.usage.completion_tokens,
            model: openai_response.model,
            provider: "openai-compatible".to_string(),
            tool_calls: None,
            finish_reason: "stop".to_string(),
        })
    }

    async fn stream(
        &self,
        req: CompletionRequest,
    ) -> ProviderResult<Pin<Box<dyn Stream<Item = ProviderResult<StreamChunk>> + Send>>> {
        let openai_req = OpenAIRequest {
            model: req.model.clone(),
            messages: req.messages,
            temperature: req.temperature,
            max_tokens: req.max_tokens,
            stream: Some(true),
        };

        let mut request = self
            .client
            .post(self.get_api_url())
            .header("Content-Type", "application/json")
            .json(&openai_req);

        // Add authorization header if API key is provided
        if let Some(auth_value) = self.format_api_key() {
            request = request.header(&self.api_key_header, auth_value);
        }

        let response = request
            .send()
            .await
            .map_err(|e| ProviderError::RequestFailed(e.to_string()))?;

        if !response.status().is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(ProviderError::ApiError(error_text));
        }

        let stream = response
            .bytes_stream()
            .map_err(|e| ProviderError::StreamError(format!("Stream error: {}", e)));

        Ok(Box::pin(OpenAICompatibleStreamAdapter::new(stream)))
    }

    fn get_pricing(&self, model: &str) -> Option<ModelPricing> {
        self.pricing.get_pricing(model).cloned()
    }

    fn name(&self) -> &str {
        "openai-compatible"
    }
}

#[pin_project]
struct OpenAICompatibleStreamAdapter<S> {
    #[pin]
    stream: S,
    buffer: String,
    total_prompt_tokens: Option<u64>,
    total_completion_tokens: Option<u64>,
}

impl<S> OpenAICompatibleStreamAdapter<S> {
    fn new(stream: S) -> Self {
        Self {
            stream,
            buffer: String::new(),
            total_prompt_tokens: None,
            total_completion_tokens: None,
        }
    }
}

impl<S, E> Stream for OpenAICompatibleStreamAdapter<S>
where
    S: Stream<Item = Result<Bytes, E>> + Unpin,
    E: std::fmt::Display,
{
    type Item = ProviderResult<StreamChunk>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.as_mut().project();

        match this.stream.as_mut().poll_next(cx) {
            Poll::Ready(Some(Ok(bytes))) => {
                let text = String::from_utf8_lossy(&bytes);
                this.buffer.push_str(&text);

                // Process complete lines
                while let Some(line_end) = this.buffer.find('\n') {
                    let line = this.buffer[..line_end].trim().to_string();
                    this.buffer.drain(..=line_end);

                    if let Some(data) = line.strip_prefix("data: ") {
                        if data == "[DONE]" {
                            return Poll::Ready(Some(Ok(StreamChunk {
                                content: String::new(),
                                is_final: true,
                                prompt_tokens: *this.total_prompt_tokens,
                                completion_tokens: *this.total_completion_tokens,
                            })));
                        }

                        match serde_json::from_str::<OpenAIStreamResponse>(data) {
                            Ok(response) => {
                                // Use trait methods to extract tokens
                                if let Some(pt) = response.extract_prompt_tokens() {
                                    *this.total_prompt_tokens = Some(pt);
                                }

                                if let Some(ct) = response.extract_completion_tokens() {
                                    *this.total_completion_tokens = Some(ct);
                                }

                                let content = response
                                    .choices
                                    .first()
                                    .and_then(|c| c.delta.content.clone())
                                    .unwrap_or_default();

                                let is_final = response
                                    .choices
                                    .first()
                                    .and_then(|c| c.finish_reason.as_ref())
                                    .is_some();

                                return Poll::Ready(Some(Ok(StreamChunk {
                                    content,
                                    is_final,
                                    prompt_tokens: None,
                                    completion_tokens: None,
                                })));
                            }
                            Err(e) => {
                                return Poll::Ready(Some(Err(ProviderError::InvalidResponse(
                                    format!("Failed to parse SSE data: {}", e),
                                ))));
                            }
                        }
                    }
                }

                // Continue polling
                cx.waker().wake_by_ref();
                Poll::Pending
            }
            Poll::Ready(Some(Err(e))) => {
                Poll::Ready(Some(Err(ProviderError::StreamError(e.to_string()))))
            }
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}
