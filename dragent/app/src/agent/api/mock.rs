use uuid::Uuid;

use super::types::{AgentAction, AgentActionKind, AgentQueryResponse, BlockAnalysis};
use super::{AgentApi, AgentError};
use crate::agent::api::session::AgentSessionContext;
use crate::agent::api::types::AgentQueryRequest;
use crate::agent::project_context::AnalyzeBlockRequest;
use crate::agent::tools::ToolResultKind;
use std::path::PathBuf;

pub struct MockAgentApi;

impl MockAgentApi {
    pub fn new() -> Self {
        Self
    }

    /// Stub continuation after a tool result.  Returns a terminal `Done` response.
    /// (Full continue_conversation lands in Layer 10; this satisfies Layer 7 tests.)
    pub fn continue_with_tool_result(&self, _result: &ToolResultKind) -> AgentQueryResponse {
        AgentQueryResponse {
            conversation_id: Uuid::new_v4(),
            actions: vec![
                AgentAction::message("Read files, analysis complete."),
                AgentAction::done(),
            ],
        }
    }
}

impl AgentApi for MockAgentApi {
    fn continue_conversation(
        &self,
        conversation_id: Uuid,
        tool_results: Vec<AgentAction>,
        _session: &AgentSessionContext,
    ) -> Result<AgentQueryResponse, AgentError> {
        // Simple scripted multi-round conversation support for mock
        let mut text = format!(
            "Received {} tool results. Continuing...",
            tool_results.len()
        );
        for res in tool_results {
            match res.kind {
                AgentActionKind::ReadFiles { .. } => {
                    text = "Read files successfully. Recommending command to run:".to_string();
                }
                AgentActionKind::Grep { .. } => {
                    text = "Grep matched pattern. Recommending command to run:".to_string();
                }
                AgentActionKind::SearchCodebase { .. } => {
                    text = "Codebase search found matching files. Recommending command to run:"
                        .to_string();
                }
                _ => {}
            }
        }
        Ok(AgentQueryResponse {
            conversation_id,
            actions: vec![
                AgentAction::message(text),
                AgentAction::run_command("cargo build", "Recompile after check"),
                AgentAction::done(),
            ],
        })
    }
    fn submit_query(&self, request: AgentQueryRequest) -> Result<AgentQueryResponse, AgentError> {
        let q = request.query.to_lowercase();
        let last_failed = request.session.last_failed_block().cloned();

        let actions = if q.contains("deploy") || q.contains("setup") || q.contains("install") {
            vec![
                AgentAction::message("Sure, I'll help you with that. Here's the command to run:"),
                AgentAction::run_command(
                    "echo 'mock deploy step'",
                    format!("Executing deploy step for: {}", request.query),
                ),
                AgentAction::done(),
            ]
        } else if q.contains("explain") || q.contains("why") || q.contains("what") {
            let text = match last_failed {
                Some(ref b) => format!(
                    "The last command `{}` failed (exit {}). {}",
                    b.command,
                    b.exit_code.unwrap_or(-1),
                    if b.output_text.is_empty() {
                        "No output captured.".to_string()
                    } else {
                        format!(
                            "Output: {}",
                            b.output_text.lines().take(3).collect::<Vec<_>>().join(" ")
                        )
                    }
                ),
                None => "No recent failures found. Everything looks good!".to_string(),
            };
            vec![AgentAction::message(text), AgentAction::done()]
        } else if q == "fix" || q == "fix it" || q.starts_with("fix ") {
            match last_failed {
                Some(ref b) => {
                    let fix_cmd = if b.command.contains("cargo") {
                        "cargo build 2>&1 | head -50"
                    } else {
                        "echo 're-run with verbose flags'"
                    };
                    vec![
                        AgentAction::message(format!(
                            "I'll try to fix the failure in `{}`.",
                            b.command
                        )),
                        AgentAction::run_command(fix_cmd, "Attempting to fix the last error"),
                        AgentAction::done(),
                    ]
                }
                None => vec![
                    AgentAction::message(
                        "I don't see a recent failure to fix. What would you like me to fix?",
                    ),
                    AgentAction::done(),
                ],
            }
        } else if q.contains("find") || q.contains("search") || q.contains("where is") {
            let repo_root = request
                .session
                .project
                .as_ref()
                .and_then(|p| p.project_root.clone())
                .unwrap_or_else(|| request.session.cwd.clone());
            vec![AgentAction::search_codebase(
                request.query.clone(),
                repo_root,
            )]
        } else {
            vec![
                AgentAction::message("Mock agent received your query."),
                AgentAction::done(),
            ]
        };

        Ok(AgentQueryResponse {
            conversation_id: Uuid::new_v4(),
            actions,
        })
    }

    fn agent_step(
        &self,
        request: super::types::AgentStepRequest,
    ) -> Result<super::types::AgentStepResponse, AgentError> {
        use super::types::{AgentRole, AgentStepResponse, AgentToolCall};
        // In mock mode: if any tools are available and messages don't yet have tool results,
        // call read_file on the first tool available. Otherwise return a final answer.
        let has_tool_result = request.messages.iter().any(|m| m.role == AgentRole::Tool);
        if !has_tool_result && !request.tools.is_empty() {
            Ok(AgentStepResponse {
                tool_calls: vec![AgentToolCall {
                    id: "mock_call_1".to_string(),
                    name: "read_file".to_string(),
                    args_json: r#"{"path":"src/main.rs"}"#.to_string(),
                }],
                content: String::new(),
                rotated_refresh_token: None,
                rotated_access_token: None,
                rotated_access_token_expires_in: None,
            })
        } else {
            Ok(AgentStepResponse {
                tool_calls: vec![],
                content: "I've analyzed the error. The issue is in your source code. Try running `cargo build 2>&1 | head -50` to see the full error output.".to_string(),
                rotated_refresh_token: None,
                rotated_access_token: None,
                rotated_access_token_expires_in: None,
            })
        }
    }

    fn analyze_block(&self, request: &AnalyzeBlockRequest) -> Result<BlockAnalysis, AgentError> {
        let block = &request.block;
        let project = &request.project;

        if !block.failed {
            return Err(AgentError::NotApplicable);
        }

        // Build a project-aware suffix for the explanation.
        let mut ctx_parts: Vec<String> = Vec::new();
        if let Some(branch) = &project.git_branch {
            ctx_parts.push(format!("branch `{branch}`"));
        }
        if project.git_changed_files > 0 || project.git_untracked_files > 0 {
            ctx_parts.push(format!(
                "{} changed, {} untracked",
                project.git_changed_files, project.git_untracked_files
            ));
        }
        if !project.referenced_paths.is_empty() {
            let files: Vec<String> = project
                .referenced_paths
                .iter()
                .take(3)
                .map(|p| p.display().to_string())
                .collect();
            ctx_parts.push(format!("relevant files: {}", files.join(", ")));
        }
        let ctx_suffix = if ctx_parts.is_empty() {
            String::new()
        } else {
            format!(" ({})", ctx_parts.join("; "))
        };

        // Build a file-context suffix when file contents are available.
        let file_suffix = if let Some(fc) = &request.file_context {
            let snippets: Vec<String> = fc
                .files
                .iter()
                .filter_map(|f| {
                    f.content.as_deref().map(|text| {
                        let preview: String = text.lines().take(5).collect::<Vec<_>>().join(" | ");
                        format!("{}: {}", f.path.display(), preview)
                    })
                })
                .take(3)
                .collect();
            if snippets.is_empty() {
                String::new()
            } else {
                format!(" [files: {}]", snippets.join("; "))
            }
        } else {
            String::new()
        };

        if block.command.contains("cargo") {
            Ok(BlockAnalysis {
                explanation: format!(
                    "The cargo command `{}` exited with code {}{ctx_suffix}{file_suffix}. \
                     Check the output above for compiler errors.",
                    block.command,
                    block.exit_code.unwrap_or(-1),
                ),
                suggested_command: Some("cargo build 2>&1 | head -50".to_string()),
                confidence: 0.85,
                rotated_refresh_token: None,
                rotated_access_token: None,
                rotated_access_token_expires_in: None,
            })
        } else {
            Ok(BlockAnalysis {
                explanation: format!(
                    "Command `{}` failed with exit code {}{ctx_suffix}{file_suffix}.",
                    block.command,
                    block.exit_code.unwrap_or(-1),
                ),
                suggested_command: Some("echo 're-run with verbose flags'".to_string()),
                confidence: 0.6,
                rotated_refresh_token: None,
                rotated_access_token: None,
                rotated_access_token_expires_in: None,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::agent::api::session::{AgentSessionContext, BlockContextSnapshot};
    use crate::agent::project_context::{AnalyzeBlockRequest, ProjectContext};

    fn ctx_with_failed(cmd: &str) -> AgentSessionContext {
        AgentSessionContext {
            cwd: PathBuf::from("/tmp"),
            recent_blocks: vec![BlockContextSnapshot {
                block_id: 1,
                command: cmd.to_string(),
                cwd: PathBuf::from("/tmp"),
                exit_code: Some(1),
                duration_ms: None,
                output_text: "error: could not compile".to_string(),
                output_truncated: false,
                failed: true,
            }],
            is_autodetected: true,
            project: None,
            timestamp: chrono::Utc::now(),
            os: "macos".to_string(),
            shell: "zsh".to_string(),
            codebases: Vec::new(),
            attached_blocks: Vec::new(),
            skills: Vec::new(),
        }
    }

    fn ctx_empty() -> AgentSessionContext {
        AgentSessionContext {
            cwd: PathBuf::from("/tmp"),
            recent_blocks: vec![],
            is_autodetected: true,
            project: None,
            timestamp: chrono::Utc::now(),
            os: "macos".to_string(),
            shell: "zsh".to_string(),
            codebases: Vec::new(),
            attached_blocks: Vec::new(),
            skills: Vec::new(),
        }
    }

    fn analyze_request(snap: BlockContextSnapshot) -> AnalyzeBlockRequest {
        AnalyzeBlockRequest {
            block: snap,
            project: ProjectContext::default(),
            file_context: None,
            refresh_token: None,
            access_token: None,
        }
    }

    // Implement mock AgentApi trait for test helper if needed.
    struct TestMockApi;
    impl AgentApi for TestMockApi {
        fn submit_query(
            &self,
            request: AgentQueryRequest,
        ) -> Result<AgentQueryResponse, AgentError> {
            MockAgentApi::new().submit_query(request)
        }
        fn analyze_block(
            &self,
            request: &AnalyzeBlockRequest,
        ) -> Result<BlockAnalysis, AgentError> {
            MockAgentApi::new().analyze_block(request)
        }
        fn continue_conversation(
            &self,
            conversation_id: Uuid,
            tool_results: Vec<AgentAction>,
            session: &AgentSessionContext,
        ) -> Result<AgentQueryResponse, AgentError> {
            MockAgentApi::new().continue_conversation(conversation_id, tool_results, session)
        }
        fn agent_step(
            &self,
            request: crate::agent::api::types::AgentStepRequest,
        ) -> Result<crate::agent::api::types::AgentStepResponse, AgentError> {
            MockAgentApi::new().agent_step(request)
        }
    }

    fn query(q: &str, session: AgentSessionContext) -> AgentQueryRequest {
        AgentQueryRequest {
            query: q.to_string(),
            session,
        }
    }

    fn action_kinds(resp: &AgentQueryResponse) -> Vec<String> {
        resp.actions
            .iter()
            .map(|a| match &a.kind {
                crate::agent::api::types::AgentActionKind::Message { .. } => "Message".to_string(),
                crate::agent::api::types::AgentActionKind::RunCommand { .. } => {
                    "RunCommand".to_string()
                }
                crate::agent::api::types::AgentActionKind::Done => "Done".to_string(),
                crate::agent::api::types::AgentActionKind::ReadFiles { .. } => {
                    "ReadFiles".to_string()
                }
                crate::agent::api::types::AgentActionKind::ListDirectory { .. } => {
                    "ListDirectory".to_string()
                }
                crate::agent::api::types::AgentActionKind::Grep { .. } => "Grep".to_string(),
                crate::agent::api::types::AgentActionKind::SearchCodebase { .. } => {
                    "SearchCodebase".to_string()
                }
                crate::agent::api::types::AgentActionKind::WriteFile { .. } => {
                    "WriteFile".to_string()
                }
                crate::agent::api::types::AgentActionKind::EditFile { .. } => {
                    "EditFile".to_string()
                }
            })
            .collect()
    }

    #[test]
    fn deploy_returns_run_command() {
        let api = MockAgentApi::new();
        let resp = api
            .submit_query(query("deploy my app", ctx_empty()))
            .unwrap();
        assert!(action_kinds(&resp).contains(&"RunCommand".to_string()));
    }

    #[test]
    fn setup_returns_run_command() {
        let api = MockAgentApi::new();
        let resp = api
            .submit_query(query("setup the project", ctx_empty()))
            .unwrap();
        assert!(action_kinds(&resp).contains(&"RunCommand".to_string()));
    }

    #[test]
    fn explain_returns_message_only() {
        let api = MockAgentApi::new();
        let resp = api
            .submit_query(query("explain this error", ctx_empty()))
            .unwrap();
        assert!(!action_kinds(&resp).contains(&"RunCommand".to_string()));
        assert!(action_kinds(&resp).contains(&"Message".to_string()));
    }

    #[test]
    fn what_returns_message_only() {
        let api = MockAgentApi::new();
        let resp = api
            .submit_query(query("what went wrong", ctx_empty()))
            .unwrap();
        assert!(!action_kinds(&resp).contains(&"RunCommand".to_string()));
    }

    #[test]
    fn fix_with_failed_block_gives_run_command() {
        let api = MockAgentApi::new();
        let resp = api
            .submit_query(query("fix it", ctx_with_failed("cargo build")))
            .unwrap();
        assert!(action_kinds(&resp).contains(&"RunCommand".to_string()));
    }

    #[test]
    fn fix_without_failure_gives_message() {
        let api = MockAgentApi::new();
        let resp = api
            .submit_query(query("fix the problem", ctx_empty()))
            .unwrap();
        assert!(!action_kinds(&resp).contains(&"RunCommand".to_string()));
    }

    #[test]
    fn default_fallback_message() {
        let api = MockAgentApi::new();
        let resp = api.submit_query(query("hello there", ctx_empty())).unwrap();
        assert!(action_kinds(&resp).contains(&"Message".to_string()));
    }

    #[test]
    fn every_response_gets_unique_conversation_id() {
        let api = MockAgentApi::new();
        let r1 = api.submit_query(query("hello", ctx_empty())).unwrap();
        let r2 = api.submit_query(query("hello", ctx_empty())).unwrap();
        assert_ne!(r1.conversation_id, r2.conversation_id);
    }

    #[test]
    fn analyze_success_block_is_not_applicable() {
        let api = MockAgentApi::new();
        let snap = BlockContextSnapshot {
            block_id: 1,
            command: "echo hi".to_string(),
            cwd: PathBuf::from("/tmp"),
            exit_code: Some(0),
            duration_ms: None,
            output_text: String::new(),
            output_truncated: false,
            failed: false,
        };
        assert_eq!(
            api.analyze_block(&analyze_request(snap)),
            Err(AgentError::NotApplicable)
        );
    }

    #[test]
    fn analyze_failed_cargo_suggests_command() {
        let api = MockAgentApi::new();
        let snap = BlockContextSnapshot {
            block_id: 2,
            command: "cargo build".to_string(),
            cwd: PathBuf::from("/tmp"),
            exit_code: Some(1),
            duration_ms: None,
            output_text: "error: aborting due to previous error".to_string(),
            output_truncated: false,
            failed: true,
        };
        let analysis = api.analyze_block(&analyze_request(snap)).unwrap();
        assert!(!analysis.explanation.is_empty());
        assert!(analysis.suggested_command.is_some());
        assert!(analysis.confidence > 0.8);
    }

    #[test]
    fn analyze_generic_failure_lower_confidence() {
        let api = MockAgentApi::new();
        let snap = BlockContextSnapshot {
            block_id: 3,
            command: "make build".to_string(),
            cwd: PathBuf::from("/tmp"),
            exit_code: Some(2),
            duration_ms: None,
            output_text: String::new(),
            output_truncated: false,
            failed: true,
        };
        let analysis = api.analyze_block(&analyze_request(snap)).unwrap();
        assert!(analysis.confidence < 0.8);
        assert!(analysis.suggested_command.is_some());
    }

    #[test]
    fn analyze_explanation_includes_project_context() {
        let api = MockAgentApi::new();
        let snap = BlockContextSnapshot {
            block_id: 4,
            command: "cargo build".to_string(),
            cwd: PathBuf::from("/tmp"),
            exit_code: Some(1),
            duration_ms: None,
            output_text: "  --> src/main.rs:5:1\nerror[E0308]: mismatched types".to_string(),
            output_truncated: false,
            failed: true,
        };
        let project = ProjectContext::build(
            &PathBuf::from("/tmp"),
            Some(dragent_agent::project::GitInfo {
                branch: "feature-branch".to_string(),
                changed_files: 2,
                untracked_files: 1,
            }),
            &snap.output_text,
        );
        let request = AnalyzeBlockRequest {
            block: snap,
            project,
            file_context: None,
            refresh_token: None,
            access_token: None,
        };
        let analysis = api.analyze_block(&request).unwrap();
        assert!(
            analysis.explanation.contains("feature-branch"),
            "expected branch in explanation: {}",
            analysis.explanation
        );
        assert!(
            analysis.explanation.contains("src/main.rs"),
            "expected file path in explanation: {}",
            analysis.explanation
        );
    }
}
