use async_trait::async_trait;
use serde::Serialize;
use serde_json::Value;
use sinew_core::{
    AppError, ChatMessage, ModelCapabilities, ModelRef, Part, Provider, ProviderRequest,
    ProviderStream, Result, Role, TokenEstimate, ToolDescriptor,
};

use crate::{auth::Credential, model_info, stream::map_stream, wire};

// DeepSeek's OpenAI-compatible endpoint. `/chat/completions` (and `/models`
// for key validation) hang off this base.
const BASE_URL: &str = "https://api.deepseek.com/v1";
const USER_AGENT: &str = "Sinew/0.1";
const RECONNECT_MESSAGE: &str =
    "DeepSeek API key was rejected. Re-enter your key in Settings > Providers.";

#[derive(Clone)]
pub struct DeepSeekConfig {
    pub credential: Credential,
    pub base_url: String,
}

impl DeepSeekConfig {
    pub fn new(credential: Credential) -> Self {
        Self {
            credential,
            base_url: BASE_URL.into(),
        }
    }

    pub fn from_default_sources() -> Result<Self> {
        if let Some(credential) = Credential::load_default()? {
            return Ok(Self::new(credential));
        }

        Err(AppError::Auth(
            "no DeepSeek API key found. Add one in Settings > Providers.".into(),
        ))
    }
}

pub struct DeepSeekProvider {
    config: DeepSeekConfig,
    http: reqwest::Client,
}

impl DeepSeekProvider {
    pub fn new(config: DeepSeekConfig) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .build()
            .map_err(|err| AppError::Network(err.to_string()))?;
        Ok(Self { config, http })
    }

    pub fn from_default_sources() -> Result<Self> {
        Self::new(DeepSeekConfig::from_default_sources()?)
    }

    async fn send_json<T: Serialize + ?Sized>(
        &self,
        route: &str,
        body: &T,
    ) -> Result<reqwest::Response> {
        self.http
            .post(format!(
                "{}{}",
                self.config.base_url.trim_end_matches('/'),
                route
            ))
            .bearer_auth(self.config.credential.api_key())
            .header("content-type", "application/json")
            .header("accept", "application/json")
            .json(body)
            .send()
            .await
            .map_err(|err| AppError::Network(err.to_string()))
    }
}

#[async_trait]
impl Provider for DeepSeekProvider {
    fn name(&self) -> &str {
        model_info::PROVIDER_ID
    }

    fn capabilities(&self, model: &ModelRef) -> Option<ModelCapabilities> {
        if model.provider != model_info::PROVIDER_ID {
            return None;
        }
        Some(model_info::capabilities(model))
    }

    async fn estimate_tokens(&self, request: ProviderRequest) -> Result<TokenEstimate> {
        if request.model.provider != model_info::PROVIDER_ID {
            return Err(AppError::Unsupported(format!(
                "deepseek provider cannot count model provider {}",
                request.model.provider
            )));
        }
        Ok(TokenEstimate {
            input_tokens: rough_token_estimate(&request),
            exact: false,
        })
    }

    async fn stream(&self, request: ProviderRequest) -> Result<ProviderStream> {
        if request.model.provider != model_info::PROVIDER_ID {
            return Err(AppError::Unsupported(format!(
                "deepseek provider cannot run model provider {}",
                request.model.provider
            )));
        }

        let caps = model_info::capabilities(&request.model);
        // DeepSeek V4 reasons on its own and streams `reasoning_content`; we send
        // a plain OpenAI-compatible request and let the stream surface thinking.
        let body = wire::ChatCompletionsRequest {
            model: &request.model.name,
            messages: to_wire_messages(&request, caps.supports_images)?,
            tools: request.tools.iter().map(to_wire_tool).collect(),
            max_tokens: Some(request.output_token_budget(&caps)),
            temperature: request.temperature,
            // DeepSeek does context caching automatically and has no
            // `prompt_cache_key` param; sending an unknown field risks a 400,
            // so omit it (unlike Kimi/OpenAI which accept it).
            prompt_cache_key: None,
            reasoning_effort: None,
            thinking: None,
            stream: true,
            stream_options: Some(wire::StreamOptions {
                include_usage: true,
            }),
        };

        let response = self.send_json("/chat/completions", &body).await?;
        if !response.status().is_success() {
            return Err(read_http_error(response).await);
        }

        Ok(map_stream(response.bytes_stream(), request.model.name))
    }
}

/// Validate a DeepSeek API key by listing models (OpenAI-compatible `/models`).
pub async fn validate_api_key(api_key: &str) -> Result<()> {
    let api_key = api_key.trim();
    if api_key.is_empty() {
        return Err(AppError::Auth("DeepSeek API key cannot be empty".into()));
    }
    let http = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .build()
        .map_err(|err| AppError::Network(err.to_string()))?;
    let response = http
        .get(format!("{}/models", BASE_URL.trim_end_matches('/')))
        .bearer_auth(api_key)
        .header("accept", "application/json")
        .send()
        .await
        .map_err(|err| AppError::Network(format!("DeepSeek key validation failed: {err}")))?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(read_http_error(response).await)
    }
}

fn to_wire_tool(tool: &ToolDescriptor) -> wire::WireTool<'_> {
    wire::WireTool {
        kind: "function",
        function: wire::WireToolFunction {
            name: &tool.name,
            description: &tool.description,
            parameters: &tool.input_schema,
        },
    }
}

fn to_wire_messages<'a>(
    request: &'a ProviderRequest,
    supports_images: bool,
) -> Result<Vec<wire::WireMessage<'a>>> {
    let mut messages = Vec::new();
    if let Some(system) = request
        .system_prompt
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        messages.push(wire::WireMessage::System {
            role: "system",
            content: system,
        });
    }

    for message in &request.transcript {
        match message.role {
            Role::User => push_user_messages(message, &mut messages, supports_images),
            Role::Assistant => push_assistant_message(message, &mut messages),
        }
    }

    Ok(messages)
}

fn push_user_messages<'a>(
    message: &'a ChatMessage,
    messages: &mut Vec<wire::WireMessage<'a>>,
    supports_images: bool,
) {
    let mut builder = ContentBuilder::new(supports_images);
    for part in &message.parts {
        if part_is_ui_only(part) {
            continue;
        }
        match part {
            Part::Text { text, .. } => builder.push_text(text),
            Part::Image {
                media_type, data, ..
            } => builder.push_image(media_type, data),
            Part::ToolResult {
                tool_call_id,
                content,
                images,
                ..
            } => {
                flush_user_builder(&mut builder, messages);
                let mut result = ContentBuilder::new(supports_images);
                result.push_text(content);
                for image in images {
                    if !image.data.trim().is_empty() {
                        result.push_image(&image.media_type, &image.data);
                    }
                }
                let content = result
                    .finish_allow_empty()
                    .unwrap_or_else(|| wire::WireContent::Text(String::new()));
                messages.push(wire::WireMessage::Tool {
                    role: "tool",
                    content,
                    tool_call_id,
                });
            }
            Part::Thinking { .. } | Part::ToolCall { .. } => {}
        }
    }
    flush_user_builder(&mut builder, messages);
}

fn flush_user_builder<'a>(builder: &mut ContentBuilder, messages: &mut Vec<wire::WireMessage<'a>>) {
    if let Some(content) = builder.finish() {
        messages.push(wire::WireMessage::User {
            role: "user",
            content,
        });
    }
}

fn push_assistant_message<'a>(message: &'a ChatMessage, messages: &mut Vec<wire::WireMessage<'a>>) {
    let mut text = String::new();
    let mut reasoning = String::new();
    let mut tool_calls = Vec::new();

    for part in &message.parts {
        if part_is_ui_only(part) {
            continue;
        }
        match part {
            Part::Text { text: value, .. } => text.push_str(value),
            Part::Thinking { text: value, .. } => reasoning.push_str(value),
            Part::ToolCall {
                id, name, input, ..
            } => tool_calls.push(wire::WireToolCall {
                id,
                kind: "function",
                function: wire::WireToolCallFunction {
                    name,
                    arguments: input.to_string(),
                },
            }),
            Part::Image { .. } | Part::ToolResult { .. } => {}
        }
    }

    if text.is_empty() && reasoning.is_empty() && tool_calls.is_empty() {
        return;
    }

    let content = (!text.is_empty()).then_some(wire::WireContent::Text(text));
    let reasoning_content = (!reasoning.is_empty()).then_some(reasoning);
    messages.push(wire::WireMessage::Assistant {
        role: "assistant",
        content,
        reasoning_content,
        tool_calls,
    });
}

#[derive(Default)]
struct ContentBuilder {
    text: String,
    blocks: Vec<wire::WireContentBlock>,
    has_media: bool,
    supports_images: bool,
}

impl ContentBuilder {
    fn new(supports_images: bool) -> Self {
        Self {
            supports_images,
            ..Self::default()
        }
    }

    fn push_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        if self.has_media {
            self.blocks.push(wire::WireContentBlock::Text {
                text: text.to_string(),
            });
        } else {
            self.text.push_str(text);
        }
    }

    fn push_image(&mut self, media_type: &str, data: &str) {
        if data.trim().is_empty() {
            return;
        }
        // DeepSeek V4 is text-only; downgrade images to a text placeholder so
        // conversations that accumulated screenshots still stream instead of
        // getting rejected with "unknown variant 'image_url'".
        if !self.supports_images {
            self.push_text(&format!("\n[Image omitted: {media_type}]\n"));
            return;
        }
        if !self.has_media {
            self.has_media = true;
            if !self.text.is_empty() {
                self.blocks.push(wire::WireContentBlock::Text {
                    text: std::mem::take(&mut self.text),
                });
            }
        }
        self.blocks.push(wire::WireContentBlock::ImageUrl {
            image_url: wire::WireImageUrl {
                url: format!("data:{media_type};base64,{data}"),
            },
        });
    }

    fn finish(&mut self) -> Option<wire::WireContent> {
        self.finish_inner(false)
    }

    fn finish_allow_empty(&mut self) -> Option<wire::WireContent> {
        self.finish_inner(true)
    }

    fn finish_inner(&mut self, allow_empty_text: bool) -> Option<wire::WireContent> {
        if self.has_media {
            if self.blocks.is_empty() {
                return None;
            }
            self.has_media = false;
            return Some(wire::WireContent::Blocks(std::mem::take(&mut self.blocks)));
        }
        if self.text.is_empty() && !allow_empty_text {
            return None;
        }
        Some(wire::WireContent::Text(std::mem::take(&mut self.text)))
    }
}

fn part_is_ui_only(part: &Part) -> bool {
    part_meta(part)
        .and_then(|meta| meta.get("ui_only"))
        .and_then(|value| value.as_bool())
        == Some(true)
}

fn part_meta(part: &Part) -> Option<&Value> {
    match part {
        Part::Text { meta, .. }
        | Part::Image { meta, .. }
        | Part::Thinking { meta, .. }
        | Part::ToolCall { meta, .. }
        | Part::ToolResult { meta, .. } => meta.as_ref(),
    }
}

fn rough_token_estimate(request: &ProviderRequest) -> u32 {
    let mut chars: usize = 0;
    if let Some(system) = &request.system_prompt {
        chars += system.chars().count();
    }
    for message in &request.transcript {
        for part in &message.parts {
            if part_is_ui_only(part) {
                continue;
            }
            match part {
                Part::Text { text, .. } | Part::Thinking { text, .. } => {
                    chars += text.chars().count()
                }
                Part::Image { .. } => chars += 4_000,
                Part::ToolCall { name, input, .. } => {
                    chars += name.chars().count();
                    chars += input.to_string().chars().count();
                }
                Part::ToolResult {
                    content, images, ..
                } => {
                    chars += content.chars().count();
                    chars += images.len() * 4_000;
                }
            }
        }
    }
    for tool in &request.tools {
        chars += tool.name.chars().count();
        chars += tool.description.chars().count();
        chars += tool.input_schema.to_string().chars().count();
    }
    ((chars / 4).max(1)).min(u32::MAX as usize) as u32
}

async fn read_http_error(response: reqwest::Response) -> AppError {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    let parsed: std::result::Result<wire::ApiErrorEnvelope, _> = serde_json::from_str(&body);
    let message = parsed
        .ok()
        .map(|payload| {
            let code = payload.error.code.unwrap_or_default();
            if code.is_empty() {
                format!("{}: {}", payload.error.kind, payload.error.message)
            } else {
                format!("{} ({code}): {}", payload.error.kind, payload.error.message)
            }
        })
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(body);

    if status == reqwest::StatusCode::UNAUTHORIZED {
        AppError::Auth(RECONNECT_MESSAGE.into())
    } else if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        AppError::RateLimit(message)
    } else if status.is_client_error() {
        if message.contains("context") || message.contains("too long") {
            AppError::ContextLength(message)
        } else {
            AppError::InvalidRequest(message)
        }
    } else {
        AppError::Provider(format!("HTTP {status}: {message}"))
    }
}
