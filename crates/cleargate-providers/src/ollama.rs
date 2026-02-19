use crate::pricing::{ModelPricing, PricingCalculator};
use crate::traits::*;
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::Stream;
use futures::TryStreamExt;
use pin_project::pin_project;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::pin::Pin;
use std::task::{Context, Poll};

const DEFAULT_OLLAMA_BASE_URL: &str = "http://localhost:11434";

#[derive(Debug, Clone)]
pub struct OllamaProvider {
    client: Client,
    base_url: String,
    api_key: Option<String>,
    pricing: PricingCalculator,
}

// Ollama /api/chat request
#[derive(Debug, Serialize)]
struct OllamaChatRequest {
    model: String,
    messages: Vec<OllamaChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    options: Option<OllamaOptions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize)]
struct OllamaChatMessage {
    role: String,
    #[serde(default)]
    content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OllamaToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct OllamaToolCall {
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    call_type: Option<String>,
    function: OllamaToolCallFunction,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct OllamaToolCallFunction {
    name: String,
    arguments: Value,
}

#[derive(Debug, Serialize)]
struct OllamaOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    num_predict: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
}

// Ollama /api/chat response (non-streaming)
#[derive(Debug, Deserialize)]
struct OllamaChatResponse {
    model: String,
    message: OllamaChatMessage,
    #[serde(default)]
    #[allow(dead_code)]
    done: bool,
    #[serde(default)]
    prompt_eval_count: Option<u64>,
    #[serde(default)]
    eval_count: Option<u64>,
}

// Ollama /api/chat streaming response
#[derive(Debug, Deserialize)]
struct OllamaChatStreamResponse {
    #[allow(dead_code)]
    model: String,
    #[serde(default)]
    message: Option<OllamaChatMessage>,
    #[serde(default)]
    done: bool,
    #[serde(default)]
    prompt_eval_count: Option<u64>,
    #[serde(default)]
    eval_count: Option<u64>,
}

impl StreamResponseTokens for OllamaChatStreamResponse {
    fn extract_prompt_tokens(&self) -> Option<u64> {
        let tokens = self.prompt_eval_count;

        if let Some(count) = tokens {
            tracing::debug!(
                provider = "ollama",
                prompt_tokens = count,
                "Extracted prompt tokens from stream response"
            );
        }

        tokens
    }

    fn extract_completion_tokens(&self) -> Option<u64> {
        let tokens = self.eval_count;

        if let Some(count) = tokens {
            tracing::debug!(
                provider = "ollama",
                completion_tokens = count,
                "Extracted completion tokens from stream response"
            );
        }

        tokens
    }
}

impl OllamaProvider {
    /// Create a new OllamaProvider with a custom base URL
    ///
    /// # Arguments
    /// * `base_url` - The base URL for the Ollama server (e.g., `http://localhost:11434` or `https://ollama.example.com`)
    /// * `api_key` - Optional API key for hosted Ollama instances
    /// * `models` - List of models with their pricing information
    pub fn new(
        base_url: Option<String>,
        api_key: Option<String>,
        models: Vec<(String, ModelPricing)>,
    ) -> Self {
        let mut pricing = PricingCalculator::new();
        for (model, model_pricing) in models {
            pricing.add_model(model, model_pricing);
        }

        Self {
            client: Client::new(),
            base_url: base_url.unwrap_or_else(|| DEFAULT_OLLAMA_BASE_URL.to_string()),
            api_key,
            pricing,
        }
    }

    /// Create a new OllamaProvider for local usage (default localhost:11434)
    pub fn new_local(models: Vec<(String, ModelPricing)>) -> Self {
        Self::new(None, None, models)
    }

    /// Create a new OllamaProvider for hosted usage
    pub fn new_hosted(
        base_url: String,
        api_key: Option<String>,
        models: Vec<(String, ModelPricing)>,
    ) -> Self {
        Self::new(Some(base_url), api_key, models)
    }

    fn get_api_url(&self) -> String {
        format!("{}/api/chat", self.base_url)
    }

    /// Convert provider Messages to Ollama chat messages
    fn convert_messages(messages: &[Message]) -> Vec<OllamaChatMessage> {
        messages
            .iter()
            .map(|m| {
                let tool_calls = m.tool_calls.as_ref().map(|tcs| {
                    tcs.iter()
                        .map(|tc| OllamaToolCall {
                            id: Some(tc.id.clone()),
                            call_type: Some(tc.call_type.clone()),
                            function: OllamaToolCallFunction {
                                name: tc.function.name.clone(),
                                // Parse arguments JSON string back to Value for Ollama
                                arguments: serde_json::from_str(&tc.function.arguments)
                                    .unwrap_or(Value::Object(serde_json::Map::new())),
                            },
                        })
                        .collect()
                });

                OllamaChatMessage {
                    role: m.role.clone(),
                    content: m.content.clone(),
                    tool_calls,
                    tool_call_id: m.tool_call_id.clone(),
                }
            })
            .collect()
    }

    /// Convert Ollama tool calls to provider ToolCall format
    fn convert_tool_calls(ollama_calls: &[OllamaToolCall]) -> Vec<ToolCall> {
        ollama_calls
            .iter()
            .map(|tc| ToolCall {
                id: tc
                    .id
                    .clone()
                    .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
                call_type: tc
                    .call_type
                    .clone()
                    .unwrap_or_else(|| "function".to_string()),
                function: ToolCallFunction {
                    name: tc.function.name.clone(),
                    // Serialize Value back to JSON string (OpenAI format)
                    arguments: serde_json::to_string(&tc.function.arguments)
                        .unwrap_or_else(|_| "{}".to_string()),
                },
            })
            .collect()
    }
}

#[async_trait]
impl LLMProvider for OllamaProvider {
    async fn complete(&self, req: CompletionRequest) -> ProviderResult<CompletionResponse> {
        let messages = Self::convert_messages(&req.messages);

        let options = if req.max_tokens.is_some() || req.temperature.is_some() {
            Some(OllamaOptions {
                num_predict: req.max_tokens,
                temperature: req.temperature,
            })
        } else {
            None
        };

        let ollama_req = OllamaChatRequest {
            model: req.model.clone(),
            messages,
            tools: req.tools,
            options,
            stream: Some(false),
        };

        let mut request = self
            .client
            .post(self.get_api_url())
            .header("Content-Type", "application/json")
            .json(&ollama_req);

        if let Some(api_key) = &self.api_key {
            request = request.header("Authorization", format!("Bearer {}", api_key));
        }

        let response = request
            .send()
            .await
            .map_err(|e| ProviderError::RequestFailed(e.to_string()))?;

        if !response.status().is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(ProviderError::ApiError(error_text));
        }

        let ollama_response: OllamaChatResponse = response
            .json()
            .await
            .map_err(|e| ProviderError::InvalidResponse(e.to_string()))?;

        let tool_calls = ollama_response
            .message
            .tool_calls
            .as_ref()
            .filter(|tcs| !tcs.is_empty())
            .map(|tcs| Self::convert_tool_calls(tcs));

        let finish_reason = if tool_calls.is_some() {
            "tool_calls".to_string()
        } else {
            "stop".to_string()
        };

        Ok(CompletionResponse {
            content: ollama_response.message.content,
            prompt_tokens: ollama_response.prompt_eval_count.unwrap_or(0),
            completion_tokens: ollama_response.eval_count.unwrap_or(0),
            model: ollama_response.model,
            provider: "ollama".to_string(),
            tool_calls,
            finish_reason,
        })
    }

    async fn stream(
        &self,
        req: CompletionRequest,
    ) -> ProviderResult<Pin<Box<dyn Stream<Item = ProviderResult<StreamChunk>> + Send>>> {
        let messages = Self::convert_messages(&req.messages);

        let options = if req.max_tokens.is_some() || req.temperature.is_some() {
            Some(OllamaOptions {
                num_predict: req.max_tokens,
                temperature: req.temperature,
            })
        } else {
            None
        };

        let ollama_req = OllamaChatRequest {
            model: req.model.clone(),
            messages,
            tools: req.tools,
            options,
            stream: Some(true),
        };

        let mut request = self
            .client
            .post(self.get_api_url())
            .header("Content-Type", "application/json")
            .json(&ollama_req);

        if let Some(api_key) = &self.api_key {
            request = request.header("Authorization", format!("Bearer {}", api_key));
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

        Ok(Box::pin(OllamaChatStreamAdapter::new(stream)))
    }

    fn get_pricing(&self, model: &str) -> Option<ModelPricing> {
        self.pricing.get_pricing(model).cloned()
    }

    fn name(&self) -> &str {
        "ollama"
    }
}

#[pin_project]
struct OllamaChatStreamAdapter<S> {
    #[pin]
    stream: S,
    buffer: String,
    total_prompt_tokens: Option<u64>,
    total_completion_tokens: Option<u64>,
}

impl<S> OllamaChatStreamAdapter<S> {
    fn new(stream: S) -> Self {
        Self {
            stream,
            buffer: String::new(),
            total_prompt_tokens: None,
            total_completion_tokens: None,
        }
    }
}

impl<S, E> Stream for OllamaChatStreamAdapter<S>
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

                // Process complete lines (Ollama sends newline-delimited JSON)
                while let Some(line_end) = this.buffer.find('\n') {
                    let line = this.buffer[..line_end].trim().to_string();
                    this.buffer.drain(..=line_end);

                    if line.is_empty() {
                        continue;
                    }

                    tracing::debug!("Ollama stream raw JSON: {}", line);

                    match serde_json::from_str::<OllamaChatStreamResponse>(&line) {
                        Ok(response) => {
                            if let Some(pt) = response.extract_prompt_tokens() {
                                *this.total_prompt_tokens = Some(pt);
                            }

                            if let Some(ct) = response.extract_completion_tokens() {
                                *this.total_completion_tokens = Some(ct);
                            }

                            let content = response
                                .message
                                .as_ref()
                                .map(|m| m.content.clone())
                                .unwrap_or_default();
                            let is_final = response.done;

                            let (prompt_tokens, completion_tokens) = if is_final {
                                (*this.total_prompt_tokens, *this.total_completion_tokens)
                            } else {
                                (None, None)
                            };

                            return Poll::Ready(Some(Ok(StreamChunk {
                                content,
                                is_final,
                                prompt_tokens,
                                completion_tokens,
                            })));
                        }
                        Err(e) => {
                            return Poll::Ready(Some(Err(ProviderError::InvalidResponse(
                                format!("Failed to parse JSON: {}", e),
                            ))));
                        }
                    }
                }

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
