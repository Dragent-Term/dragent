//! GraphQL sign-in session and diagnostic query client.
//!
//! All HTTP calls go through `crate::api::API_CLIENT`, which handles Bearer
//! injection, token rotation on 401, and keychain persistence automatically.

use serde::Deserialize;
use std::sync::Arc;
use uuid::Uuid;

use super::types::{AgentAction, AgentQueryResponse, BlockAnalysis};
use super::{AgentApi, AgentError};
use crate::agent::api::session::AgentSessionContext;
use crate::agent::api::types::AgentQueryRequest;
use crate::agent::project_context::AnalyzeBlockRequest;

pub struct GraphqlAgentApi;

impl GraphqlAgentApi {
    pub fn new() -> Self {
        Self
    }
}

fn api_err(e: crate::api::ApiError) -> AgentError {
    match e {
        crate::api::ApiError::LocalMode => {
            AgentError::BackendUnavailable(crate::api::NEEDS_ACCOUNT.to_string())
        }
        crate::api::ApiError::NotAuthenticated => AgentError::BackendUnavailable(
            "Sign in required to perform self-healing diagnostics".to_string(),
        ),
        other => AgentError::BackendUnavailable(other.to_string()),
    }
}

impl AgentApi for GraphqlAgentApi {
    fn submit_query(&self, _request: AgentQueryRequest) -> Result<AgentQueryResponse, AgentError> {
        Err(AgentError::NotApplicable)
    }

    fn continue_conversation(
        &self,
        _conversation_id: Uuid,
        _tool_results: Vec<AgentAction>,
        _session: &AgentSessionContext,
    ) -> Result<AgentQueryResponse, AgentError> {
        Err(AgentError::NotApplicable)
    }

    fn agent_step(
        &self,
        request: super::types::AgentStepRequest,
    ) -> Result<super::types::AgentStepResponse, AgentError> {
        use super::types::{AgentRole, AgentStepResponse, AgentToolCall};

        // A model the user has their own key for is called straight from
        // here; their key never goes to Dragent's server.
        #[cfg(not(target_family = "wasm"))]
        if let Some(answer) = direct_agent_step(&request) {
            match answer {
                Ok(response) => return Ok(response),
                Err(error) if !crate::settings::AppSettings::load().allow_dragent_fallback => {
                    return Err(AgentError::Internal(error.to_string()))
                }
                Err(error) => {
                    log::warn!("[agent_step] direct call failed ({error}); trying Dragent's keys")
                }
            }
        }

        let query = r#"
            mutation AgentStep($input: AgentStepInput!) {
                agentStep(input: $input) {
                    toolCalls { id name argsJson }
                    content
                }
            }
        "#;

        let messages: Vec<serde_json::Value> = request
            .messages
            .iter()
            .map(|m| {
                let mut obj = serde_json::json!({
                    "role": match m.role {
                        AgentRole::User => "user",
                        AgentRole::Assistant => "assistant",
                        AgentRole::Tool => "tool",
                    },
                    "content": m.content,
                });
                if let Some(id) = &m.tool_call_id {
                    obj["toolCallId"] = serde_json::Value::String(id.clone());
                }
                if !m.tool_calls.is_empty() {
                    let calls_json = serde_json::to_string(&m.tool_calls).unwrap_or_default();
                    obj["toolCallsJson"] = serde_json::Value::String(calls_json);
                }
                obj
            })
            .collect();

        let tools: Vec<serde_json::Value> = request
            .tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "name": t.name,
                    "description": t.description,
                    "parametersJson": t.parameters_json,
                })
            })
            .collect();

        let variables = serde_json::json!({
            "input": {
                "modelName": request.model_name,
                "system": request.system,
                "messages": messages,
                "tools": tools,
            }
        });

        // Only whether Dragent's keys may serve it (see
        // AppSettings::request_key_headers): the user's own keys stay here.
        let settings = crate::settings::AppSettings::load();
        let extra_headers = settings
            .request_key_headers(&request.model_name)
            .map_err(crate::agent::api::AgentError::Internal)?;

        let extra_refs: Vec<(&str, &str)> = extra_headers
            .iter()
            .map(|(k, v)| (*k, v.as_str()))
            .collect();

        let body = crate::api::API_CLIENT
            .graphql_with_headers(query, variables, 60, &extra_refs)
            .map_err(api_err)?;

        if let Some(errs) = body.get("errors").filter(|v| !v.is_null()) {
            // Full technical blob → logs only. Surface just the server's message.
            log::warn!("[agent_step] GraphQL errors: {errs}");
            return Err(AgentError::Internal(first_graphql_message(errs)));
        }

        #[derive(Deserialize)]
        struct StepData {
            #[serde(rename = "agentStep")]
            agent_step: GqlAgentStepResult,
        }
        #[derive(Deserialize)]
        struct GqlAgentStepResult {
            #[serde(rename = "toolCalls")]
            tool_calls: Vec<GqlToolCall>,
            content: String,
        }
        #[derive(Deserialize)]
        struct GqlToolCall {
            id: String,
            name: String,
            #[serde(rename = "argsJson")]
            args_json: String,
        }

        let data: StepData =
            serde_json::from_value(body.get("data").cloned().unwrap_or(serde_json::Value::Null))
                .map_err(|e| AgentError::Internal(format!("parse agent_step response: {e}")))?;

        Ok(AgentStepResponse {
            tool_calls: data
                .agent_step
                .tool_calls
                .into_iter()
                .map(|tc| AgentToolCall {
                    id: tc.id,
                    name: tc.name,
                    args_json: tc.args_json,
                })
                .collect(),
            content: data.agent_step.content,
            // Token rotation is handled inside API_CLIENT — no need to propagate here.
            rotated_refresh_token: None,
            rotated_access_token: None,
            rotated_access_token_expires_in: None,
        })
    }

    fn analyze_block(&self, request: &AnalyzeBlockRequest) -> Result<BlockAnalysis, AgentError> {
        let block = &request.block;

        if !block.failed {
            return Err(AgentError::NotApplicable);
        }

        let query = r#"
            mutation ReportBlockCompleted($input: BlockCompletedInput!) {
                reportBlockCompleted(input: $input) {
                    id
                    explanation
                    suggestedCommand
                }
            }
        "#;

        let variables = serde_json::json!({
            "input": {
                "command": block.command,
                "cwd": block.cwd.to_string_lossy(),
                "exitCode": block.exit_code.unwrap_or(-1),
                "outputText": block.output_text,
                "modelName": "gpt-4o"
            }
        });

        let body = crate::api::API_CLIENT
            .graphql(query, variables, 12)
            .map_err(api_err)?;

        if let Some(errs) = body.get("errors").filter(|v| !v.is_null()) {
            log::warn!("[graphql] execution errors: {errs}");
            return Err(AgentError::Internal(first_graphql_message(errs)));
        }

        #[derive(Deserialize)]
        struct ReportData {
            #[serde(rename = "reportBlockCompleted")]
            report_block_completed: Option<GqlBlockAnalysis>,
        }
        #[derive(Deserialize)]
        struct GqlBlockAnalysis {
            explanation: String,
            #[serde(rename = "suggestedCommand")]
            suggested_command: Option<String>,
        }

        let data: ReportData =
            serde_json::from_value(body.get("data").cloned().unwrap_or(serde_json::Value::Null))
                .map_err(|e| {
                    AgentError::Internal(format!("Failed to parse analysis response: {e}"))
                })?;

        let analysis = data.report_block_completed.ok_or_else(|| {
            AgentError::Internal("GraphQL response contains empty analysis data".to_string())
        })?;

        Ok(BlockAnalysis {
            explanation: analysis.explanation,
            suggested_command: analysis.suggested_command,
            confidence: 1.0,
            // Token rotation is handled inside API_CLIENT — no need to propagate here.
            rotated_refresh_token: None,
            rotated_access_token: None,
            rotated_access_token_expires_in: None,
        })
    }
}

/// Pull the human-readable `message` out of a GraphQL errors array, so the UI
/// sees the server's actual error ("5-hour session limit reached: …") rather
/// than the raw `[{"locations":[{"line":3,"column":17}]}]` transport blob.
fn first_graphql_message(errs: &serde_json::Value) -> String {
    errs.as_array()
        .and_then(|a| a.first())
        .and_then(|e| e.get("message"))
        .and_then(|m| m.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "The request failed. Please try again.".to_string())
}

/// `request` straight to the provider when the user has their own key for
/// its model (see `pipeline::step_direct::direct_call`); `None` otherwise.
#[cfg(not(target_family = "wasm"))]
fn direct_agent_step(
    request: &super::types::AgentStepRequest,
) -> Option<Result<super::types::AgentStepResponse, crate::agent::pipeline::StreamError>> {
    use super::types::{AgentRole, AgentStepResponse, AgentToolCall};

    let messages: Vec<serde_json::Value> = request
        .messages
        .iter()
        .map(|m| match m.role {
            AgentRole::User => serde_json::json!({"role": "user", "content": m.content}),
            AgentRole::Assistant => {
                let calls: Vec<serde_json::Value> = m
                    .tool_calls
                    .iter()
                    .map(|call| serde_json::json!({"id": call.id, "name": call.name, "args": call.args_json}))
                    .collect();
                serde_json::json!({"role": "assistant", "content": m.content, "tool_calls": calls})
            }
            AgentRole::Tool => serde_json::json!({
                "role": "tool",
                "tool_call_id": m.tool_call_id,
                "content": m.content,
            }),
        })
        .collect();
    let tools: Vec<serde_json::Value> = request
        .tools
        .iter()
        .map(|tool| {
            serde_json::json!({
                "name": tool.name,
                "description": tool.description,
                "parameters": serde_json::from_str::<serde_json::Value>(&tool.parameters_json)
                    .unwrap_or_else(|_| serde_json::json!({"type": "object", "properties": {}})),
            })
        })
        .collect();
    let outcome = crate::agent::pipeline::step_direct::direct_call(
        &request.model_name,
        &request.system,
        &messages,
        &serde_json::Value::Array(tools),
    )?;
    Some(outcome.map(|outcome| {
        AgentStepResponse {
            tool_calls: outcome
                .tool_uses
                .into_iter()
                .map(|tool| AgentToolCall {
                    id: tool.id,
                    name: tool.name,
                    args_json: tool.input_json,
                })
                .collect(),
            content: outcome.text,
            rotated_refresh_token: None,
            rotated_access_token: None,
            rotated_access_token_expires_in: None,
        }
    }))
}
