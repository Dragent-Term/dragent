/**
 * A unified diff (`git diff`) as rows the phone can draw: hunk headers and
 * added, removed and unchanged lines with their line numbers. Pure text in,
 * plain data out; the file header lines git prints first are left out.
 */

export interface DiffRow {
  readonly kind: "hunk" | "add" | "del" | "ctx" | "note";
  readonly text: string;
  /** Line number in the old file (removed and unchanged lines). */
  readonly oldLine: number | null;
  /** Line number in the new file (added and unchanged lines). */
  readonly newLine: number | null;
}

export interface ParsedDiff {
  readonly rows: readonly DiffRow[];
  readonly added: number;
  readonly removed: number;
}

const HUNK = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@(.*)$/u;

export function parseUnifiedDiff(diff: string): ParsedDiff {
  const rows: DiffRow[] = [];
  let added = 0;
  let removed = 0;
  let oldLine = 0;
  let newLine = 0;
  let inHunk = false;
  const lines = diff.split("\n");
  // A trailing newline leaves one empty piece that is not a line.
  if (lines.length > 0 && lines[lines.length - 1] === "") lines.pop();
  for (const line of lines) {
    const hunk = HUNK.exec(line);
    if (hunk) {
      inHunk = true;
      oldLine = Number(hunk[1]);
      newLine = Number(hunk[2]);
      rows.push({ kind: "hunk", text: line, oldLine: null, newLine: null });
      continue;
    }
    if (!inHunk) continue;
    if (line.startsWith("+")) {
      rows.push({ kind: "add", text: line.slice(1), oldLine: null, newLine });
      newLine += 1;
      added += 1;
    } else if (line.startsWith("-")) {
      rows.push({ kind: "del", text: line.slice(1), oldLine, newLine: null });
      oldLine += 1;
      removed += 1;
    } else if (line.startsWith("\\")) {
      rows.push({ kind: "note", text: line.slice(2), oldLine: null, newLine: null });
    } else {
      rows.push({ kind: "ctx", text: line.startsWith(" ") ? line.slice(1) : line, oldLine, newLine });
      oldLine += 1;
      newLine += 1;
    }
  }
  return { rows, added, removed };
}
