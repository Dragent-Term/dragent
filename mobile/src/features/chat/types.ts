import type { TerminalAgentBlock, TerminalState } from "../../terminal";

/**
 * Read-only agent conversation model. The host owns all content through the
 * already-bounded `TerminalAgentBlock` projection; this component never parses
 * tool JSON, diffs, or attachments out of flattened text and never invents
 * actions. Approval/question/plan_review rows stay read-only because the wire
 * exposes no response commands.
 */
export interface AgentConversationProps {
  /** Full terminal state; `agentBlocks`, surface status and truncation flags are read. */
  state?: TerminalState;
  /** Direct block list override. Takes precedence over `state.agentBlocks` when provided. */
  blocks?: readonly TerminalAgentBlock[];
  /** Session identity. Resets scroll/follow state when the conversation changes. */
  sessionId?: string;
}

export type { TerminalAgentBlock };
