use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Serialize)]
pub struct ChatCompletionsRequest<'a> {
    pub model: &'a str,
    pub messages: Vec<WireMessage<'a>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<WireTool<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_cache_key: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingConfig>,
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_options: Option<StreamOptions>,
}

#[derive(Debug, Serialize)]
pub struct StreamOptions {
    pub include_usage: bool,
}

#[derive(Debug, Serialize)]
pub struct ThinkingConfig {
    #[serde(rename = "type")]
    pub kind: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum WireMessage<'a> {
    System {
        role: &'static str,
        content: &'a str,
    },
    User {
        role: &'static str,
        content: WireContent,
    },
    Assistant {
        role: &'static str,
        #[serde(skip_serializing_if = "Option::is_none")]
        content: Option<WireContent>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reasoning_content: Option<String>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        tool_calls: Vec<WireToolCall<'a>>,
    },
    Tool {
        role: &'static str,
        content: WireContent,
        tool_call_id: &'a str,
    },
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum WireContent {
    Text(String),
    Blocks(Vec<WireContentBlock>),
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WireContentBlock {
    Text { text: String },
    ImageUrl { image_url: WireImageUrl },
}

#[derive(Debug, Serialize)]
pub struct WireImageUrl {
    pub url: String,
}

#[derive(Debug, Serialize)]
pub struct WireToolCall<'a> {
    pub id: &'a str,
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub function: WireToolCallFunction<'a>,
}

#[derive(Debug, Serialize)]
pub struct WireToolCallFunction<'a> {
    pub name: &'a str,
    pub arguments: String,
}

#[derive(Debug, Serialize)]
pub struct WireTool<'a> {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub function: WireToolFunction<'a>,
}

#[derive(Debug, Serialize)]
pub struct WireToolFunction<'a> {
    pub name: &'a str,
    pub description: &'a str,
    pub parameters: &'a Value,
}

// ===== Responses API (US model families: Grok / GPT-Luna / Muse) =====
//
// OpenCode Go serves these through the OpenAI *Responses* API (`/responses`),
// NOT `/chat/completions` (which 503s "Endpoint is unavailable" for them). The
// request shape differs: `input` items instead of `messages`, flat function
// tools, `max_output_tokens`, and `reasoning: { effort }`. The SSE response is
// parsed generically in `stream::map_responses_stream`.

#[derive(Debug, Serialize)]
pub struct ResponsesRequest<'a> {
    pub model: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<&'a str>,
    pub input: Vec<ResponsesInput>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ResponsesTool<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<ResponsesReasoning>,
    pub stream: bool,
}

#[derive(Debug, Serialize)]
pub struct ResponsesReasoning {
    pub effort: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum ResponsesInput {
    Message {
        role: &'static str,
        content: Vec<ResponsesContent>,
    },
    FunctionCall {
        #[serde(rename = "type")]
        kind: &'static str,
        call_id: String,
        name: String,
        arguments: String,
    },
    FunctionCallOutput {
        #[serde(rename = "type")]
        kind: &'static str,
        call_id: String,
        output: String,
    },
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponsesContent {
    InputText { text: String },
    OutputText { text: String },
    InputImage { image_url: String },
}

#[derive(Debug, Serialize)]
pub struct ResponsesTool<'a> {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub name: &'a str,
    pub description: &'a str,
    pub parameters: &'a Value,
}

#[derive(Debug, Deserialize)]
pub struct ChatChunk {
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default, deserialize_with = "null_seq_as_default")]
    pub choices: Vec<ChatChoice>,
    #[serde(default)]
    pub usage: Option<UsageBody>,
}

#[derive(Debug, Deserialize)]
pub struct ChatChoice {
    #[serde(default)]
    pub delta: ChatDelta,
    #[serde(default)]
    pub finish_reason: Option<String>,
    #[serde(default)]
    pub usage: Option<UsageBody>,
}

#[derive(Debug, Default, Deserialize)]
pub struct ChatDelta {
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub reasoning_content: Option<String>,
    #[serde(default, deserialize_with = "null_seq_as_default")]
    pub tool_calls: Vec<ToolCallDelta>,
}

/// Deserialize a sequence that some OpenCode Go models send as an explicit
/// `null` instead of omitting it (e.g. DeepSeek V4 Flash streams
/// `"tool_calls": null`). Plain `#[serde(default)]` handles a *missing* field
/// but errors on an explicit null ("invalid type: null, expected a sequence"),
/// so treat null the same as absent.
fn null_seq_as_default<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Option::<Vec<T>>::deserialize(deserializer)?.unwrap_or_default())
}

#[derive(Debug, Deserialize)]
pub struct ToolCallDelta {
    #[serde(default)]
    pub index: Option<usize>,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub function: Option<ToolCallFunctionDelta>,
}

#[derive(Debug, Deserialize)]
pub struct ToolCallFunctionDelta {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub arguments: Option<String>,
}

#[derive(Debug, Default, Clone, Deserialize)]
pub struct UsageBody {
    #[serde(default)]
    pub prompt_tokens: u32,
    #[serde(default)]
    pub completion_tokens: u32,
    #[serde(default)]
    pub total_tokens: u32,
    #[serde(default)]
    pub cached_tokens: Option<u32>,
    #[serde(default)]
    pub prompt_tokens_details: Option<PromptTokensDetails>,
    #[serde(default)]
    pub completion_tokens_details: Option<CompletionTokensDetails>,
}

#[derive(Debug, Default, Clone, Deserialize)]
pub struct PromptTokensDetails {
    #[serde(default)]
    pub cached_tokens: Option<u32>,
}

#[derive(Debug, Default, Clone, Deserialize)]
pub struct CompletionTokensDetails {
    #[serde(default)]
    pub reasoning_tokens: Option<u32>,
}

#[derive(Debug, Default, Deserialize)]
pub struct ApiErrorEnvelope {
    #[serde(default)]
    pub error: ApiErrorBody,
}

#[derive(Debug, Default, Deserialize)]
pub struct ApiErrorBody {
    #[serde(default, rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub code: Option<String>,
}
