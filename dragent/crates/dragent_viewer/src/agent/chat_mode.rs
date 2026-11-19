//! Agent execution modes for the input bar — the Cursor-style Agent / Plan /
//! Debug / Multitask / Ask selector.
//!
//! Each mode is a posture the agent adopts for a turn. It maps onto the
//! controller's existing levers — the agent `role` (for parallel delegation)
//! and the plan tool-gate — plus a mode-specific system-prompt section. Shell
//! is intentionally NOT here: it stays a separate input toggle.

use crate::agent::controller::AgentRole;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ChatMode {
    /// Autonomous multi-step execution with full tools. (Cursor "Agent".)
    #[default]
    Agent,
    /// Research + a reviewable plan before any edits. (Cursor "Plan".)
    Plan,
    /// Reproduce → instrument with runtime logs → root-cause → fix → verify.
    /// (Cursor 2.2 "Debug".)
    Debug,
    /// Fan out independent subtasks to parallel Worker sub-agents.
    /// (Cursor 2.0 "/multitask" parallel agents — see notes below.)
    Multitask,
    /// Read-only Q&A: answer from the codebase, never edit or run mutations.
    /// (Cursor "Ask".)
    Ask,
}

impl ChatMode {
    pub const ALL: [ChatMode; 5] = [
        ChatMode::Agent,
        ChatMode::Plan,
        ChatMode::Debug,
        ChatMode::Multitask,
        ChatMode::Ask,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ChatMode::Agent => "Agent",
            ChatMode::Plan => "Plan",
            ChatMode::Debug => "Debug",
            ChatMode::Multitask => "Multitask",
            ChatMode::Ask => "Ask",
        }
    }

    /// One-line description shown in the dropdown.
    pub fn subtitle(self) -> &'static str {
        match self {
            ChatMode::Agent => "Autonomous multi-step execution",
            ChatMode::Plan => "Research and a reviewable plan first",
            ChatMode::Debug => "Reproduce, instrument, and root-cause bugs",
            ChatMode::Multitask => "Fan out parallel sub-agents",
            ChatMode::Ask => "Read-only questions and answers",
        }
    }

    /// The agent role this mode runs under. Multitask uses the Coordinator role
    /// (plan-and-delegate to parallel Workers); everything else runs Default.
    pub fn role(self) -> AgentRole {
        match self {
            ChatMode::Multitask => AgentRole::Coordinator,
            _ => AgentRole::Default,
        }
    }

    /// Whether write/exec tools are gated behind an approved plan for this mode.
    pub fn gate_tools(self) -> bool {
        matches!(self, ChatMode::Plan)
    }

    /// Whether mutating tools are hard-refused (Ask is strictly read-only).
    pub fn read_only(self) -> bool {
        matches!(self, ChatMode::Ask)
    }

    /// Mode-specific system-prompt section, appended to the dynamic prompt.
    /// Empty for Agent (the default posture) and Multitask (its behavior comes
    /// from the Coordinator role's own prompt section).
    pub fn system_prompt_suffix(self) -> &'static str {
        match self {
            ChatMode::Agent | ChatMode::Multitask => "",
            ChatMode::Plan => {
                "\n\n\
## Plan mode

Do NOT edit files or run mutating commands yet. First research the codebase \
(read_file, grep, search_codebase, list_directory), ask any genuinely blocking \
clarifying questions via ask_user, then present a concise, reviewable plan of \
the concrete changes you will make. Wait for the user to approve the plan \
before writing anything."
            }
            ChatMode::Debug => {
                "\n\n\
## Debug mode

You are hunting a specific bug. Work empirically, not from assumption:
1. **Reproduce** — establish the exact failing behavior and a way to trigger it.
2. **Instrument** — add targeted runtime logging/asserts around the suspect path \
   (and run it) to observe actual values, control flow, and state. Prefer \
   evidence from a real run over reasoning about what the code 'should' do.
3. **Isolate the root cause** — narrow to the smallest failing unit; confirm the \
   cause with the logs before proposing a fix.
4. **Fix minimally** — change only what the root cause requires.
5. **Verify** — re-run to confirm the bug is gone, then remove scaffolding logs \
   you added (unless they are genuinely useful to keep).
State your current step as you go so the user can follow the investigation."
            }
            ChatMode::Ask => {
                "\n\n\
## Ask mode (read-only)

This is a READ-ONLY question-and-answer turn. Do NOT edit files, and do NOT run \
commands that change state (no writes, installs, migrations, git mutations, \
servers). Use only read-only tools — read_file, grep, search_codebase, \
list_directory, and read-only shell (ls, cat, git status/log/diff). Answer the \
user's question directly and cite the specific files/lines you relied on. If the \
answer would require making changes, explain what you WOULD do instead of doing \
it, and suggest switching to Agent mode."
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_plan_gates_tools() {
        for m in ChatMode::ALL {
            assert_eq!(
                m.gate_tools(),
                m == ChatMode::Plan,
                "gate mismatch for {m:?}"
            );
        }
    }

    #[test]
    fn only_multitask_is_coordinator() {
        for m in ChatMode::ALL {
            let is_coord = m.role() == AgentRole::Coordinator;
            assert_eq!(
                is_coord,
                m == ChatMode::Multitask,
                "role mismatch for {m:?}"
            );
        }
    }

    #[test]
    fn ask_and_debug_and_plan_carry_prompt_sections() {
        assert!(!ChatMode::Ask.system_prompt_suffix().is_empty());
        assert!(!ChatMode::Debug.system_prompt_suffix().is_empty());
        assert!(!ChatMode::Plan.system_prompt_suffix().is_empty());
        assert!(ChatMode::Agent.system_prompt_suffix().is_empty());
    }
}
