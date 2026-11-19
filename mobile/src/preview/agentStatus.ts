import type { AgentDotState } from "../components/AgentStateDot";
import type { PreviewTabDetail } from "./types";

/**
 * The tab color flow: gray idle → glowing orange while working → emerald
 * once finished but unopened (needs review). Opening the tab records it seen,
 * so the emerald drops back to gray. A stop the user issued reads muted, and
 * failures read red, instead of ever looking like fresh work.
 */
export function previewTabAgentState(tab: PreviewTabDetail | null, seenTsMs = 0): AgentDotState {
  if (!tab || tab.kind === "browser") return "idle";
  if (tab.kind === "terminal") {
    if (tab.terminal.surface?.command_running) return "working";
    let latest = 0;
    for (const block of tab.terminal.blocks) latest = Math.max(latest, block.tsMs);
    return latest > seenTsMs ? "done" : "idle";
  }
  const blocks = tab.terminal.agentBlocks;
  const last = blocks[blocks.length - 1];
  if (!last) return "idle";
  if (last.kind === "user_query") return "working";
  if (last.kind === "system_note") {
    if (/stopp/i.test(last.text)) return "interrupted";
    return last.tsMs > seenTsMs ? "done" : "idle";
  }
  return last.tsMs > seenTsMs ? "done" : "idle";
}
