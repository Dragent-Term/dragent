//! Direct provider transport: pure routing, request translation, and SSE
//! parsing for calling model providers straight from the local engine.
//!
//! This is the shared core of the server-to-local move: the per-step loop
//! driver stays transport-agnostic, and this module answers "which provider,
//! which endpoint, which wire shape" plus "bytes in → step events out". HTTP
//! execution lives app-side (ureq); everything here is pure serde/string work
//! so the future local engine (and unit tests) inherit it unchanged.
//!
//! Coverage: Anthropic natively; everything else through one
//! OpenAI-compatible client (OpenAI, xAI, Groq, Cerebras, Gemini, OpenRouter,
//! Ollama, LM Studio, custom base URLs). Provider-specific quirks are
//! documented at the call site; unknown shapes fall back to the cloud path,
//! never to a guess.

use serde_json::Value;

// ── Routing ─────────────────────────────────────────────────────────────────

/// Wire protocol used for a direct call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DirectKind {
    /// Anthropic Messages API (`/v1/messages`, SSE, `tool_use` blocks).
    Anthropic,
    /// OpenAI-compatible Chat Completions (`/chat/completions`, SSE,
    /// `tool_calls`). Covers xAI, Groq, Cerebras, Gemini, Meta, OpenRouter,
    /// Ollama, LM Studio, and custom endpoints.
    OpenAiCompatible,
    /// OpenAI Responses API (`/responses`, SSE, `function_call` items).
    /// The forward-compatible surface for the GPT-5.6 family and higher:
    /// first-party tool use goes here. Other providers stay on Chat
    /// Completions (their `/responses` support is unknown or absent).
    OpenAiResponses,
}

/// Endpoint flavor of a user-supplied custom model endpoint. Stored on the
/// settings record (serde default = chat completions, preserving existing
/// records); selects the wire client.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CustomEndpointStyle {
    #[default]
    ChatCompletions,
    Responses,
    AnthropicMessages,
}

impl CustomEndpointStyle {
    /// Parse the settings value. Unknown values are an error (fail fast so
    /// a typo never silently runs the wrong protocol).
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "chat_completions" | "chat" | "completions" => Ok(Self::ChatCompletions),
            "responses" => Ok(Self::Responses),
            "anthropic" | "anthropic_messages" | "messages" => Ok(Self::AnthropicMessages),
            other => Err(format!(
                "Unknown endpoint style '{other}'. Valid: chat_completions, responses, anthropic_messages."
            )),
        }
    }

    pub fn direct_kind(&self) -> DirectKind {
        match self {
            Self::ChatCompletions => DirectKind::OpenAiCompatible,
            Self::Responses => DirectKind::OpenAiResponses,
            Self::AnthropicMessages => DirectKind::Anthropic,
        }
    }
}

/// Static route for a provider id (slugs match the settings key slots).
#[derive(Clone, Copy, Debug)]
pub struct ProviderRoute {
    /// Settings key slot holding the credential, if the provider needs one.
    /// `None` = local server, no key (Ollama / LM Studio).
    pub key_slot: Option<&'static str>,
    pub base_url: &'static str,
    pub kind: DirectKind,
}

/// Route for a known provider id. `None` = unknown provider → cloud path.
pub fn route_for_provider(provider_id: &str) -> Option<ProviderRoute> {
    let route = match provider_id {
        "anthropic" => ProviderRoute {
            key_slot: Some("anthropic"),
            base_url: "https://api.anthropic.com",
            kind: DirectKind::Anthropic,
        },
        "openai" => ProviderRoute {
            key_slot: Some("openai"),
            base_url: "https://api.openai.com/v1",
            kind: DirectKind::OpenAiCompatible,
        },
        "xai" => ProviderRoute {
            key_slot: Some("xai"),
            base_url: "https://api.x.ai/v1",
            kind: DirectKind::OpenAiCompatible,
        },
        "groq" => ProviderRoute {
            key_slot: Some("groq"),
            base_url: "https://api.groq.com/openai/v1",
            kind: DirectKind::OpenAiCompatible,
        },
        "cerebras" => ProviderRoute {
            key_slot: Some("cerebras"),
            base_url: "https://api.cerebras.ai/v1",
            kind: DirectKind::OpenAiCompatible,
        },
        // Gemini's OpenAI-compatible endpoint. Tool-calling through it is
        // less battle-tested than the native API; transport errors fall back
        // to the cloud path, never to a guess.
        "google" => ProviderRoute {
            key_slot: Some("google"),
            base_url: "https://generativelanguage.googleapis.com/v1beta/openai/",
            kind: DirectKind::OpenAiCompatible,
        },
        "openrouter" => ProviderRoute {
            key_slot: Some("openrouter"),
            base_url: "https://openrouter.ai/api/v1",
            kind: DirectKind::OpenAiCompatible,
        },
        // Meta Llama API: OpenAI-compatible surface at /compat/v1 (chat,
        // streaming, vision, function calling). Bearer LLAMA_API_KEY.
        // Meta Model API (Muse Spark family only — the retired Llama API
        // preview is deliberately unsupported). Responses-first, matching
        // Meta's own flagship surface (reasoning replay, search grounding).
        "meta" => ProviderRoute {
            key_slot: Some("meta"),
            base_url: "https://api.meta.ai/v1",
            kind: DirectKind::OpenAiResponses,
        },
        // DeepSeek: OpenAI-compatible chat completions. Note: the reasoner
        // streams `reasoning_content` deltas alongside content; this client
        // surfaces the final content (thinking stays invisible, the answer
        // is intact).
        "deepseek" => ProviderRoute {
            key_slot: Some("deepseek"),
            base_url: "https://api.deepseek.com/v1",
            kind: DirectKind::OpenAiCompatible,
        },
        // Alibaba Qwen via Model Studio (international endpoint).
        "qwen" => ProviderRoute {
            key_slot: Some("qwen"),
            base_url: "https://dashscope-intl.aliyuncs.com/compatible-mode/v1",
            kind: DirectKind::OpenAiCompatible,
        },
        // Zhipu GLM.
        "zhipu" => ProviderRoute {
            key_slot: Some("zhipu"),
            base_url: "https://open.bigmodel.cn/api/paas/v4/",
            kind: DirectKind::OpenAiCompatible,
        },
        // Moonshot / Kimi (international endpoint).
        "moonshot" => ProviderRoute {
            key_slot: Some("moonshot"),
            base_url: "https://api.moonshot.ai/v1",
            kind: DirectKind::OpenAiCompatible,
        },
        // MiniMax (international endpoint).
        "minimax" => ProviderRoute {
            key_slot: Some("minimax"),
            base_url: "https://api.minimax.io/v1",
            kind: DirectKind::OpenAiCompatible,
        },
        // Mistral La Plateforme.
        "mistral" => ProviderRoute {
            key_slot: Some("mistral"),
            base_url: "https://api.mistral.ai/v1",
            kind: DirectKind::OpenAiCompatible,
        },
        // Together AI (open-model hosting).
        "together" => ProviderRoute {
            key_slot: Some("together"),
            base_url: "https://api.together.xyz/v1",
            kind: DirectKind::OpenAiCompatible,
        },
        // DeepInfra (open-model hosting).
        "deepinfra" => ProviderRoute {
            key_slot: Some("deepinfra"),
            base_url: "https://api.deepinfra.net/v1/openai",
            kind: DirectKind::OpenAiCompatible,
        },
        // Perplexity (chat completions; search is built into every model).
        "perplexity" => ProviderRoute {
            key_slot: Some("perplexity"),
            base_url: "https://api.perplexity.ai",
            kind: DirectKind::OpenAiCompatible,
        },
        // Venice (private inference).
        "venice" => ProviderRoute {
            key_slot: Some("venice"),
            base_url: "https://api.venice.ai/api/v1",
            kind: DirectKind::OpenAiCompatible,
        },
        // NVIDIA NIM.
        "nvidia" => ProviderRoute {
            key_slot: Some("nvidia"),
            base_url: "https://integrate.api.nvidia.com/v1",
            kind: DirectKind::OpenAiCompatible,
        },
        // Fireworks (open-model hosting).
        "fireworks" => ProviderRoute {
            key_slot: Some("fireworks"),
            base_url: "https://api.fireworks.ai/inference/v1",
            kind: DirectKind::OpenAiCompatible,
        },
        // Cohere (OpenAI-compatibility endpoint).
        "cohere" => ProviderRoute {
            key_slot: Some("cohere"),
            base_url: "https://api.cohere.com/compatibility/v1",
            kind: DirectKind::OpenAiCompatible,
        },
        "ollama" => ProviderRoute {
            key_slot: None,
            base_url: "http://localhost:11434/v1",
            kind: DirectKind::OpenAiCompatible,
        },
        "lmstudio" => ProviderRoute {
            key_slot: None,
            base_url: "http://localhost:1234/v1",
            kind: DirectKind::OpenAiCompatible,
        },
        _ => return None,
    };
    Some(route)
}

/// Routing prefixes (`groq/llama-…`, `ollama/qwen…`) stripped to the
/// provider-native model id. Unprefixed ids pass through unchanged.
pub fn native_model_id(model_id: &str) -> &str {
    for prefix in [
        "groq/",
        "cerebras/",
        "openrouter/",
        "ollama/",
        "lmstudio/",
        "google/",
        "meta/",
        "deepseek/",
        "qwen/",
        "alibaba/",
        "glm/",
        "zhipu/",
        "kimi/",
        "moonshot/",
        "minimax/",
        "mistral/",
        "together/",
        "deepinfra/",
        "perplexity/",
        "venice/",
        "nvidia/",
        "fireworks/",
        "cohere/",
    ] {
        if let Some(rest) = model_id.strip_prefix(prefix) {
            return rest;
        }
    }
    model_id
}

// ── Provider step events ────────────────────────────────────────────────────

/// One assembled tool call from a streamed step.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolUse {
    pub id: String,
    pub name: String,
    /// Complete arguments JSON object (string form, `{}` when absent).
    pub input_json: String,
}

/// Assembled result of one streamed provider step.
#[derive(Clone, Debug, Default)]
pub struct StepParseOutcome {
    pub text: String,
    pub tool_uses: Vec<ToolUse>,
    /// Provider-native searches executed server-side during the step
    /// (Anthropic `web_search_tool_result` blocks, Responses
    /// `web_search_call` items). Complete in-response: no client round trip
    /// needed, but the controller renders display-only cards for them.
    pub native_searches: Vec<NativeSearchCall>,
    pub input_tokens: i64,
    pub output_tokens: i64,
    /// Provider-reported failure (HTTP-level errors are handled by the
    /// transport, not here). `Some` means the step failed.
    pub error: Option<String>,
}

/// One provider-native search executed during a step. Billed to the user's
/// own provider key — no third-party deal, no cloud involved.
#[derive(Clone, Debug, PartialEq)]
pub struct NativeSearchCall {
    /// `"anthropic"` or `"openai"`.
    pub provider: String,
}

/// Cap on provider-native searches per direct request. Bounds spend on the
/// user's own key; the model gets a `max_uses_exceeded` result past it and
/// carries on with what it has.
pub const NATIVE_SEARCH_MAX_USES: u32 = 5;

// ── Local message schema → provider shapes ──────────────────────────────────
// Local history entries: `{role, content}` (+ `attachments` on user msgs),
// assistant turns gain `tool_calls: [{id, name, args}]`, tool results arrive
// as `{role: "tool", tool_call_id, content}`. Both request builders below
// translate that schema; unknown roles are skipped (documented, tested).

fn tool_defs_anthropic(extra_tools: &Value) -> Vec<Value> {
    extra_tools
        .as_array()
        .map(|tools| {
            tools
                .iter()
                .filter_map(|t| {
                    Some(serde_json::json!({
                        "name": t.get("name")?,
                        "description": t.get("description")?,
                        "input_schema": t.get("parameters")?,
                    }))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn tool_defs_openai(extra_tools: &Value) -> Vec<Value> {
    extra_tools
        .as_array()
        .map(|tools| {
            tools
                .iter()
                .filter_map(|t| {
                    Some(serde_json::json!({
                        "type": "function",
                        "function": {
                            "name": t.get("name")?,
                            "description": t.get("description")?,
                            "parameters": t.get("parameters")?,
                        },
                    }))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn image_block_anthropic(media_type: &str, data_b64: &str) -> Value {
    serde_json::json!({
        "type": "image",
        "source": { "type": "base64", "media_type": media_type, "data": data_b64 },
    })
}

/// Build an Anthropic `/v1/messages` streaming body.
///
/// `effort` is the pre-gated level name (`low`/`medium`/`high`, caller
/// applies the model-support table — notably GPT-style gating has no meaning
/// here and gpt-5.6-class rejection is handled by never sending it); sent as
/// `output_config.effort`, omitted when `None`.
pub fn anthropic_request_body(
    model: &str,
    system: &str,
    messages: &[Value],
    extra_tools: &Value,
    max_tokens: i64,
    effort: Option<&str>,
    native_search: bool,
) -> Value {
    let mut out = Vec::new();
    for m in messages {
        let role = m.get("role").and_then(|r| r.as_str()).unwrap_or("");
        match role {
            "user" if m.get("tool_call_id").is_none() => {
                let text = m
                    .get("content")
                    .and_then(|c| c.as_str())
                    .unwrap_or("")
                    .to_string();
                let mut blocks = Vec::new();
                if let Some(atts) = m.get("attachments").and_then(|a| a.as_array()) {
                    for a in atts {
                        if a.get("type").and_then(|t| t.as_str()) != Some("image") {
                            continue;
                        }
                        blocks.push(image_block_anthropic(
                            a.get("media_type")
                                .and_then(|t| t.as_str())
                                .unwrap_or("image/png"),
                            a.get("data").or_else(|| a.get("data_b64")).and_then(|d| d.as_str()).unwrap_or(""),
                        ));
                    }
                }
                if !text.is_empty() || blocks.is_empty() {
                    blocks.push(serde_json::json!({ "type": "text", "text": text }));
                }
                out.push(serde_json::json!({ "role": "user", "content": blocks }));
            }
            "assistant" => {
                let mut blocks = Vec::new();
                let text = m.get("content").and_then(|c| c.as_str()).unwrap_or("");
                if !text.is_empty() {
                    blocks.push(serde_json::json!({ "type": "text", "text": text }));
                }
                if let Some(calls) = m.get("tool_calls").and_then(|c| c.as_array()) {
                    for call in calls {
                        let input: Value = call
                            .get("args")
                            .and_then(|a| a.as_str())
                            .and_then(|s| serde_json::from_str(s).ok())
                            .unwrap_or(serde_json::json!({}));
                        blocks.push(serde_json::json!({
                            "type": "tool_use",
                            "id": call.get("id"),
                            "name": call.get("name"),
                            "input": input,
                        }));
                    }
                }
                out.push(serde_json::json!({ "role": "assistant", "content": blocks }));
            }
            _ if m.get("tool_call_id").is_some() => {
                let content = m
                    .get("content")
                    .and_then(|c| c.as_str())
                    .unwrap_or("")
                    .to_string();
                out.push(serde_json::json!({
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": m.get("tool_call_id"),
                        "content": content,
                    }],
                }));
            }
            _ => {
                // Unknown role without a tool_call_id: skip rather than guess.
            }
        }
    }
    let mut tools = tool_defs_anthropic(extra_tools);
    if native_search {
        // Basic server-side search (direct calls, results inline). Newer
        // versions default to code-execution-coupled filtering, which this
        // client does not drive — basic keeps results in-response.
        tools.push(serde_json::json!({
            "type": "web_search_20250305",
            "name": "web_search",
            "max_uses": NATIVE_SEARCH_MAX_USES,
        }));
    }
    let mut body = serde_json::json!({
        "model": model,
        "max_tokens": max_tokens,
        "system": system,
        "messages": out,
        "tools": tools,
        "stream": true,
    });
    if let Some(level) = effort {
        body["output_config"] = serde_json::json!({ "effort": level });
    }
    body
}

/// Build an OpenAI-compatible `/chat/completions` streaming body.
///
/// `effort` is the pre-gated level name, sent as `reasoning_effort` for the
/// providers whose vocabulary includes it (OpenAI reasoning models, xAI);
/// omitted when `None` — which must already cover gpt-5.6-class models that
/// reject any level once function tools are present.
pub fn openai_request_body(
    model: &str,
    messages: &[Value],
    extra_tools: &Value,
    effort: Option<&str>,
) -> Value {
    let mut out = Vec::new();
    for m in messages {
        let role = m.get("role").and_then(|r| r.as_str()).unwrap_or("");
        match role {
            "user" if m.get("tool_call_id").is_none() => {
                let text = m
                    .get("content")
                    .and_then(|c| c.as_str())
                    .unwrap_or("")
                    .to_string();
                let has_images = m
                    .get("attachments")
                    .and_then(|a| a.as_array())
                    .map(|atts| {
                        atts.iter()
                            .any(|a| a.get("type").and_then(|t| t.as_str()) == Some("image"))
                    })
                    .unwrap_or(false);
                if !has_images {
                    out.push(serde_json::json!({ "role": "user", "content": text }));
                    continue;
                }
                let mut parts = Vec::new();
                if !text.is_empty() {
                    parts.push(serde_json::json!({ "type": "text", "text": text }));
                }
                for a in m.get("attachments").and_then(|a| a.as_array()).unwrap() {
                    if a.get("type").and_then(|t| t.as_str()) != Some("image") {
                        continue;
                    }
                    let media = a
                        .get("media_type")
                        .and_then(|t| t.as_str())
                        .unwrap_or("image/png");
                    let data = a.get("data").or_else(|| a.get("data_b64")).and_then(|d| d.as_str()).unwrap_or("");
                    parts.push(serde_json::json!({
                        "type": "image_url",
                        "image_url": { "url": format!("data:{media};base64,{data}") },
                    }));
                }
                out.push(serde_json::json!({ "role": "user", "content": parts }));
            }
            "assistant" => {
                let text = m.get("content").and_then(|c| c.as_str()).unwrap_or("");
                let calls: Vec<Value> = m
                    .get("tool_calls")
                    .and_then(|c| c.as_array())
                    .map(|calls| {
                        calls
                            .iter()
                            .map(|call| {
                                serde_json::json!({
                                    "id": call.get("id"),
                                    "type": "function",
                                    "function": {
                                        "name": call.get("name"),
                                        "arguments": call.get("args").and_then(|a| a.as_str()).unwrap_or("{}"),
                                    },
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let mut msg = serde_json::json!({ "role": "assistant" });
                if calls.is_empty() {
                    msg["content"] = serde_json::Value::String(text.to_string());
                } else {
                    msg["content"] = match text.is_empty() {
                        true => Value::Null,
                        false => serde_json::Value::String(text.to_string()),
                    };
                    msg["tool_calls"] = Value::Array(calls);
                }
                out.push(msg);
            }
            _ if m.get("tool_call_id").is_some() => {
                out.push(serde_json::json!({
                    "role": "tool",
                    "tool_call_id": m.get("tool_call_id"),
                    "content": m.get("content").and_then(|c| c.as_str()).unwrap_or(""),
                }));
            }
            _ => {}
        }
    }
    let mut body = serde_json::json!({
        "model": model,
        "messages": out,
        "tools": tool_defs_openai(extra_tools),
        "stream": true,
        "stream_options": { "include_usage": true },
    });
    if let Some(level) = effort {
        body["reasoning_effort"] = serde_json::Value::String(level.to_string());
    }
    body
}

/// Build an OpenAI `/responses` streaming body — the forward-compatible
/// surface for the GPT-5.6 family and higher, where first-party tool use
/// lives. `instructions` carries the system prompt; `effort` maps to
/// `reasoning.effort` (pre-gated by the caller; gpt-5.6-class omits it).
///
/// `store: false` is deliberate and load-bearing: Responses stores server
/// side by default, which would retain BYOK prompts on OpenAI's side beyond
/// the inference itself. Local direct mode must not opt into retention.
pub fn openai_responses_request_body(
    model: &str,
    instructions: &str,
    messages: &[Value],
    extra_tools: &Value,
    effort: Option<&str>,
    native_search: bool,
) -> Value {
    let mut input = Vec::new();
    for m in messages {
        let role = m.get("role").and_then(|r| r.as_str()).unwrap_or("");
        match role {
            "user" if m.get("tool_call_id").is_none() => {
                let text = m
                    .get("content")
                    .and_then(|c| c.as_str())
                    .unwrap_or("")
                    .to_string();
                let mut parts = Vec::new();
                if !text.is_empty()
                    || !m
                        .get("attachments")
                        .is_some_and(|a| a.as_array().map(|atts| !atts.is_empty()).unwrap_or(false))
                {
                    parts.push(serde_json::json!({ "type": "input_text", "text": text }));
                }
                if let Some(atts) = m.get("attachments").and_then(|a| a.as_array()) {
                    for a in atts {
                        if a.get("type").and_then(|t| t.as_str()) != Some("image") {
                            continue;
                        }
                        let media = a
                            .get("media_type")
                            .and_then(|t| t.as_str())
                            .unwrap_or("image/png");
                        let data = a.get("data").or_else(|| a.get("data_b64")).and_then(|d| d.as_str()).unwrap_or("");
                        parts.push(serde_json::json!({
                            "type": "input_image",
                            "image_url": format!("data:{media};base64,{data}"),
                        }));
                    }
                }
                input.push(serde_json::json!({
                    "type": "message", "role": "user", "content": parts,
                }));
            }
            "assistant" => {
                let text = m.get("content").and_then(|c| c.as_str()).unwrap_or("");
                if !text.is_empty() {
                    input.push(serde_json::json!({
                        "type": "message",
                        "role": "assistant",
                        "content": [{ "type": "output_text", "text": text }],
                    }));
                }
                if let Some(calls) = m.get("tool_calls").and_then(|c| c.as_array()) {
                    for call in calls {
                        input.push(serde_json::json!({
                            "type": "function_call",
                            "call_id": call.get("id"),
                            "name": call.get("name"),
                            "arguments": call.get("args").and_then(|a| a.as_str()).unwrap_or("{}"),
                        }));
                    }
                }
            }
            _ if m.get("tool_call_id").is_some() => {
                input.push(serde_json::json!({
                    "type": "function_call_output",
                    "call_id": m.get("tool_call_id"),
                    "output": m.get("content").and_then(|c| c.as_str()).unwrap_or(""),
                }));
            }
            _ => {}
        }
    }
    let mut tools = tool_defs_openai(extra_tools);
    if native_search {
        // Provider-native search, billed to the user's own key. Results
        // arrive as annotated model text; no client round trip needed.
        tools.push(serde_json::json!({ "type": "web_search" }));
    }
    let mut body = serde_json::json!({
        "model": model,
        "instructions": instructions,
        "input": input,
        "tools": tools,
        "stream": true,
        "store": false,
    });
    if let Some(level) = effort {
        body["reasoning"] = serde_json::json!({ "effort": level });
    }
    body
}

// ── SSE parsers ─────────────────────────────────────────────────────────────
// Both parsers are line-fed (`feed` per chunk, `finish` at EOF) so transports
// stream arbitrarily-sized bodies without buffering the whole response.

/// Anthropic Messages SSE → step outcome.
#[derive(Debug, Default)]
pub struct AnthropicStreamParser {
    buffer: String,
    pending_event: String,
    text: String,
    tools: Vec<ToolUse>,
    current_tool: Option<ToolUse>,
    current_tool_index: Option<u64>,
    /// In-flight server-side block kinds by content index
    /// (`server_tool_use`, `web_search_tool_result`, …).
    server_blocks: std::collections::HashMap<u64, String>,
    native_searches: Vec<NativeSearchCall>,
    input_tokens: i64,
    output_tokens: i64,
    error: Option<String>,
}

/// Upper bounds on scraped search evidence (context protection).
const SEARCH_EVIDENCE_SOURCES: usize = 3;
const SEARCH_EVIDENCE_CHARS: usize = 500;

/// Best-effort text scrape of a `web_search_tool_result` block into
/// `sources` as `(title, url, excerpt)`. Shapes vary by version; anything
/// unrecognized is skipped rather than guessed.
fn scrape_search_result(block: &Value, sources: &mut Vec<(String, String, String)>) {
    let content = match block.get("content").and_then(|c| c.as_array()) {
        Some(c) => c,
        None => return,
    };
    for item in content {
        if sources.len() >= SEARCH_EVIDENCE_SOURCES {
            break;
        }
        let item_type = item.get("type").and_then(|t| t.as_str()).unwrap_or("");
        if item_type == "text"
            && let Some(t) = item.get("text").and_then(|v| v.as_str())
            && !t.trim().is_empty()
        {
            sources.push((
                String::new(),
                String::new(),
                truncate_chars(t, SEARCH_EVIDENCE_CHARS),
            ));
        } else if item_type == "web_search_result" {
            let title = item.get("title").and_then(|v| v.as_str()).unwrap_or("");
            let url = item.get("url").and_then(|v| v.as_str()).unwrap_or("");
            let mut excerpt = String::new();
            if let Some(inner) = item.get("content").and_then(|c| c.as_array()) {
                for part in inner {
                    if part.get("type").and_then(|t| t.as_str()) == Some("text")
                        && let Some(t) = part.get("text").and_then(|v| v.as_str())
                    {
                        if !excerpt.is_empty() {
                            excerpt.push(' ');
                        }
                        excerpt.push_str(t);
                    }
                }
            }
            if !title.is_empty() || !url.is_empty() || !excerpt.is_empty() {
                sources.push((
                    title.to_string(),
                    url.to_string(),
                    truncate_chars(&excerpt, SEARCH_EVIDENCE_CHARS),
                ));
            }
        }
    }
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

impl AnthropicStreamParser {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn feed(&mut self, chunk: &str) {
        self.buffer.push_str(chunk);
        while let Some(pos) = self.buffer.find('\n') {
            let line = self.buffer[..pos].trim_end_matches('\r').to_string();
            self.buffer.drain(..=pos);
            self.handle_line(&line);
        }
    }

    fn handle_line(&mut self, line: &str) {
        if line.is_empty() {
            self.pending_event.clear();
            return;
        }
        if let Some(kind) = line.strip_prefix("event: ") {
            self.pending_event = kind.trim().to_string();
            return;
        }
        let data = match line.strip_prefix("data: ") {
            Some(d) => d,
            None => return,
        };
        let value: Value = match serde_json::from_str(data) {
            Ok(v) => v,
            Err(_) => return,
        };
        match self.pending_event.as_str() {
            "message_start" => {
                self.input_tokens = value
                    .pointer("/message/usage/input_tokens")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
            }
            "content_block_start" => {
                let block = &value["content_block"];
                let index = value.get("index").and_then(|v| v.as_u64()).unwrap_or(0);
                match block.get("type").and_then(|t| t.as_str()) {
                    Some("tool_use") => {
                        self.current_tool = Some(ToolUse {
                            id: block
                                .get("id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                            name: block
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                            input_json: String::new(),
                        });
                        self.current_tool_index = Some(index);
                    }
                    // Provider-native search result: complete in-response
                    // (no client round trip). Count it and scrape evidence;
                    // the model already saw it server-side.
                    Some("web_search_tool_result") => {
                        self.server_blocks
                            .insert(index, "web_search_tool_result".to_string());
                        let mut sources = Vec::new();
                        scrape_search_result(block, &mut sources);
                        self.record_search_evidence("anthropic", &sources);
                    }
                    Some(other) => {
                        self.server_blocks.insert(index, other.to_string());
                    }
                    None => {}
                }
            }
            "content_block_delta" => {
                let delta = &value["delta"];
                let index = value.get("index").and_then(|v| v.as_u64()).unwrap_or(0);
                match delta.get("type").and_then(|t| t.as_str()) {
                    Some("text_delta") => {
                        if let Some(t) = delta.get("text").and_then(|v| v.as_str()) {
                            self.text.push_str(t);
                        }
                    }
                    Some("input_json_delta") => {
                        if let Some(p) = delta.get("partial_json").and_then(|v| v.as_str()) {
                            // Index-matched when the stream tells us the
                            // index (parallel calls); legacy streams omit
                            // reliable indexes, so fall back to in-flight.
                            let mine = self.current_tool_index.map(|i| i == index).unwrap_or(true);
                            if mine && let Some(tool) = self.current_tool.as_mut() {
                                tool.input_json.push_str(p);
                            }
                        }
                    }
                    // Server-result blocks occasionally stream follow-on
                    // fragments; scrape defensively, count once (at start).
                    _ => {
                        if self.server_blocks.get(&index).map(|k| k.as_str())
                            == Some("web_search_tool_result")
                        {
                            let mut sources = Vec::new();
                            scrape_search_result(delta, &mut sources);
                            self.record_search_evidence("anthropic", &sources);
                        }
                    }
                }
            }
            "content_block_stop" => {
                let index = value.get("index").and_then(|v| v.as_u64()).unwrap_or(0);
                // Take the client tool only when the stop belongs to it;
                // server-block stops must never steal it (both shapes
                // interleave when native search and client tools mix).
                if self.current_tool_index == Some(index) {
                    self.current_tool_index = None;
                    if let Some(mut tool) = self.current_tool.take() {
                        if tool.input_json.trim().is_empty() {
                            tool.input_json = "{}".to_string();
                        }
                        self.tools.push(tool);
                    }
                } else {
                    self.server_blocks.remove(&index);
                }
            }
            "message_delta" => {
                if let Some(out) = value
                    .pointer("/usage/output_tokens")
                    .and_then(|v| v.as_i64())
                {
                    self.output_tokens = out;
                }
            }
            "error" => {
                self.error = Some(
                    value
                        .pointer("/error/message")
                        .and_then(|v| v.as_str())
                        .unwrap_or("anthropic stream error")
                        .to_string(),
                );
            }
            _ => {}
        }
    }

    pub fn finish(mut self) -> StepParseOutcome {
        // Flush any unterminated remainder (a well-formed stream ends with
        // message_stop; a truncated one still yields what arrived).
        let tail = std::mem::take(&mut self.buffer);
        if !tail.trim().is_empty() {
            for line in tail.lines() {
                self.handle_line(line.trim_end_matches('\r'));
            }
        }
        if let Some(mut tool) = self.current_tool.take() {
            if tool.input_json.trim().is_empty() {
                tool.input_json = "{}".to_string();
            }
            self.tools.push(tool);
        }
        StepParseOutcome {
            text: self.text,
            tool_uses: self.tools,
            native_searches: self.native_searches,
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            error: self.error,
        }
    }

    /// Record one native search plus its scraped evidence. Deduplicated by
    /// construction (one record per result block); empty scrapes still
    /// record — the search happened and was billed.
    fn record_search_evidence(&mut self, provider: &str, sources: &[(String, String, String)]) {
        self.native_searches.push(NativeSearchCall {
            provider: provider.to_string(),
        });
        if sources.is_empty() {
            return;
        }
        if !self.text.is_empty() && !self.text.ends_with('\n') {
            self.text.push('\n');
        }
        self.text.push_str("\n[web search results]\n");
        for (title, url, excerpt) in sources {
            if title.is_empty() && url.is_empty() {
                self.text.push_str(&format!("- {excerpt}\n"));
            } else {
                self.text
                    .push_str(&format!("- {title} ({url}): {excerpt}\n"));
            }
        }
    }
}

/// OpenAI-compatible Chat Completions SSE → step outcome.
#[derive(Debug, Default)]
pub struct OpenAiStreamParser {
    buffer: String,
    text: String,
    tools: Vec<ToolUse>,
    input_tokens: i64,
    output_tokens: i64,
    error: Option<String>,
}

impl OpenAiStreamParser {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn feed(&mut self, chunk: &str) {
        self.buffer.push_str(chunk);
        while let Some(pos) = self.buffer.find('\n') {
            let line = self.buffer[..pos].trim_end_matches('\r').to_string();
            self.buffer.drain(..=pos);
            self.handle_line(&line);
        }
    }

    fn tool_at(&mut self, index: usize) -> &mut ToolUse {
        while self.tools.len() <= index {
            self.tools.push(ToolUse {
                id: String::new(),
                name: String::new(),
                input_json: String::new(),
            });
        }
        &mut self.tools[index]
    }

    fn handle_line(&mut self, line: &str) {
        let data = match line.strip_prefix("data: ") {
            Some(d) => d.trim(),
            None => return,
        };
        if data.is_empty() || data == "[DONE]" {
            return;
        }
        let value: Value = match serde_json::from_str(data) {
            Ok(v) => v,
            Err(_) => return,
        };
        if let Some(err) = value.get("error") {
            self.error = Some(
                err.get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("provider stream error")
                    .to_string(),
            );
            return;
        }
        if let Some(usage) = value.get("usage") {
            self.input_tokens = usage
                .get("prompt_tokens")
                .and_then(|v| v.as_i64())
                .unwrap_or(self.input_tokens);
            self.output_tokens = usage
                .get("completion_tokens")
                .and_then(|v| v.as_i64())
                .unwrap_or(self.output_tokens);
        }
        let delta = match value
            .pointer("/choices/0/delta")
            .or_else(|| value.pointer("/choices/0/message"))
        {
            Some(d) => d,
            None => return,
        };
        if let Some(t) = delta.get("content").and_then(|v| v.as_str()) {
            self.text.push_str(t);
        }
        if let Some(calls) = delta.get("tool_calls").and_then(|v| v.as_array()) {
            for call in calls {
                let index = call.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                let tool = self.tool_at(index);
                if let Some(id) = call.get("id").and_then(|v| v.as_str())
                    && !id.is_empty()
                {
                    tool.id = id.to_string();
                }
                if let Some(func) = call.get("function") {
                    if let Some(name) = func.get("name").and_then(|v| v.as_str())
                        && !name.is_empty()
                    {
                        tool.name = name.to_string();
                    }
                    if let Some(args) = func.get("arguments").and_then(|v| v.as_str()) {
                        tool.input_json.push_str(args);
                    }
                }
            }
        }
    }

    pub fn finish(mut self) -> StepParseOutcome {
        let tail = std::mem::take(&mut self.buffer);
        if !tail.trim().is_empty() {
            for line in tail.lines() {
                self.handle_line(line.trim_end_matches('\r'));
            }
        }
        for tool in &mut self.tools {
            if tool.input_json.trim().is_empty() {
                tool.input_json = "{}".to_string();
            }
        }
        // Drop placeholder slots that never received an id and a name
        // (providers that emit empty trailing tool_call entries).
        self.tools
            .retain(|t| !(t.id.is_empty() && t.name.is_empty()));
        StepParseOutcome {
            text: self.text,
            tool_uses: self.tools,
            native_searches: Vec::new(),
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            error: self.error,
        }
    }
}

/// OpenAI Responses API SSE → step outcome (`/responses` stream).
///
/// Tracks function calls per `output_index`: `output_item.added` binds the
/// call id + name, `function_call_arguments.delta` accumulates, and
/// `output_item.done` carries the authoritative final arguments (preferred
/// when present). Text arrives via `output_text.delta`.
#[derive(Debug, Default)]
pub struct OpenAiResponsesParser {
    buffer: String,
    pending_event: String,
    text: String,
    calls: Vec<ResponseCall>,
    native_searches: Vec<NativeSearchCall>,
    input_tokens: i64,
    output_tokens: i64,
    error: Option<String>,
}

#[derive(Debug, Default)]
struct ResponseCall {
    item_id: String,
    call_id: String,
    name: String,
    args_acc: String,
    args_done: Option<String>,
}

impl OpenAiResponsesParser {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn feed(&mut self, chunk: &str) {
        self.buffer.push_str(chunk);
        while let Some(pos) = self.buffer.find('\n') {
            let line = self.buffer[..pos].trim_end_matches('\r').to_string();
            self.buffer.drain(..=pos);
            self.handle_line(&line);
        }
    }

    fn call_at(&mut self, index: usize) -> &mut ResponseCall {
        while self.calls.len() <= index {
            self.calls.push(ResponseCall::default());
        }
        &mut self.calls[index]
    }

    fn handle_line(&mut self, line: &str) {
        if line.is_empty() {
            self.pending_event.clear();
            return;
        }
        if let Some(kind) = line.strip_prefix("event: ") {
            self.pending_event = kind.trim().to_string();
            return;
        }
        let data = match line.strip_prefix("data: ") {
            Some(d) => d,
            None => return,
        };
        let value: Value = match serde_json::from_str(data) {
            Ok(v) => v,
            Err(_) => return,
        };
        match self.pending_event.as_str() {
            "response.output_text.delta" => {
                if let Some(t) = value.get("delta").and_then(|v| v.as_str()) {
                    self.text.push_str(t);
                }
            }
            "response.output_item.added" => {
                let index = value
                    .get("output_index")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as usize;
                let item = &value["item"];
                // Provider-native search call: complete server-side, counted
                // for the display card. Text annotations flow via deltas.
                if item.get("type").and_then(|v| v.as_str()) == Some("web_search_call") {
                    self.native_searches.push(NativeSearchCall {
                        provider: "openai".to_string(),
                    });
                    return;
                }
                if item.get("type").and_then(|v| v.as_str()) == Some("function_call") {
                    let call = self.call_at(index);
                    call.item_id = item
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    call.call_id = item
                        .get("call_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    call.name = item
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                }
            }
            "response.function_call_arguments.delta" => {
                let index = value
                    .get("output_index")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as usize;
                if let Some(d) = value.get("delta").and_then(|v| v.as_str()) {
                    self.call_at(index).args_acc.push_str(d);
                }
            }
            "response.output_item.done" => {
                let index = value
                    .get("output_index")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as usize;
                let item = &value["item"];
                if item.get("type").and_then(|v| v.as_str()) == Some("function_call") {
                    let call = self.call_at(index);
                    // Authoritative final arguments; preferred over deltas.
                    if let Some(args) = item.get("arguments").and_then(|v| v.as_str()) {
                        call.args_done = Some(args.to_string());
                    }
                    if call.call_id.is_empty() {
                        call.call_id = item
                            .get("call_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                    }
                    if call.name.is_empty() {
                        call.name = item
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                    }
                }
            }
            "response.completed" => {
                if let Some(usage) = value.pointer("/response/usage") {
                    self.input_tokens = usage
                        .get("input_tokens")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(self.input_tokens);
                    self.output_tokens = usage
                        .get("output_tokens")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(self.output_tokens);
                }
                if value.pointer("/response/status").and_then(|v| v.as_str()) != Some("completed") {
                    self.error = Some(format!(
                        "response ended with status {}",
                        value
                            .pointer("/response/status")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown")
                    ));
                }
            }
            "response.failed" => {
                self.error = Some(
                    value
                        .pointer("/response/error/message")
                        .and_then(|v| v.as_str())
                        .unwrap_or("response failed")
                        .to_string(),
                );
            }
            // response.incomplete (max tokens), created/in_progress/queued,
            // content-part events: text already captured via deltas; an
            // incomplete response is a done-with-what-arrived, not an error.
            _ => {}
        }
    }

    pub fn finish(mut self) -> StepParseOutcome {
        let tail = std::mem::take(&mut self.buffer);
        if !tail.trim().is_empty() {
            for line in tail.lines() {
                self.handle_line(line.trim_end_matches('\r'));
            }
        }
        let mut tool_uses = Vec::new();
        for call in self.calls {
            if call.call_id.is_empty() && call.name.is_empty() && call.args_acc.is_empty() {
                continue;
            }
            let input_json = call
                .args_done
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| {
                    if call.args_acc.trim().is_empty() {
                        "{}".to_string()
                    } else {
                        call.args_acc
                    }
                });
            tool_uses.push(ToolUse {
                id: if call.call_id.is_empty() {
                    call.item_id
                } else {
                    call.call_id
                },
                name: call.name,
                input_json,
            });
        }
        StepParseOutcome {
            text: self.text,
            tool_uses,
            native_searches: self.native_searches,
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            error: self.error,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_all_known_providers() {
        for (id, kind, slot) in [
            ("anthropic", DirectKind::Anthropic, Some("anthropic")),
            ("openai", DirectKind::OpenAiCompatible, Some("openai")),
            ("xai", DirectKind::OpenAiCompatible, Some("xai")),
            ("groq", DirectKind::OpenAiCompatible, Some("groq")),
            ("cerebras", DirectKind::OpenAiCompatible, Some("cerebras")),
            ("google", DirectKind::OpenAiCompatible, Some("google")),
            (
                "openrouter",
                DirectKind::OpenAiCompatible,
                Some("openrouter"),
            ),
            ("meta", DirectKind::OpenAiResponses, Some("meta")),
            ("deepseek", DirectKind::OpenAiCompatible, Some("deepseek")),
            ("qwen", DirectKind::OpenAiCompatible, Some("qwen")),
            ("zhipu", DirectKind::OpenAiCompatible, Some("zhipu")),
            ("moonshot", DirectKind::OpenAiCompatible, Some("moonshot")),
            ("minimax", DirectKind::OpenAiCompatible, Some("minimax")),
            ("mistral", DirectKind::OpenAiCompatible, Some("mistral")),
            ("together", DirectKind::OpenAiCompatible, Some("together")),
            ("deepinfra", DirectKind::OpenAiCompatible, Some("deepinfra")),
            (
                "perplexity",
                DirectKind::OpenAiCompatible,
                Some("perplexity"),
            ),
            ("venice", DirectKind::OpenAiCompatible, Some("venice")),
            ("nvidia", DirectKind::OpenAiCompatible, Some("nvidia")),
            ("fireworks", DirectKind::OpenAiCompatible, Some("fireworks")),
            ("cohere", DirectKind::OpenAiCompatible, Some("cohere")),
            ("ollama", DirectKind::OpenAiCompatible, None),
            ("lmstudio", DirectKind::OpenAiCompatible, None),
        ] {
            let route = route_for_provider(id).unwrap_or_else(|| panic!("{id}"));
            assert_eq!(route.kind, kind, "{id}");
            assert_eq!(route.key_slot, slot, "{id}");
            assert!(!route.base_url.is_empty());
        }
        assert!(route_for_provider("unknown-provider").is_none());
    }

    #[test]
    fn native_model_id_strips_routing_prefixes() {
        assert_eq!(native_model_id("groq/llama-3.3-70b"), "llama-3.3-70b");
        assert_eq!(native_model_id("ollama/qwen3"), "qwen3");
        assert_eq!(
            native_model_id("openrouter/anthropic/claude-x"),
            "anthropic/claude-x"
        );
        assert_eq!(native_model_id("claude-sonnet-4-6"), "claude-sonnet-4-6");
        assert_eq!(native_model_id("gpt-5"), "gpt-5");
    }

    fn sample_messages() -> Vec<Value> {
        vec![
            serde_json::json!({"role": "user", "content": "hello"}),
            serde_json::json!({
                "role": "assistant",
                "content": "reading",
                "tool_calls": [{"id": "c1", "name": "read_file", "args": "{\"path\":\"a\"}"}],
            }),
            serde_json::json!({"role": "tool", "tool_call_id": "c1", "content": "file!"}),
        ]
    }

    fn sample_tools() -> Value {
        serde_json::json!([
            {"name": "read_file", "description": "read", "parameters": {"type": "object"}},
        ])
    }

    #[test]
    fn custom_endpoint_styles_parse() {
        assert_eq!(
            CustomEndpointStyle::parse("chat_completions"),
            Ok(CustomEndpointStyle::ChatCompletions)
        );
        assert_eq!(
            CustomEndpointStyle::parse("responses"),
            Ok(CustomEndpointStyle::Responses)
        );
        assert_eq!(
            CustomEndpointStyle::parse("anthropic"),
            Ok(CustomEndpointStyle::AnthropicMessages)
        );
        assert!(CustomEndpointStyle::parse("graphql").is_err());
        assert_eq!(
            CustomEndpointStyle::Responses.direct_kind(),
            DirectKind::OpenAiResponses
        );
    }

    #[test]
    fn anthropic_body_maps_all_message_shapes() {
        let body = anthropic_request_body(
            "claude-x",
            "sys",
            &sample_messages(),
            &sample_tools(),
            99,
            Some("medium"),
            false,
        );
        assert_eq!(body["model"], "claude-x");
        assert_eq!(body["max_tokens"], 99);
        assert_eq!(body["system"], "sys");
        assert_eq!(body["stream"], true);
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[1]["content"][1]["type"], "tool_use");
        assert_eq!(msgs[1]["content"][1]["input"]["path"], "a");
        assert_eq!(msgs[2]["content"][0]["type"], "tool_result");
        assert_eq!(body["tools"][0]["input_schema"]["type"], "object");
        assert_eq!(body["output_config"]["effort"], "medium");
        let no_effort = anthropic_request_body(
            "claude-x",
            "sys",
            &sample_messages(),
            &sample_tools(),
            99,
            None,
            false,
        );
        assert!(no_effort.get("output_config").is_none());
    }

    #[test]
    fn native_search_defs_append_only_when_enabled() {
        let with = anthropic_request_body("c", "s", &[], &sample_tools(), 99, None, true);
        let tools = with["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 2);
        let search = tools
            .iter()
            .find(|t| t.get("name") == Some(&serde_json::json!("web_search")));
        let search = search.expect("web_search def");
        assert_eq!(search["type"], "web_search_20250305");
        assert_eq!(search["max_uses"], NATIVE_SEARCH_MAX_USES);

        let without = anthropic_request_body("c", "s", &[], &sample_tools(), 99, None, false);
        assert_eq!(without["tools"].as_array().unwrap().len(), 1);

        let resp = openai_responses_request_body(
            "g",
            "s",
            &sample_messages(),
            &sample_tools(),
            None,
            true,
        );
        let rtools = resp["tools"].as_array().unwrap();
        assert!(
            rtools
                .iter()
                .any(|t| t.get("type") == Some(&serde_json::json!("web_search")))
        );

        // Chat completions never get native search (no such provider tool).
        let chat = openai_request_body("g", &sample_messages(), &sample_tools(), None);
        assert_eq!(chat["tools"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn openai_body_maps_all_message_shapes() {
        let body = openai_request_body("gpt-x", &sample_messages(), &sample_tools(), Some("high"));
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[1]["tool_calls"][0]["function"]["name"], "read_file");
        assert_eq!(msgs[2]["role"], "tool");
        assert_eq!(body["stream"], true);
        assert_eq!(body["reasoning_effort"], "high");
    }

    #[test]
    fn responses_body_maps_shapes_and_never_stores() {
        let body = openai_responses_request_body(
            "gpt-5.6-luna",
            "sys",
            &sample_messages(),
            &sample_tools(),
            None,
            false,
        );
        // GPT-5.6-class must omit effort (rejects it with tools present).
        assert!(body.get("reasoning").is_none());
        // BYOK privacy: never opt into server-side retention.
        assert_eq!(body["store"], false);
        assert_eq!(body["instructions"], "sys");
        let input = body["input"].as_array().unwrap();
        // User message, assistant text, function call, function output.
        assert_eq!(input.len(), 4);
        assert_eq!(input[2]["type"], "function_call");
        assert_eq!(input[2]["arguments"], "{\"path\":\"a\"}");
        assert_eq!(input[3]["type"], "function_call_output");
        assert_eq!(input[3]["output"], "file!");
    }

    #[test]
    fn anthropic_parser_assembles_text_tool_and_usage() {
        let mut p = AnthropicStreamParser::new();
        p.feed("event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":12}}}\n\n");
        p.feed("event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"hi\"}}\n\n");
        p.feed("event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"t1\",\"name\":\"grep\"}}\n\n");
        // Arguments arrive as fragments across deltas; the second feed is
        // additionally cut mid-chunk so the line buffer is exercised too.
        // Payloads are built with json! so the test cannot drift from real
        // framing again.
        let frag1 = serde_json::json!({
            "type": "content_block_delta",
            "index": 1,
            "delta": {"type": "input_json_delta", "partial_json": "{\"p\""},
        })
        .to_string();
        p.feed(&format!("event: content_block_delta\ndata: {frag1}\n\n"));
        let frag2 = serde_json::json!({
            "type": "content_block_delta",
            "index": 1,
            "delta": {"type": "input_json_delta", "partial_json": ":\"x\"}"},
        })
        .to_string();
        let feed2 = format!("event: content_block_delta\ndata: {frag2}\n\n");
        let mut cut = feed2.len() / 2;
        while !feed2.is_char_boundary(cut) {
            cut += 1;
        }
        p.feed(&feed2[..cut]);
        p.feed(&feed2[cut..]);
        p.feed(
            "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":1}\n\n",
        );
        p.feed("event: message_delta\ndata: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":7}}\n\n");
        let out = p.finish();
        assert_eq!(out.text, "hi");
        assert_eq!(out.tool_uses.len(), 1);
        assert_eq!(out.tool_uses[0].id, "t1");
        assert_eq!(out.tool_uses[0].name, "grep");
        assert_eq!(out.tool_uses[0].input_json, "{\"p\":\"x\"}");
        assert_eq!(out.input_tokens, 12);
        assert_eq!(out.output_tokens, 7);
        assert!(out.error.is_none());
    }

    #[test]
    fn openai_parser_assembles_indexed_tool_calls() {
        let mut p = OpenAiStreamParser::new();
        p.feed("data: {\"choices\":[{\"delta\":{\"content\":\"ok\",\"tool_calls\":[{\"index\":0,\"id\":\"c9\",\"function\":{\"name\":\"read_file\",\"arguments\":\"\"}}]}}]}\n\n");
        p.feed("data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"path\\\":\\\"b\\\"}\"}}]}}]}\n\n");
        p.feed("data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}],\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":9}}\n\n");
        p.feed("data: [DONE]\n\n");
        let out = p.finish();
        assert_eq!(out.text, "ok");
        assert_eq!(out.tool_uses.len(), 1);
        assert_eq!(out.tool_uses[0].id, "c9");
        assert_eq!(out.tool_uses[0].input_json, "{\"path\":\"b\"}");
        assert_eq!(out.input_tokens, 5);
        assert_eq!(out.output_tokens, 9);
    }

    #[test]
    fn responses_parser_assembles_text_call_and_usage() {
        let mut p = OpenAiResponsesParser::new();
        p.feed("event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"Hello\"}\n\n");
        p.feed("event: response.output_item.added\ndata: {\"type\":\"response.output_item.added\",\"output_index\":1,\"item\":{\"id\":\"it1\",\"type\":\"function_call\",\"call_id\":\"fc1\",\"name\":\"read_file\"}}\n\n");
        // Arguments split across feeds; item.done carries the authority.
        p.feed("event: response.function_call_arguments.delta\ndata: {\"type\":\"response.function_call_arguments.delta\",\"output_index\":1,\"delta\":\"{\\\"path\"");
        p.feed("\":\"b\\\"}}\n\n");
        p.feed("event: response.output_item.done\ndata: {\"type\":\"response.output_item.done\",\"output_index\":1,\"item\":{\"id\":\"it1\",\"type\":\"function_call\",\"call_id\":\"fc1\",\"name\":\"read_file\",\"arguments\":\"{\\\"path\\\":\\\"b\\\"}\"}}\n\n");
        p.feed("event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":11,\"output_tokens\":6}}}\n\n");
        let out = p.finish();
        assert_eq!(out.text, "Hello");
        assert_eq!(out.tool_uses.len(), 1);
        assert_eq!(out.tool_uses[0].id, "fc1");
        assert_eq!(out.tool_uses[0].name, "read_file");
        assert_eq!(out.tool_uses[0].input_json, "{\"path\":\"b\"}");
        assert_eq!(out.input_tokens, 11);
        assert_eq!(out.output_tokens, 6);
        assert!(out.error.is_none());
    }

    #[test]
    fn anthropic_search_result_blocks_count_and_scrape() {
        let mut p = AnthropicStreamParser::new();
        let block = serde_json::json!({
            "type": "web_search_tool_result",
            "tool_use_id": "srvtoolu_1",
            "content": [
                {"type": "web_search_result", "title": "Rust 1.92",
                 "url": "https://example.com/rust",
                 "content": [{"type": "text", "text": "stabilized let-chains"}]},
            ],
        });
        let start = serde_json::json!({
            "type": "content_block_start", "index": 2, "content_block": block,
        });
        p.feed(&format!("event: content_block_start\ndata: {start}\n\n"));
        p.feed(
            "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":2}\n\n",
        );
        let out = p.finish();
        assert_eq!(out.native_searches.len(), 1);
        assert_eq!(out.native_searches[0].provider, "anthropic");
        assert!(out.text.contains("Rust 1.92"), "got: {}", out.text);
        assert!(
            out.text.contains("stabilized let-chains"),
            "got: {}",
            out.text
        );
        assert!(out.tool_uses.is_empty());
    }

    #[test]
    fn responses_search_calls_count() {
        let mut p = OpenAiResponsesParser::new();
        p.feed("event: response.output_item.added\ndata: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"id\":\"ws1\",\"type\":\"web_search_call\"}}\n\n");
        p.feed("event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"found it\"}\n\n");
        let out = p.finish();
        assert_eq!(out.native_searches.len(), 1);
        assert_eq!(out.native_searches[0].provider, "openai");
        assert_eq!(out.text, "found it");
    }

    #[test]
    fn responses_parser_surfaces_failure() {
        let mut p = OpenAiResponsesParser::new();
        p.feed("event: response.failed\ndata: {\"type\":\"response.failed\",\"response\":{\"error\":{\"message\":\"quota\"}}}\n\n");
        assert_eq!(p.finish().error.as_deref(), Some("quota"));
    }

    #[test]
    fn parsers_surface_provider_errors() {
        let mut p = AnthropicStreamParser::new();
        p.feed(
            "event: error\ndata: {\"type\":\"error\",\"error\":{\"message\":\"overloaded\"}}\n\n",
        );
        assert_eq!(p.finish().error.as_deref(), Some("overloaded"));

        let mut q = OpenAiStreamParser::new();
        q.feed("data: {\"error\":{\"message\":\"bad key\",\"code\":401}}\n\n");
        assert_eq!(q.finish().error.as_deref(), Some("bad key"));
    }
}

#[cfg(test)]
mod computer_screenshot_tests {
    use super::*;
    #[test]
    fn canonical_screenshot_attachments_reach_every_direct_provider_format() {
        let messages = vec![serde_json::json!({"role":"user", "content":"Window screenshot", "attachments":[{"type":"image", "media_type":"image/png", "data":"AA=="}]})];
        let tools = serde_json::json!([]);
        let anthropic = anthropic_request_body("model", "", &messages, &tools, 4096, None, false);
        assert_eq!(anthropic["messages"][0]["content"][0]["source"]["data"], "AA==");
        let openai = openai_request_body("model", &messages, &tools, None);
        assert_eq!(openai["messages"][0]["content"][1]["image_url"]["url"], "data:image/png;base64,AA==");
        let responses = openai_responses_request_body("model", "", &messages, &tools, None, false);
        assert_eq!(responses["input"][0]["content"][1]["image_url"], "data:image/png;base64,AA==");
    }
}
