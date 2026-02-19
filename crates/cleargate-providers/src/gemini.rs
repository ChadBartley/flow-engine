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

const DEFAULT_GEMINI_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta";

#[derive(Debug, Clone)]
pub struct GeminiProvider {
    client: Client,
    base_url: String,
    api_key: String,
    pricing: PricingCalculator,
}

// Gemini API request format
#[derive(Debug, Serialize)]
struct GeminiRequest {
    contents: Vec<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generation_config: Option<GeminiGenerationConfig>,
}

#[derive(Debug, Serialize, Deserialize)]
struct GeminiContent {
    role: String,
    /// Parts may be missing if the response was truncated (e.g., MAX_TOKENS)
    #[serde(default)]
    parts: Vec<GeminiPart>,
}

#[derive(Debug, Serialize, Deserialize)]
struct GeminiPart {
    text: String,
}

#[derive(Debug, Serialize)]
struct GeminiGenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u32>,
}

// Gemini API response format
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiResponse {
    candidates: Vec<GeminiCandidate>,
    #[serde(default)]
    usage_metadata: Option<GeminiUsageMetadata>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiCandidate {
    content: GeminiContent,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiUsageMetadata {
    #[serde(default)]
    prompt_token_count: Option<u64>,
    #[serde(default)]
    candidates_token_count: Option<u64>,
    #[serde(default)]
    #[allow(dead_code)]
    total_token_count: Option<u64>,
}

impl StreamResponseTokens for GeminiUsageMetadata {
    fn extract_prompt_tokens(&self) -> Option<u64> {
        let tokens = self.prompt_token_count;

        if let Some(count) = tokens {
            tracing::debug!(
                provider = "gemini",
                prompt_tokens = count,
                "Extracted prompt tokens from usage metadata"
            );
        }

        tokens
    }

    fn extract_completion_tokens(&self) -> Option<u64> {
        let tokens = self.candidates_token_count;

        if let Some(count) = tokens {
            tracing::debug!(
                provider = "gemini",
                completion_tokens = count,
                "Extracted completion tokens from usage metadata"
            );
        }

        tokens
    }
}

// Gemini streaming response format
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiStreamResponse {
    candidates: Vec<GeminiCandidate>,
    #[serde(default)]
    usage_metadata: Option<GeminiUsageMetadata>,
}

impl StreamResponseTokens for GeminiStreamResponse {
    fn extract_prompt_tokens(&self) -> Option<u64> {
        self.usage_metadata
            .as_ref()
            .and_then(|um| um.extract_prompt_tokens())
    }

    fn extract_completion_tokens(&self) -> Option<u64> {
        self.usage_metadata
            .as_ref()
            .and_then(|um| um.extract_completion_tokens())
    }
}

impl GeminiProvider {
    /// Create a new GeminiProvider
    ///
    /// # Arguments
    /// * `base_url` - Base URL for Gemini API (defaults to v1beta endpoint)
    /// * `api_key` - Google API key
    /// * `models` - List of models with their pricing information
    pub fn new(
        base_url: Option<String>,
        api_key: String,
        models: Vec<(String, ModelPricing)>,
    ) -> Self {
        let mut pricing = PricingCalculator::new();
        for (model, model_pricing) in models {
            pricing.add_model(model, model_pricing);
        }

        Self {
            client: Client::new(),
            base_url: base_url.unwrap_or_else(|| DEFAULT_GEMINI_BASE_URL.to_string()),
            api_key,
            pricing,
        }
    }

    fn get_api_url(&self, model: &str) -> String {
        format!("{}/models/{}:generateContent", self.base_url, model)
    }

    fn get_stream_api_url(&self, model: &str) -> String {
        format!(
            "{}/models/{}:streamGenerateContent?alt=sse",
            self.base_url, model
        )
    }

    /// Convert messages to Gemini's contents format
    ///
    /// Gemini uses a different format:
    /// - `user` role -> `user`
    /// - `assistant` role -> `model`
    /// - `system` role -> prepended to first user message as text
    fn messages_to_contents(messages: &[Message]) -> Vec<GeminiContent> {
        let mut contents = Vec::new();
        let mut system_message: Option<String> = None;

        for msg in messages {
            match msg.role.as_str() {
                "system" => {
                    // System messages are prepended to the first user message
                    if system_message.is_none() {
                        system_message = Some(msg.content.clone());
                    } else {
                        // If multiple system messages, append them
                        if let Some(ref mut sys) = system_message {
                            sys.push('\n');
                            sys.push_str(&msg.content);
                        }
                    }
                }
                "user" => {
                    let mut text = msg.content.clone();
                    // Prepend system message to first user message if present
                    if let Some(sys) = system_message.take() {
                        text = format!("{}\n\n{}", sys, text);
                    }
                    contents.push(GeminiContent {
                        role: "user".to_string(),
                        parts: vec![GeminiPart { text }],
                    });
                }
                "assistant" => {
                    contents.push(GeminiContent {
                        role: "model".to_string(),
                        parts: vec![GeminiPart {
                            text: msg.content.clone(),
                        }],
                    });
                }
                _ => {
                    // Unknown role, treat as user message
                    contents.push(GeminiContent {
                        role: "user".to_string(),
                        parts: vec![GeminiPart {
                            text: msg.content.clone(),
                        }],
                    });
                }
            }
        }

        contents
    }
}

#[async_trait]
impl LLMProvider for GeminiProvider {
    async fn complete(&self, req: CompletionRequest) -> ProviderResult<CompletionResponse> {
        let contents = Self::messages_to_contents(&req.messages);

        let generation_config = if req.temperature.is_some() || req.max_tokens.is_some() {
            Some(GeminiGenerationConfig {
                temperature: req.temperature,
                max_output_tokens: req.max_tokens,
            })
        } else {
            None
        };

        let gemini_req = GeminiRequest {
            contents,
            generation_config,
        };

        let response = self
            .client
            .post(self.get_api_url(&req.model))
            .header("x-goog-api-key", &self.api_key)
            .header("Content-Type", "application/json")
            .json(&gemini_req)
            .send()
            .await
            .map_err(|e| ProviderError::RequestFailed(e.to_string()))?;

        let status = response.status();
        let response_text = response.text().await.map_err(|e| {
            ProviderError::InvalidResponse(format!("Failed to read response: {}", e))
        })?;

        if !status.is_success() {
            return Err(ProviderError::ApiError(format!(
                "Gemini API error ({}): {}",
                status, response_text
            )));
        }

        let gemini_response: GeminiResponse =
            serde_json::from_str(&response_text).map_err(|e| {
                ProviderError::InvalidResponse(format!(
                    "Failed to parse Gemini response: {}. Raw response: {}",
                    e,
                    if response_text.len() > 500 {
                        format!("{}...", &response_text[..500])
                    } else {
                        response_text
                    }
                ))
            })?;

        let content = gemini_response
            .candidates
            .first()
            .and_then(|c| c.content.parts.first())
            .map(|p| p.text.clone())
            .unwrap_or_default();

        let prompt_tokens = gemini_response
            .usage_metadata
            .as_ref()
            .and_then(|um| um.prompt_token_count)
            .unwrap_or(0);

        let completion_tokens = gemini_response
            .usage_metadata
            .as_ref()
            .and_then(|um| um.candidates_token_count)
            .unwrap_or(0);

        Ok(CompletionResponse {
            content,
            prompt_tokens,
            completion_tokens,
            model: req.model,
            provider: "gemini".to_string(),
            tool_calls: None,
            finish_reason: "stop".to_string(),
        })
    }

    async fn stream(
        &self,
        req: CompletionRequest,
    ) -> ProviderResult<Pin<Box<dyn Stream<Item = ProviderResult<StreamChunk>> + Send>>> {
        let contents = Self::messages_to_contents(&req.messages);

        let generation_config = if req.temperature.is_some() || req.max_tokens.is_some() {
            Some(GeminiGenerationConfig {
                temperature: req.temperature,
                max_output_tokens: req.max_tokens,
            })
        } else {
            None
        };

        let gemini_req = GeminiRequest {
            contents,
            generation_config,
        };

        let response = self
            .client
            .post(self.get_stream_api_url(&req.model))
            .header("x-goog-api-key", &self.api_key)
            .header("Content-Type", "application/json")
            .json(&gemini_req)
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

        Ok(Box::pin(GeminiStreamAdapter::new(stream)))
    }

    fn get_pricing(&self, model: &str) -> Option<ModelPricing> {
        self.pricing.get_pricing(model).cloned()
    }

    fn name(&self) -> &str {
        "gemini"
    }
}

#[pin_project]
struct GeminiStreamAdapter<S> {
    #[pin]
    stream: S,
    buffer: String,
    total_prompt_tokens: Option<u64>,
    total_completion_tokens: Option<u64>,
}

impl<S> GeminiStreamAdapter<S> {
    fn new(stream: S) -> Self {
        Self {
            stream,
            buffer: String::new(),
            total_prompt_tokens: None,
            total_completion_tokens: None,
        }
    }
}

impl<S, E> Stream for GeminiStreamAdapter<S>
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

                // Process complete lines (Gemini uses SSE format)
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

                        match serde_json::from_str::<GeminiStreamResponse>(data) {
                            Ok(response) => {
                                // Use trait methods to extract tokens
                                if let Some(pt) = response.extract_prompt_tokens() {
                                    *this.total_prompt_tokens = Some(pt);
                                }

                                if let Some(ct) = response.extract_completion_tokens() {
                                    *this.total_completion_tokens = Some(ct);
                                }

                                let content = response
                                    .candidates
                                    .first()
                                    .and_then(|c| c.content.parts.first())
                                    .map(|p| p.text.clone())
                                    .unwrap_or_default();

                                let is_final = response
                                    .candidates
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
