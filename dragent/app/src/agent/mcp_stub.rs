//! Wasm stub for the MCP (Model Context Protocol) client.
//!
//! MCP spawns local processes and opens network connections to tool servers —
//! none of which the web viewer does. This module mirrors the public API of the
//! native `mcp` module (path-swapped in `agent/mod.rs`) so every `crate::agent::
//! mcp::*` call site type-checks on wasm and reports "no servers, no tools".
#![allow(dead_code, unused_variables)]

use once_cell::sync::Lazy;
use std::sync::RwLock;

/// No-op stand-in for the native runtime manager. Never constructed on wasm
/// (`MCP_MANAGER` stays `None`), so its methods are unreachable.
pub struct McpRuntimeManager {
    _private: (),
}

impl McpRuntimeManager {
    pub fn remove_server(&mut self, _name: &str) {}
}

pub static MCP_MANAGER: Lazy<RwLock<Option<McpRuntimeManager>>> = Lazy::new(|| RwLock::new(None));

pub fn register_reconnect_notifier(_tx: async_channel::Sender<String>) {}

pub fn reinit_server(_name: &str, _on_done: Option<Box<dyn FnOnce() + Send + 'static>>) {}

pub fn is_mcp_oauth_callback(_url: &str) -> bool {
    false
}

pub fn handle_mcp_oauth_callback(_url: &str) {}

pub fn call_tool(_prefixed_name: &str, _args_json: &str) -> (String, bool) {
    ("MCP is unavailable in the web viewer".to_string(), false)
}

pub fn call_prompt(_prefixed_name: &str, _args_json: &str) -> (String, bool) {
    ("MCP is unavailable in the web viewer".to_string(), false)
}

pub fn call_resource(_prefixed_name: &str) -> (String, bool) {
    ("MCP is unavailable in the web viewer".to_string(), false)
}

pub fn tool_count_for(_server_name: &str) -> usize {
    0
}

pub fn prompt_count_for(_server_name: &str) -> usize {
    0
}

pub fn resource_count_for(_server_name: &str) -> usize {
    0
}

pub fn tools_for(_server_name: &str) -> Vec<String> {
    Vec::new()
}

pub fn prompts_for(_server_name: &str) -> Vec<String> {
    Vec::new()
}

pub fn resources_for(_server_name: &str) -> Vec<String> {
    Vec::new()
}

pub fn all_tool_defs() -> Vec<(String, String, String)> {
    Vec::new()
}

pub fn all_prompt_defs() -> Vec<(String, String, String)> {
    Vec::new()
}

pub fn all_resource_defs() -> Vec<(String, String, String)> {
    Vec::new()
}
