//! Direct provider step transport (item M1): execute one provider round
//! straight from the local engine instead of POSTing it to Dragent cloud.
//!
//! Selection: a BYOK key configured for the turn's model provider (or a
//! keyless local server / custom endpoint) routes direct; everything else
//! keeps the existing cloud path byte-for-byte. Direct failures fall back
//! to cloud when the user's fallback toggle allows it — never the reverse,
//! and never in a loop (one fallback attempt, then surface).
//!
//! Privacy properties (load-bearing, see the architecture notes):
//! - The user's key travels only to the provider it belongs to (or nowhere,
//!   for local servers). Key headers bound for Dragent cloud are filtered
//!   out of direct runs.
//! - Responses-API calls set `store: false` (see the request builder).
//! - Nothing here persists prompts, keys, or streams; checkpoints keep
//!   their existing no-keys invariant.
//!
//! Native-only execution; on wasm every selection resolves to cloud.

use dragent_agent::direct::{
    anthropic_request_body, native_model_id, openai_request_body, openai_responses_request_body,
    route_for_provider, AnthropicStreamParser, CustomEndpointStyle, DirectKind,
    OpenAiResponsesParser, OpenAiStreamParser, StepParseOutcome,
};

use crate::agent::pipeline::StreamError;

/// Resolved direct-call parameters for one turn's provider. Constructed by
/// [`select_transport`]; `None` means the cloud path.
#[derive(Clone, Debug)]
pub struct DirectSpec {
    pub kind: DirectKind,
    /// Endpoint root, e.g. `https://api.anthropic.com` (no trailing slash).
    pub base_url: String,
    /// Credential for the provider. `None` = keyless local server.
    pub key: Option<String>,
    /// Provider-native model id (routing prefixes stripped).
    pub model: String,
    /// Pre-gated reasoning level (`low`/`medium`/`high`), if the provider
    /// vocabulary includes it for this model.
    pub effort: Option<String>,
    /// Declare provider-native search tools (Anthropic server-side search,
    /// Responses `web_search`). True only for first-party provider routes —
    /// never custom proxies (unknown backend) or local servers.
    pub native_search: bool,
    /// Extra headers (OpenRouter attribution). Never credentials.
    pub extra_headers: Vec<(String, String)>,
}

/// Choose the step transport for `model_name`.
///
/// Direct when the user configured their own credential for the model (BYOK),
/// a custom endpoint, or a keyless local server — in that order. Otherwise
/// cloud (current behavior, including paid platform-key runs until the thin
/// managed endpoint lands in M3).
pub fn select_transport(
    model_name: &str,
    settings: &crate::settings::AppSettings,
) -> Option<DirectSpec> {
    // Custom endpoints first: explicit user configuration wins.
    if let Some(custom) = settings
        .custom_models
        .iter()
        .find(|m| m.model_id == model_name)
    {
        return Some(DirectSpec {
            kind: custom.endpoint_style.direct_kind(),
            base_url: custom.base_url.trim_end_matches('/').to_string(),
            key: custom.api_key.clone().filter(|k| !k.trim().is_empty()),
            model: custom.model_id.clone(),
            // Unknown vocabulary on custom endpoints: omit effort rather
            // than risk a 400 on a user-typed URL. Same for native search:
            // only first-party routes declare provider tools.
            effort: None,
            native_search: false,
            extra_headers: Vec::new(),
        });
    }
    let provider_id = crate::settings::AppSettings::provider_id_for_model_static(model_name)?;
    let route = route_for_provider(&provider_id)?;
    // Keyless local servers: base-URL overrides ride the same settings
    // slots (`ollama_base_url`); no credential of any kind.
    if provider_id == "ollama" || provider_id == "lmstudio" {
        return Some(DirectSpec {
            kind: route.kind,
            base_url: settings
                .provider_keys
                .get(&provider_id)
                .map(|u| u.trim_end_matches('/').to_string())
                .filter(|u| !u.is_empty())
                .unwrap_or_else(|| route.base_url.to_string()),
            key: None,
            model: native_model_id(model_name).to_string(),
            effort: None,
            native_search: false,
            extra_headers: Vec::new(),
        });
    }
    // Everyone else needs their own key configured; without one the turn
    // stays on the cloud path (today's behavior, unchanged).
    let slot = route.key_slot?;
    let key = settings.provider_keys.get(slot).map(str::to_string)?;
    Some(DirectSpec {
        kind: if provider_id == "openai" {
            // GPT-5.6 family and higher: first-party tool use lives on the
            // Responses API. Chat fallback happens on transport failure.
            DirectKind::OpenAiResponses
        } else if provider_id == "meta" {
            // Muse Spark: Meta's flagship surface is Responses (reasoning
            // replay across turns). Same chat fallback on failure.
            DirectKind::OpenAiResponses
        } else {
            route.kind
        },
        base_url: route.base_url.to_string(),
        key: Some(key),
        model: native_model_id(model_name).to_string(),
        effort: effort_for_direct(&provider_id, model_name, settings),
        // Provider-native search is declared only on first-party routes
        // whose server-side search contract is verified above.
        native_search: provider_id == "anthropic" || provider_id == "openai",
        extra_headers: openrouter_headers(&provider_id),
    })
}

/// Reasoning level for direct calls, if the provider vocabulary includes it
/// for this model. `None` covers both "user left it on Auto" and "this
/// model rejects the parameter" — critically the gpt-5.6 class, which
/// rejects any level once function tools are present (and tools are always
/// present here).
fn effort_for_direct(
    provider_id: &str,
    model_name: &str,
    settings: &crate::settings::AppSettings,
) -> Option<String> {
    if !crate::settings::AgentEffort::model_supports(model_name) {
        return None;
    }
    let level = settings.selected_effort.wire_value()?;
    match provider_id {
        // Anthropic `output_config.effort`, OpenAI `reasoning_effort` /
        // Responses `reasoning.effort`, xAI `reasoning_effort`, Meta
        // Responses `reasoning.effort`. Every other provider: omit
        // (unknown vocabulary must never 400 a turn).
        "anthropic" | "openai" | "xai" | "meta" => Some(level.to_string()),
        _ => None,
    }
}

fn openrouter_headers(provider_id: &str) -> Vec<(String, String)> {
    if provider_id == "openrouter" {
        vec![
            (
                "HTTP-Referer".to_string(),
                "https://dragent.dev".to_string(),
            ),
            ("X-Title".to_string(), "Dragent".to_string()),
        ]
    } else {
        Vec::new()
    }
}

/// Tool definition for `fetch_url` in direct runs: same name and shape the
/// server advertises on the cloud path, executed locally by
/// `exec_fetch_url`. Large pages arrive truncated with a spill path, so the
/// description steers the model toward slicing rather than re-fetching.
fn fetch_url_tool_def() -> serde_json::Value {
    serde_json::json!({
        "name": "fetch_url",
        "description": "Fetch a URL and return its readable text content. Large pages are truncated; the result tells you how to read further slices. Prefer this over guessing URL contents.",
        "parameters": {
            "type": "object",
            "properties": {
                "url": { "type": "string", "description": "Full http(s) URL to fetch." },
            },
            "required": ["url"],
        },
    })
}

/// Augment the locally-assembled tool catalog for direct runs: append the
/// `fetch_url` definition the server would otherwise inject. `web_search`
/// stays out (see above).
fn with_direct_tool_defs(extra_tools: &serde_json::Value) -> serde_json::Value {
    let mut tools = extra_tools.clone();
    if let Some(arr) = tools.as_array_mut() {
        arr.push(fetch_url_tool_def());
    }
    tools
}

/// Request path (relative to [`DirectSpec::base_url`]) for the wire client.
pub fn step_path(kind: DirectKind) -> &'static str {
    match kind {
        DirectKind::Anthropic => "v1/messages",
        DirectKind::OpenAiCompatible => "chat/completions",
        DirectKind::OpenAiResponses => "responses",
    }
}

/// Map a provider HTTP failure onto [`StreamError`], mirroring the cloud
/// `map_task_start_error` contract: 401 → auth, 402 → credits, 429 →
/// rate-limited (retryable by the establishment retry), other 4xx → parse
/// (definitive, never retried), 5xx → network (retryable).
pub fn map_direct_http_error(status: u16, body: &str) -> StreamError {
    let server_msg = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            v.get("error")
                .and_then(|e| {
                    e.as_str().map(str::to_string).or_else(|| {
                        e.get("message")
                            .and_then(|m| m.as_str())
                            .map(str::to_string)
                    })
                })
                .or_else(|| {
                    v.get("message")
                        .and_then(|m| m.as_str())
                        .map(str::to_string)
                })
        })
        .unwrap_or_default();
    match status {
        // The person's own key, sent straight to the provider: a provider
        // problem, never a Dragent sign-in one.
        401 => StreamError::ProviderKey(if server_msg.is_empty() {
            "provider rejected the API key".to_string()
        } else {
            server_msg
        }),
        402 => StreamError::InsufficientCredits(if server_msg.is_empty() {
            "Provider reported insufficient credits.".to_string()
        } else {
            server_msg
        }),
        429 => StreamError::RateLimited(if server_msg.is_empty() {
            "Provider rate limit reached.".to_string()
        } else {
            server_msg
        }),
        400 | 403 | 404 | 422 => StreamError::Parse(format!(
            "Provider rejected the direct request (HTTP {status}): {}",
            truncate_one_line(&server_msg, 300)
        )),
        _ => StreamError::Network(format!(
            "Provider request failed (HTTP {status}): {}",
            truncate_one_line(&server_msg, 300)
        )),
    }
}

fn truncate_one_line(s: &str, max: usize) -> String {
    let flat: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.len() <= max {
        flat
    } else {
        format!("{}…", crate::agent::tools_background::utf8_prefix(&flat, max))
    }
}

/// Execute one direct provider step: POST the translated request and parse
/// the streamed response into a step outcome.
///
/// Round-buffered by design (v1): unlike the cloud path, intra-round text
/// deltas are not forwarded live — the turn progresses identically, but the
/// UI paints each round on completion rather than token-by-token. Closing
/// that liveliness gap is a pure parser addition (incremental drain), no
/// protocol change, tracked as fast-follow.
///
/// Native-only. Returns the assembled outcome; transport-level failures are
/// `StreamError` for the retry/fallback policy to classify. The 90s read
/// timeout bounds wire silence (tighter than the controller's 120s
/// inactivity trip), so no keepalive traffic is needed.
#[cfg(not(target_family = "wasm"))]
pub fn run_direct_step(
    spec: &DirectSpec,
    system: &str,
    messages: &[serde_json::Value],
    extra_tools: &serde_json::Value,
) -> Result<StepParseOutcome, StreamError> {
    // Direct runs have no server to inject its tool definitions, so the
    // locally-executable web tool is advertised here. `web_search` is
    // deliberately NOT advertised: with no user search key configured there
    // is no backend to serve it, and an uncallable tool only invites
    // hallucinated calls (opencode handles this the same way — exclusion,
    // not an error path). The dispatch-side graceful error stays as a
    // safety net for hallucinated calls.
    run_direct_request(spec, system, messages, &with_direct_tool_defs(extra_tools))
}

/// One model call for the app's small side tasks (naming a workspace,
/// fixing a failed command), straight to the provider with the user's own
/// key. `None` when the model has no such route (no key of the user's for
/// it): the caller goes through Dragent's server, which uses Dragent's keys.
/// `tools` are offered as given (no web tool added).
#[cfg(not(target_family = "wasm"))]
pub fn direct_call(
    model_name: &str,
    system: &str,
    messages: &[serde_json::Value],
    tools: &serde_json::Value,
) -> Option<Result<StepParseOutcome, StreamError>> {
    let settings = crate::settings::AppSettings::load();
    let spec = select_transport(model_name, &settings)?;
    Some(run_direct_request(&spec, system, messages, tools))
}

/// Send one step to the provider with exactly `extra_tools` offered.
#[cfg(not(target_family = "wasm"))]
fn run_direct_request(
    spec: &DirectSpec,
    system: &str,
    messages: &[serde_json::Value],
    extra_tools: &serde_json::Value,
) -> Result<StepParseOutcome, StreamError> {
    let body = match spec.kind {
        DirectKind::Anthropic => anthropic_request_body(
            &spec.model,
            system,
            messages,
            extra_tools,
            16_384,
            spec.effort.as_deref(),
            spec.native_search,
        ),
        DirectKind::OpenAiCompatible => {
            openai_request_body(&spec.model, messages, extra_tools, spec.effort.as_deref())
        }
        DirectKind::OpenAiResponses => openai_responses_request_body(
            &spec.model,
            system,
            messages,
            extra_tools,
            spec.effort.as_deref(),
            spec.native_search,
        ),
    };
    let url = format!(
        "{}/{}",
        spec.base_url.trim_end_matches('/'),
        step_path(spec.kind)
    );
    let agent = ureq::builder()
        .timeout_connect(std::time::Duration::from_secs(10))
        .timeout_read(std::time::Duration::from_secs(90))
        .build();
    let mut req = agent
        .post(&url)
        .set("Content-Type", "application/json")
        .set("Accept", "text/event-stream")
        .set("Cache-Control", "no-cache");
    match spec.kind {
        DirectKind::Anthropic => {
            let key = spec
                .key
                .as_deref()
                .ok_or_else(|| StreamError::ProviderKey("no Anthropic API key configured".to_string()))?;
            req = req
                .set("x-api-key", key)
                .set("anthropic-version", "2023-06-01");
        }
        _ => {
            if let Some(key) = spec.key.as_deref() {
                req = req.set("Authorization", &format!("Bearer {key}"));
            }
        }
    }
    for (name, value) in &spec.extra_headers {
        req = req.set(name, value);
    }
    let response = match req.send_json(&body) {
        Ok(resp) => resp,
        Err(ureq::Error::Status(code, resp)) => {
            let text = resp.into_string().unwrap_or_default();
            return Err(map_direct_http_error(code, &text));
        }
        Err(ureq::Error::Transport(t)) => {
            return Err(StreamError::Network(format!(
                "direct provider transport: {t}"
            )));
        }
    };
    stream_provider_response(spec.kind, response)
}

/// Drain a provider SSE response through the matching parser.
#[cfg(not(target_family = "wasm"))]
fn stream_provider_response(
    kind: DirectKind,
    response: ureq::Response,
) -> Result<StepParseOutcome, StreamError> {
    use std::io::Read;
    let mut text = String::new();
    response
        .into_reader()
        .read_to_string(&mut text)
        .map_err(|e| StreamError::Network(format!("direct provider stream read: {e}")))?;
    // Round-buffered (see `run_direct_step`): one feed, then finish. The
    // parsers are chunk-tolerant, so this is equivalent to incremental
    // feeding minus live forwarding.
    let mut outcome = match kind {
        DirectKind::Anthropic => {
            let mut p = AnthropicStreamParser::new();
            p.feed(&text);
            p.finish()
        }
        DirectKind::OpenAiCompatible => {
            let mut p = OpenAiStreamParser::new();
            p.feed(&text);
            p.finish()
        }
        DirectKind::OpenAiResponses => {
            let mut p = OpenAiResponsesParser::new();
            p.feed(&text);
            p.finish()
        }
    };
    if let Some(error) = outcome.error.take() {
        return Err(StreamError::Network(format!(
            "provider step error: {error}"
        )));
    }
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_line_preview_cuts_on_a_character() {
        // "🙂" spans bytes 2..6, so a 4-byte cut lands inside it.
        assert_eq!(truncate_one_line("ab🙂cd", 4), "ab…");
        assert_eq!(truncate_one_line("ab🙂cd", 6), "ab🙂…");
    }

    fn settings_with(key_slot: &str, key: &str) -> crate::settings::AppSettings {
        let mut settings = crate::settings::AppSettings::default();
        settings.provider_keys.set(key_slot, key.to_string());
        settings
    }

    #[test]
    fn selects_direct_for_configured_providers() {
        let settings = settings_with("anthropic", "sk-ant-x");
        let spec = select_transport("claude-sonnet-4-6", &settings).expect("direct");
        assert_eq!(spec.kind, DirectKind::Anthropic);
        assert_eq!(spec.model, "claude-sonnet-4-6");
        assert!(spec.base_url.contains("anthropic.com"));
        // First-party Anthropic declares native search; custom/local never do.
        assert!(spec.native_search);

        let settings = settings_with("openai", "sk-x");
        let spec = select_transport("gpt-5.6-luna", &settings).expect("direct");
        // First-party OpenAI rides Responses (5.6-family tool support).
        assert_eq!(spec.kind, DirectKind::OpenAiResponses);
        // 5.6-class rejects effort with tools: must be omitted.
        assert_eq!(spec.effort, None);

        let settings = settings_with("groq", "gsk-x");
        let spec = select_transport("groq/llama-3.3-70b-versatile", &settings).expect("direct");
        assert_eq!(spec.kind, DirectKind::OpenAiCompatible);
        assert_eq!(spec.model, "llama-3.3-70b-versatile");
    }

    #[test]
    fn selects_cloud_without_a_key() {
        let settings = crate::settings::AppSettings::default();
        assert!(select_transport("claude-sonnet-4-6", &settings).is_none());
        assert!(select_transport("gpt-5", &settings).is_none());
        // Unknown models stay cloud.
        let settings = settings_with("anthropic", "sk-ant-x");
        assert!(select_transport("mystery-model-9", &settings).is_none());
    }

    #[test]
    fn local_servers_need_no_key() {
        let settings = crate::settings::AppSettings::default();
        let spec = select_transport("ollama/qwen3", &settings).expect("direct");
        assert_eq!(spec.key, None);
        assert!(spec.base_url.contains("11434"));
    }

    #[test]
    fn custom_endpoints_route_by_style() {
        let mut settings = crate::settings::AppSettings::default();
        settings.custom_models.push(crate::settings::CustomModel {
            name: "c".into(),
            base_url: "https://corp.example/v1/".into(),
            model_id: "corp-model".into(),
            api_key: Some("k".into()),
            endpoint_style: CustomEndpointStyle::Responses,
        });
        let spec = select_transport("corp-model", &settings).expect("direct");
        assert_eq!(spec.kind, DirectKind::OpenAiResponses);
        assert_eq!(spec.base_url, "https://corp.example/v1");
        assert!(!spec.native_search);
    }

    #[test]
    fn http_errors_map_like_the_cloud_contract() {
        assert!(matches!(
            map_direct_http_error(401, "bad"),
            StreamError::ProviderKey(_)
        ));
        assert!(matches!(
            map_direct_http_error(429, "slow"),
            StreamError::RateLimited(_)
        ));
        // Definitive client errors must not retry.
        assert!(matches!(
            map_direct_http_error(404, "no such model"),
            StreamError::Parse(_)
        ));
        assert!(matches!(
            map_direct_http_error(500, "boom"),
            StreamError::Network(_)
        ));
    }

    #[test]
    fn direct_catalog_advertises_fetch_but_not_search() {
        let base = serde_json::json!([
            {"name": "read_file", "description": "r", "parameters": {"type": "object"}},
        ]);
        let augmented = with_direct_tool_defs(&base);
        let names: Vec<&str> = augmented
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t.get("name").and_then(|n| n.as_str()))
            .collect();
        assert!(names.contains(&"read_file"));
        assert!(names.contains(&"fetch_url"));
        assert!(!names.contains(&"web_search"));
    }

    #[test]
    fn step_paths_are_correct() {
        assert_eq!(step_path(DirectKind::Anthropic), "v1/messages");
        assert_eq!(step_path(DirectKind::OpenAiCompatible), "chat/completions");
        assert_eq!(step_path(DirectKind::OpenAiResponses), "responses");
    }
}
