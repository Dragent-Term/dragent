import type { TerminalDelta } from "../transport";
import { TERMINAL_LIMITS as limits } from "./limits";
import { integer, normalizeAgents, normalizeLine, normalizeSurface, optionalText, text, unsigned } from "./normalize";
import type { TerminalBlock, TerminalLine, TerminalState } from "./types";

export { TERMINAL_LIMITS } from "./limits";

export function createTerminalState(): TerminalState {
  return {
    blocks: [], agentBlocks: [], altScreen: null, surface: null,
    styledOutputSeen: false, rawOutputPending: false, historyTruncated: false,
  };
}

function emptyBlock(id: string): TerminalBlock {
  return {
    id, command: "", cwd: null, tsMs: 0, status: "running", exitCode: null,
    durationMs: null, failed: false, lines: [], discardedBeforeLine: 0, truncated: false,
  };
}

function lineCharacters(line: TerminalLine): number {
  return line.spans.reduce((count, span) => count + span.text.length, 0);
}

/** Retain the newest rows without allocating gaps for arbitrary base_line values. */
function boundBlocks(state: TerminalState, values: readonly TerminalBlock[]): TerminalState {
  const blocks = values.slice(-limits.blocks);
  let remainingLines: number = limits.totalLines;
  let remainingCharacters: number = limits.totalCharacters;
  let historyTruncated = state.historyTruncated || values.length > blocks.length;
  for (let i = blocks.length - 1; i >= 0; i--) {
    const block = blocks[i]!;
    let first = block.lines.length;
    const earliest = Math.max(0, block.lines.length - limits.linesPerBlock);
    while (first > earliest && remainingLines > 0) {
      const cost = lineCharacters(block.lines[first - 1]!);
      if (cost > remainingCharacters) break;
      remainingCharacters -= cost;
      remainingLines--;
      first--;
    }
    if (first > 0) {
      blocks[i] = {
        ...block,
        lines: block.lines.slice(first),
        discardedBeforeLine: Math.max(block.discardedBeforeLine, block.lines[first - 1]!.index + 1),
        truncated: true,
      };
      historyTruncated = true;
    } else if (block.truncated) {
      historyTruncated = true;
    }
  }
  return { ...state, blocks, historyTruncated };
}

function updateBlock(state: TerminalState, id: string, update: (block: TerminalBlock) => TerminalBlock): TerminalState {
  const index = state.blocks.findIndex((block) => block.id === id);
  const blocks = [...state.blocks];
  if (index < 0) blocks.push(update(emptyBlock(id)));
  else blocks[index] = update(blocks[index]!);
  return boundBlocks(state, blocks);
}

function blockId(delta: TerminalDelta): string | null {
  return typeof delta.block_id === "string" && delta.block_id.trim().length > 0 && delta.block_id.length <= 256
    ? delta.block_id : null;
}

/**
 * Pure, replay-safe projection of the styled gateway contract. Root must feed
 * every subscribed delta in order; React render batching must not drop events.
 * Raw PTY bytes are deliberately not decoded: this viewer has no VT emulator.
 */
export function reduceTerminalDelta(state: TerminalState, delta: TerminalDelta): TerminalState {
  if (!delta || typeof delta !== "object") return state;
  switch (delta.kind) {
    case "surface_state": {
      const surface = normalizeSurface(delta.state);
      return surface ? { ...state, surface } : state;
    }
    case "block_started": {
      const id = blockId(delta);
      if (!id) return state;
      return updateBlock(state, id, (block) => ({
        ...block,
        command: typeof delta.command === "string" ? text(delta.command) : block.command,
        cwd: delta.cwd === null || typeof delta.cwd === "string" ? optionalText(delta.cwd) : block.cwd,
        tsMs: integer(delta.ts_ms) ? delta.ts_ms : block.tsMs,
        // An attach replay of the start must not reopen an already completed block.
        truncated: block.truncated || (typeof delta.command === "string" && delta.command.length > limits.metadataCharacters),
      }));
    }
    case "block_lines": {
      const id = blockId(delta);
      if (!id || !unsigned(delta.base_line) || !Array.isArray(delta.lines)) return state;
      const base = delta.base_line;
      const incoming = delta.lines;
      if (!Number.isSafeInteger(base + incoming.length)) return state;
      const next = updateBlock(state, id, (block) => {
        const rows = new Map(block.lines.map((line) => [line.index, line]));
        const start = Math.max(0, incoming.length - limits.linesPerBlock, block.discardedBeforeLine - base);
        let truncated = block.truncated || start > 0;
        // Replace only the announced rows; shorter patches do not erase the tail.
        for (let i = start; i < incoming.length; i++) {
          const line = normalizeLine(incoming[i], base + i);
          if (!line) continue;
          rows.set(line.index, line);
          truncated ||= line.truncated;
        }
        return { ...block, lines: [...rows.values()].sort((a, b) => a.index - b.index), truncated };
      });
      return { ...next, styledOutputSeen: true, rawOutputPending: false };
    }
    case "block_completed": {
      const id = blockId(delta);
      if (!id) return state;
      return updateBlock(state, id, (block) => ({
        ...block,
        status: "completed",
        exitCode: integer(delta.exit_code) ? delta.exit_code : delta.exit_code === null ? null : block.exitCode,
        durationMs: unsigned(delta.duration_ms) ? delta.duration_ms : delta.duration_ms === null ? null : block.durationMs,
        failed: typeof delta.failed === "boolean" ? delta.failed : block.failed,
      }));
    }
    case "agent_blocks": {
      if (!Array.isArray(delta.blocks)) return state;
      const snapshot = normalizeAgents(delta.blocks);
      return { ...state, agentBlocks: snapshot.blocks, historyTruncated: state.historyTruncated || snapshot.truncated };
    }
    case "alt_screen": {
      // Reject empty and fractional grids. Oversized row counts truncate to
      // the retained bound below (with truncated set) so attach replay of a
      // full-screen program can never force unbounded work; per-row bounds
      // still apply through normalizeLine.
      if (!Number.isSafeInteger(delta.rows) || (delta.rows as number) < 1
        || !Number.isSafeInteger(delta.cols) || (delta.cols as number) < 1
        || !Array.isArray(delta.lines)) return state;
      const rows = delta.rows as number, cols = delta.cols as number;
      const capacity = Math.min(rows, limits.altRows);
      const kept = delta.lines.slice(0, capacity);
      const lines = kept
        .map((line, index) => normalizeLine(line, index)).filter((line): line is TerminalLine => line !== null);
      return {
        ...state,
        styledOutputSeen: true,
        rawOutputPending: false,
        altScreen: {
          rows,
          columns: cols,
          cursorRow: unsigned(delta.cursor_row) ? Math.min(delta.cursor_row, Math.max(0, rows - 1)) : 0,
          cursorColumn: unsigned(delta.cursor_col) ? Math.min(delta.cursor_col, Math.max(0, cols - 1)) : 0,
          cursorVisible: delta.cursor_visible === true,
          lines,
          truncated: delta.lines.length > capacity || lines.length < kept.length || lines.some((line) => line.truncated),
        },
      };
    }
    case "alt_screen_ended": return state.altScreen ? { ...state, altScreen: null } : state;
    case "output_chunk":
      if (state.styledOutputSeen || state.rawOutputPending || !blockId(delta) || !unsigned(delta.seq) || delta.seq < 1
        || typeof delta.data_base64 !== "string" || delta.data_base64.length === 0) return state;
      return { ...state, rawOutputPending: true };
    default: return state;
  }
}
