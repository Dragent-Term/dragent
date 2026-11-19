/**
 * Turns a conversation into the lines the phone draws. Each turn is your
 * message, then the agent's output in order: replies in full, tool calls as
 * step lines hanging from a guide line, and back-to-back calls grouped under
 * one tally ("6 commands · 3 edits") that expands. While a turn is running
 * its timer ("Working · 12s") is the last line and the newest steps of a
 * grouped run stay listed; once it ends the timer becomes "Done in 1m 3s"
 * under your message, and a card lists the files the turn touched.
 *
 * No React here, so the layout is unit tested directly.
 */

export type StepKind = "read" | "edit" | "delete" | "search" | "execute" | "fetch" | "think" | "agent" | "plan" | "other";

/** One tool call (or thought) as a line on the rail. */
export interface Step {
  readonly id: string;
  readonly kind: StepKind;
  /** "Read", "Edited", "Searched", "Running"…: what happened, or is happening. */
  readonly verb: string | null;
  /** A file the step read or changed, shown as a chip (its name). */
  readonly file: string | null;
  /** The rest of the line: a command (mono), a query, a URL, a description. */
  readonly label: string;
  /** The label is a command, set in mono. */
  readonly mono: boolean;
  /** What opens under the line, when there is more than it shows. */
  readonly detail: string | null;
  readonly added: number | null;
  readonly removed: number | null;
  readonly running: boolean;
  readonly failed: boolean;
  readonly startedAt: number | null;
}

/** A row of the conversation, before layout. `C` is a card only the caller draws. */
export type Entry<C> =
  | { readonly kind: "you"; readonly id: string; readonly text: string; readonly at: number | null; readonly note?: YouNote }
  | { readonly kind: "reply"; readonly id: string; readonly text: string; readonly at: number | null; readonly streaming: boolean }
  | { readonly kind: "step"; readonly step: Step; readonly at: number | null }
  | { readonly kind: "card"; readonly id: string; readonly card: C; readonly at: number | null }
  /** The agent finished a turn; `durationMs` when it said how long it took. */
  | { readonly kind: "end"; readonly id: string; readonly at: number | null; readonly durationMs: number | null };

/** A message of yours that hasn't settled: still sending, failed, or queued. */
export type YouNote =
  | { readonly kind: "sending" }
  | { readonly kind: "failed"; readonly reason: string }
  | { readonly kind: "queued" };

export interface ChangedFile {
  readonly path: string;
  readonly name: string;
  readonly added: number;
  readonly removed: number;
}

export type Line<C> =
  | { readonly kind: "you"; readonly key: string; readonly id: string; readonly text: string; readonly note: YouNote | null; readonly turnStart: boolean }
  /** The turn's clock: sweeping while it runs, then how long it took. */
  | {
      readonly kind: "clock";
      readonly key: string;
      readonly startedAt: number | null;
      readonly completedAt: number | null;
      readonly running: boolean;
      readonly waiting: boolean;
    }
  | {
      readonly kind: "reply";
      readonly key: string;
      readonly id: string;
      readonly text: string;
      readonly streaming: boolean;
      /** The turn's last reply, settled: it gets Copy and its time. */
      readonly lastReply: boolean;
      readonly at: number | null;
      /** Right under work (steps or the clock): sits closer. */
      readonly afterWork: boolean;
    }
  /** Consecutive steps under one summary line. */
  | { readonly kind: "run"; readonly key: string; readonly steps: readonly Step[]; readonly active: boolean }
  /** A step on its own. */
  | { readonly kind: "step"; readonly key: string; readonly step: Step }
  | { readonly kind: "card"; readonly key: string; readonly card: C }
  | { readonly kind: "changes"; readonly key: string; readonly files: readonly ChangedFile[] };

export interface LayoutOptions {
  /** The agent is working on the latest turn. */
  readonly working: boolean;
  /** …and is held on something only you can answer. */
  readonly waiting: boolean;
  /** What "now" is for a turn still running with no newer row (tests pass it). */
  readonly now?: number;
}

function baseName(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? path;
}

/** Lay `entries` out as lines. */
export function layoutTranscript<C>(entries: readonly Entry<C>[], options: LayoutOptions): Line<C>[] {
  // Split into turns: each starts at a run of your messages.
  const turns: Entry<C>[][] = [];
  let current: Entry<C>[] = [];
  let sawAgent = false;
  for (const entry of entries) {
    if (entry.kind === "you" && sawAgent) {
      turns.push(current);
      current = [];
      sawAgent = false;
    }
    if (entry.kind !== "you") sawAgent = true;
    current.push(entry);
  }
  if (current.length > 0) turns.push(current);

  const lines: Line<C>[] = [];
  turns.forEach((turn, turnIndex) => {
    const latest = turnIndex === turns.length - 1;
    const yours = turn.filter((entry): entry is Extract<Entry<C>, { kind: "you" }> => entry.kind === "you");
    const work = turn.filter((entry) => entry.kind !== "you");
    // A turn with only your messages that isn't being worked on has no clock.
    const ended = work.some((entry) => entry.kind === "end");
    const running = latest && options.working && !ended;
    const anyPending = yours.some((entry) => entry.note && entry.note.kind !== "failed");
    const start = yours.find((entry) => entry.at !== null)?.at ?? work.find((entry) => entry.at !== null)?.at ?? null;
    const endEntry = work.find((entry): entry is Extract<Entry<C>, { kind: "end" }> => entry.kind === "end");
    let completedAt: number | null = null;
    if (!running) {
      if (endEntry?.durationMs != null && start !== null) completedAt = start + endEntry.durationMs;
      else {
        const times = work.map((entry) => entry.at).filter((at): at is number => at !== null);
        completedAt = times.length > 0 ? Math.max(...times) : null;
      }
    }

    yours.forEach((entry, index) => {
      lines.push({ kind: "you", key: `you:${entry.id}`, id: entry.id, text: entry.text, note: entry.note ?? null, turnStart: index === 0 && lines.length > 0 });
    });
    if (yours.length === 0 && work.length === 0) return;
    const clockLive = running || (latest && anyPending && work.length === 0);
    const clock: Line<C> | null = work.length > 0 || clockLive
      ? {
          kind: "clock",
          key: `clock:${yours[0]?.id ?? (work[0] ? entryId(work[0]) : String(turnIndex))}`,
          startedAt: start,
          completedAt,
          running: clockLive,
          waiting: running && options.waiting,
        }
      : null;
    // Running: the timer goes last so it sits just above the message box and
    // never scrolls away while output streams in. Finished: it moves up under
    // your message as the turn's duration.
    if (clock && !clockLive) lines.push(clock);

    type RunDraft = { kind: "run"; key: string; steps: Step[]; active: boolean };
    const open: { run: RunDraft | null } = { run: null };
    let lastReplyAt = -1;
    const files = new Map<string, ChangedFile>();
    const endRun = () => {
      const run = open.run;
      if (run && run.steps.length === 1) {
        const at = lines.indexOf(run);
        lines[at] = { kind: "step", key: `step:${run.steps[0]!.id}`, step: run.steps[0]! };
      }
      open.run = null;
    };
    for (const entry of work) {
      if (entry.kind === "step") {
        const { step } = entry;
        if (step.kind === "edit" && step.file && (step.added !== null || step.removed !== null)) {
          const key = step.detail ?? step.file;
          const seen = files.get(key);
          files.set(key, {
            path: key,
            name: step.file,
            added: (seen?.added ?? 0) + (step.added ?? 0),
            removed: (seen?.removed ?? 0) + (step.removed ?? 0),
          });
        }
        if (open.run) {
          open.run.steps.push(step);
        } else {
          open.run = { kind: "run", key: `run:${step.id}`, steps: [step], active: false };
          lines.push(open.run);
        }
        continue;
      }
      if (entry.kind === "end") continue;
      endRun();
      if (entry.kind === "reply") {
        const previous = lines[lines.length - 1];
        lines.push({
          kind: "reply",
          key: `reply:${entry.id}`,
          id: entry.id,
          text: entry.text,
          streaming: entry.streaming,
          lastReply: false,
          at: entry.at,
          afterWork: previous !== undefined && (previous.kind === "run" || previous.kind === "step" || previous.kind === "clock"),
        });
        lastReplyAt = lines.length - 1;
      } else {
        lines.push({ kind: "card", key: `card:${entry.id}`, card: entry.card });
      }
    }
    const tail = open.run as RunDraft | null;
    if (running && tail && lines[lines.length - 1] === tail) tail.active = true;
    endRun();
    if (clock && clockLive) lines.push(clock);

    if (!running && lastReplyAt >= 0) {
      const reply = lines[lastReplyAt] as Extract<Line<C>, { kind: "reply" }>;
      if (!reply.streaming) lines[lastReplyAt] = { ...reply, lastReply: true };
    }
    if (!running && files.size > 0) {
      const card: Line<C> = { kind: "changes", key: `changes:${yours[0]?.id ?? turnIndex}`, files: [...files.values()] };
      if (lastReplyAt >= 0) lines.splice(lastReplyAt + 1, 0, card);
      else lines.push(card);
    }
  });
  return lines;
}

function entryId<C>(entry: Entry<C>): string {
  return entry.kind === "step" ? entry.step.id : entry.id;
}

const SUMMARY_ORDER: ReadonlyArray<[StepKind[], string, string]> = [
  [["execute"], "command", "commands"],
  [["edit", "delete"], "edit", "edits"],
  [["read"], "read", "reads"],
  [["search"], "search", "searches"],
  [["fetch"], "fetch", "fetches"],
  [["agent", "plan", "other"], "other step", "other steps"],
];

/** "6 commands · 3 edits · 2 reads": a tally of what a group of steps did. */
export function runSummary(steps: readonly Step[]): string {
  const parts: string[] = [];
  for (const [kinds, one, many] of SUMMARY_ORDER) {
    const count = steps.filter((step) => kinds.includes(step.kind)).length;
    if (count > 0) parts.push(`${count} ${count === 1 ? one : many}`);
  }
  // A group made only of thoughts has nothing to count.
  return parts.join(" · ") || "Reasoning";
}

/** The kind most of a run's steps are (its icon). */
export function dominantKind(steps: readonly Step[]): StepKind {
  const counts = new Map<StepKind, number>();
  for (const step of steps) if (step.kind !== "think") counts.set(step.kind, (counts.get(step.kind) ?? 0) + 1);
  let best: StepKind = "other";
  let most = 0;
  for (const [kind, count] of counts) if (count > most) { best = kind; most = count; }
  return best;
}

/** "Working · 12s" while running, "Done in 1m 3s" after, "Waiting on you" when held. */
export function clockLabel(line: Extract<Line<unknown>, { kind: "clock" }>, now: number): string | null {
  if (line.waiting) return "Waiting on you";
  if (line.startedAt === null) return line.running ? "Working" : null;
  const end = line.running ? now : line.completedAt;
  if (end === null) return line.running ? "Working" : null;
  const seconds = Math.max(0, Math.floor((end - line.startedAt) / 1000));
  if (!line.running && seconds === 0) return null;
  const span = seconds < 60 ? `${seconds}s` : `${Math.floor(seconds / 60)}m ${seconds % 60}s`;
  return line.running ? `Working · ${span}` : `Done in ${span}`;
}

/** "12s", "1m 3s": how long a step has been running. */
export function runningTime(startedAt: number, now: number): string {
  const seconds = Math.max(0, Math.floor((now - startedAt) / 1000));
  return seconds < 60 ? `${seconds}s` : `${Math.floor(seconds / 60)}m ${seconds % 60}s`;
}

// ---- Steps from a CLI agent's transcript (Claude Code, Codex) ----

export interface ToolRow {
  readonly id: string;
  readonly tool: string | null;
  readonly text: string;
  readonly error: boolean;
  readonly running: boolean;
  readonly path: string | null;
  readonly added: number | null;
  readonly removed: number | null;
  readonly at: number | null;
}

const SHELL_TOOLS = /^(bash|shell|exec|exec_command|local_shell|command|terminal|run_command|run_terminal_cmd|run_shell_command)$/i;

/** The verb a shell command reads as, when it only read, searched or listed. */
function shellIntent(command: string): { verb: [string, string]; kind: StepKind; subject: string; file: boolean } | null {
  const words = command.trim().split(/\s+/);
  const program = (words[0] ?? "").replace(/^.*\//, "");
  const last = words[words.length - 1] ?? "";
  if (/^(cat|head|tail|less|bat)$/.test(program) && words.length > 1 && !/[|><;&]/.test(command)) {
    return { verb: ["Reading", "Read"], kind: "read", subject: baseName(last), file: true };
  }
  if (program === "sed" && words[1] === "-n" && !/[|><;&]/.test(command)) {
    return { verb: ["Reading", "Read"], kind: "read", subject: baseName(last), file: true };
  }
  if (/^(rg|grep|ag|ack)$/.test(program) && !/[|><;&]/.test(command)) {
    const query = words.slice(1).find((word) => !word.startsWith("-")) ?? "";
    return { verb: ["Searching", "Searched"], kind: "search", subject: query.replace(/^["']|["']$/g, ""), file: false };
  }
  if (/^(ls|tree|find|fd)$/.test(program) && !/[|><;&]/.test(command)) {
    const path = words.slice(1).find((word) => !word.startsWith("-")) ?? ".";
    return { verb: ["Listing", "Listed"], kind: "search", subject: path, file: false };
  }
  return null;
}

/** A transcript tool row as a step. */
export function stepFromToolRow(row: ToolRow, live: boolean): Step {
  const name = row.tool ?? "Tool";
  const running = row.running && live;
  const base = {
    id: row.id,
    detail: null as string | null,
    added: row.added,
    removed: row.removed,
    running,
    failed: row.error,
    startedAt: row.at,
  };
  const pick = (a: string, b: string) => (running ? a : b);
  const lower = name.toLowerCase();
  const path = row.path;
  if (/^(delete|delete_file|remove_file)$/i.test(name) && (path || row.text)) {
    return { ...base, kind: "delete", verb: pick("Deleting", "Deleted"), file: baseName(path ?? row.text), label: "", mono: false, detail: path ?? row.text };
  }
  if (/^(edit|multiedit|write|write_file|replace|patch|apply_patch|applypatch|strreplace|notebookedit|str_replace_editor|str_replace_based_edit_tool|create|create_file)$/i.test(name) && (path || row.text)) {
    const created = ["write", "write_file", "create", "create_file"].includes(lower);
    return {
      ...base,
      kind: "edit",
      verb: created ? pick("Writing", "Wrote") : pick("Editing", "Edited"),
      file: baseName(path ?? row.text),
      label: "",
      mono: false,
      detail: path ?? row.text,
    };
  }
  if (/^(read|readfile|view|read_file|read_many_files|open_file)$/i.test(name)) {
    return { ...base, kind: "read", verb: pick("Reading", "Read"), file: baseName(path ?? row.text), label: "", mono: false, detail: path ?? row.text };
  }
  if (/^(grep|rg|glob|search|search_files|search_file_content|codebase_search|semanticsearch|find)$/i.test(name)) {
    return { ...base, kind: "search", verb: pick("Searching", "Searched"), file: null, label: row.text, mono: false, detail: null };
  }
  if (/^(ls|list_dir|list|list_directory)$/i.test(name)) {
    return { ...base, kind: "search", verb: pick("Listing", "Listed"), file: null, label: row.text, mono: false, detail: null };
  }
  if (/^web_?fetch$|^fetch(_url)?$/i.test(name)) {
    return { ...base, kind: "fetch", verb: pick("Fetching", "Fetched"), file: null, label: row.text, mono: false, detail: null };
  }
  if (/^(web_?search|google_web_search)$/i.test(name)) {
    return { ...base, kind: "fetch", verb: pick("Searching the web", "Searched the web"), file: null, label: row.text, mono: false, detail: null };
  }
  if (/^(task|agent)$/i.test(name)) {
    return { ...base, kind: "agent", verb: null, file: null, label: row.text || "Agent", mono: false, detail: null };
  }
  if (/^(todowrite|update_plan)$/i.test(name)) {
    return { ...base, kind: "plan", verb: null, file: null, label: running ? "Updating the plan" : "Updated the plan", mono: false, detail: null };
  }
  if (SHELL_TOOLS.test(name)) {
    const intent = shellIntent(row.text);
    if (intent) {
      return {
        ...base,
        kind: intent.kind,
        verb: pick(...intent.verb),
        file: intent.file ? intent.subject : null,
        label: intent.file ? "" : intent.subject,
        mono: false,
        detail: `$ ${row.text}`,
      };
    }
    return { ...base, kind: "execute", verb: running ? "Running" : null, file: null, label: row.text, mono: true, detail: row.text.length > 60 ? `$ ${row.text}` : null };
  }
  return { ...base, kind: "other", verb: null, file: null, label: row.text && row.text !== name ? `${name} ${row.text}` : name, mono: false, detail: null };
}

/** The first non-empty line of `text`, cut to `max` characters. */
export function firstLine(text: string, max = 120): string {
  const line = (text.split("\n").find((part) => part.trim() !== "") ?? "").trim();
  return line.length > max ? `${line.slice(0, max - 1)}…` : line;
}
